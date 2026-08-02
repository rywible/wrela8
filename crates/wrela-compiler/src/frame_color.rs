//! Deterministic interference coloring for compiler-owned Flow frame homes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::flow_liveness::FlowLiveness;
use crate::flowwir::FlowWirFn;
use crate::frame_plan::{FramePlan, FrameSlot, Home, SlotId, StorageClass};
use crate::mwir::LayoutCtx;
use crate::mwir::Temp;

fn candidate(plan: &FramePlan, t: Temp) -> bool {
    !matches!(
        plan.classes[t.0],
        StorageClass::Pinned | StorageClass::Escaped
    ) && matches!(plan.homes[t.0], Home::Frame { .. })
}

fn add_clique(adj: &mut [BTreeSet<Temp>], values: &[Temp]) {
    for (i, &a) in values.iter().enumerate() {
        for &b in &values[i + 1..] {
            if a == b {
                continue;
            }
            adj[a.0].insert(b);
            adj[b.0].insert(a);
        }
    }
}

fn interference(a: &FlowLiveness, plan: &FramePlan) -> Vec<BTreeSet<Temp>> {
    let mut adj = vec![BTreeSet::new(); plan.homes.len()];
    for p in &a.points {
        add_clique(&mut adj, &p.live_in);
        add_clique(&mut adj, &p.live_out);
        for &d in &p.defs {
            for &live in &p.live_out {
                if d != live {
                    adj[d.0].insert(live);
                    adj[live.0].insert(d);
                }
            }
        }
    }
    for suspend in &a.suspends {
        add_clique(&mut adj, &suspend.save);
    }
    adj
}

fn slot_class(slot: &FrameSlot) -> (u32, u32) {
    (slot.size, slot.alignment)
}

/// Color exact size/alignment classes.  Slot IDs remain stable; only their
/// final offsets and occupant lists change.
pub fn materialize_all_homes(
    f: &FlowWirFn,
    plan: &FramePlan,
    layout: &LayoutCtx,
) -> Result<FramePlan, String> {
    let mut out = plan.clone();
    let mut cursor = out
        .slots
        .iter()
        .map(|slot| slot.offset.saturating_add(slot.size))
        .max()
        .unwrap_or(out.abi_prefix_bytes);
    for (i, home) in plan.homes.iter().enumerate() {
        if !matches!(home, Home::None) {
            continue;
        }
        let size: u32 = crate::mwir::size_of(&f.frame.temp_types[i], layout)?
            .try_into()
            .map_err(|_| format!("temp t{i} frame size does not fit u32"))?;
        cursor = cursor.div_ceil(8) * 8;
        let id = out.slots.len();
        out.slots.push(FrameSlot {
            id,
            size: size.max(8),
            alignment: 8,
            offset: cursor,
            occupants: vec![Temp(i)],
        });
        out.homes[i] = Home::Frame { slot: id };
        cursor = cursor
            .checked_add(size.max(8))
            .ok_or_else(|| "FlowWir frame offset overflow".to_string())?;
    }
    out.frame_size = cursor.div_ceil(16) * 16;
    Ok(out)
}

pub fn color_flow(
    _f: &FlowWirFn,
    analysis: &FlowLiveness,
    plan: &FramePlan,
) -> Result<FramePlan, String> {
    let mut out = plan.clone();
    let adj = interference(analysis, plan);
    let mut original: BTreeMap<Temp, SlotId> = BTreeMap::new();
    for (t, home) in plan.homes.iter().enumerate() {
        if let Home::Frame { slot } = home {
            original.insert(Temp(t), *slot);
        }
    }

    // Preserve non-colorable slots and clear the occupants of colorable slots.
    let mut colorable_slots = BTreeSet::new();
    for t in original.keys().copied() {
        if candidate(plan, t) {
            colorable_slots.insert(original[&t]);
        }
    }
    for slot in &mut out.slots {
        if colorable_slots.contains(&slot.id) {
            slot.occupants.clear();
        }
    }

    let mut temps: Vec<Temp> = original
        .keys()
        .copied()
        .filter(|t| candidate(plan, *t))
        .collect();
    temps.sort_unstable();
    for t in temps {
        let source_slot = original[&t];
        let class = slot_class(&plan.slots[source_slot]);
        let mut pick = None;
        for slot in out
            .slots
            .iter_mut()
            .filter(|s| colorable_slots.contains(&s.id))
            .filter(|s| slot_class(s) == class)
            .collect::<Vec<_>>()
        {
            let conflict = slot.occupants.iter().any(|other| adj[t.0].contains(other));
            if !conflict {
                slot.occupants.push(t);
                pick = Some(slot.id);
                break;
            }
        }
        let slot = match pick {
            Some(slot) => slot,
            None => {
                let id = out.slots.len();
                let old = &plan.slots[source_slot];
                out.slots.push(FrameSlot {
                    id,
                    size: old.size,
                    alignment: old.alignment,
                    offset: 0,
                    occupants: vec![t],
                });
                colorable_slots.insert(id);
                id
            }
        };
        out.homes[t.0] = Home::Frame { slot };
    }

    // First-fit leaves the original candidate slots that were not selected
    // empty.  Keeping those slots in the physical layout makes coloring a
    // metadata-only no-op: the empty slots still consume their old bytes.
    // Compact them and remap homes before assigning offsets.
    let mut remap = BTreeMap::<SlotId, SlotId>::new();
    let mut compact = Vec::new();
    for slot in out
        .slots
        .into_iter()
        .filter(|slot| !slot.occupants.is_empty())
    {
        let new_id = compact.len();
        remap.insert(slot.id, new_id);
        compact.push(FrameSlot { id: new_id, ..slot });
    }
    for home in &mut out.homes {
        if let Home::Frame { slot } = home {
            *slot = *remap
                .get(slot)
                .ok_or_else(|| "colored home refers to an empty frame slot".to_string())?;
        }
    }
    out.slots = compact;

    // ABI prefixes are not candidates.  Lay out slots by the documented
    // decreasing alignment/size order, with stable compacted slot ID as the
    // tie-break.  Slot IDs remain vector indices; physical order is separate.
    let mut order: Vec<SlotId> = (0..out.slots.len()).collect();
    order.sort_by_key(|&id| {
        let slot = &out.slots[id];
        (
            std::cmp::Reverse(slot.alignment),
            std::cmp::Reverse(slot.size),
            slot.id,
        )
    });
    let mut cursor = out.abi_prefix_bytes;
    for id in order {
        let slot = &mut out.slots[id];
        cursor = cursor.div_ceil(slot.alignment.max(1)) * slot.alignment.max(1);
        slot.offset = cursor;
        cursor = cursor
            .checked_add(slot.size)
            .ok_or_else(|| "frame slot offset overflow".to_string())?;
    }
    out.frame_size = cursor.div_ceil(16) * 16;
    if out.frame_size > plan.frame_size {
        return Err(format!(
            "frame coloring grew the frame from {} to {} bytes",
            plan.frame_size, out.frame_size
        ));
    }

    // Validate the resulting coloring directly, rather than relying on the
    // first-fit loop's reasoning.
    for t in 0..adj.len() {
        let Home::Frame { slot: a_slot } = &out.homes[t] else {
            continue;
        };
        let a_slot = *a_slot;
        for &other in &adj[t] {
            if other.0 <= t {
                continue;
            }
            if let Home::Frame { slot: b_slot } = out.homes[other.0] {
                if a_slot == b_slot {
                    return Err(format!(
                        "interfering temps t{t} and {other} share frame slot {a_slot}"
                    ));
                }
            }
        }
    }
    for (i, slot) in out.slots.iter().enumerate() {
        if slot.offset % slot.alignment.max(1) != 0 {
            return Err(format!("slot {i} has an unaligned offset {}", slot.offset));
        }
        if slot.offset.saturating_add(slot.size) > out.frame_size {
            return Err(format!("slot {i} falls outside the frame"));
        }
    }
    Ok(out)
}

pub fn dump_plan(plan: &FramePlan) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "colored frame_size={}", plan.frame_size);
    for slot in &plan.slots {
        let occupants = slot
            .occupants
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            out,
            "slot{} offset={} size={} align={} occupants=[{}]",
            slot.id, slot.offset, slot.size, slot.alignment, occupants
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow_liveness;
    use crate::flowwir::{FlowInst, FlowWirFn, FrameLayout, State, Transition};
    use crate::frame_plan::plan_flow;
    use crate::mwir::{Inst, LayoutCtx};
    use crate::sema::types::Type;

    fn diamond() -> FlowWirFn {
        FlowWirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::Unit,
            frame: FrameLayout {
                temp_types: vec![Type::Bool, Type::U64, Type::U64, Type::U64],
                lineage_group_slot: Temp(0),
                lineage_deadline_slot: Temp(0),
            },
            states: vec![State {
                ops: vec![
                    FlowInst::Mwir(Inst::ConstBool {
                        dst: Temp(0),
                        value: true,
                    }),
                    FlowInst::Mwir(Inst::JumpIfFalse {
                        cond: Temp(0),
                        target: 3,
                    }),
                    FlowInst::Mwir(Inst::ConstInt {
                        dst: Temp(1),
                        ty: Type::U64,
                        value: 1,
                    }),
                    FlowInst::Mwir(Inst::Jump { target: 4 }),
                    FlowInst::Mwir(Inst::ConstInt {
                        dst: Temp(2),
                        ty: Type::U64,
                        value: 2,
                    }),
                    FlowInst::Mwir(Inst::ConstInt {
                        dst: Temp(3),
                        ty: Type::U64,
                        value: 3,
                    }),
                ],
                transition: Transition::Return(None),
            }],
        }
    }

    #[test]
    fn loop_carried_values_interfere() {
        let f = FlowWirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::Unit,
            frame: FrameLayout {
                temp_types: vec![Type::Bool, Type::U64, Type::U64],
                lineage_group_slot: Temp(0),
                lineage_deadline_slot: Temp(0),
            },
            states: vec![State {
                ops: vec![
                    FlowInst::Mwir(Inst::ConstInt {
                        dst: Temp(1),
                        ty: Type::U64,
                        value: 0,
                    }),
                    FlowInst::Mwir(Inst::ConstInt {
                        dst: Temp(2),
                        ty: Type::U64,
                        value: 1,
                    }),
                    FlowInst::Mwir(Inst::ArithWrapping {
                        dst: Temp(1),
                        op: crate::syntax::ast::BinOp::AddW,
                        ty: Type::U64,
                        lhs: Temp(1),
                        rhs: Temp(2),
                    }),
                    FlowInst::Mwir(Inst::Jump { target: 2 }),
                ],
                transition: Transition::Return(None),
            }],
        };
        let analysis = flow_liveness::analyze(&f).expect("analysis");
        let plan = plan_flow(&f, &analysis, &LayoutCtx::default()).expect("plan");
        let all = materialize_all_homes(&f, &plan, &LayoutCtx::default()).expect("homes");
        let colored = color_flow(&f, &analysis, &all).expect("color");
        let slot_of = |temp: usize| match colored.homes[temp] {
            Home::Frame { slot } => slot,
            ref home => panic!("t{temp} has no frame slot: {home:?}"),
        };
        assert_ne!(
            slot_of(1),
            slot_of(2),
            "the loop-carried accumulator and increment are simultaneously live"
        );
    }

    #[test]
    fn coloring_is_deterministic() {
        let f = diamond();
        let a = flow_liveness::analyze(&f).expect("analysis");
        let p = plan_flow(&f, &a, &LayoutCtx::default()).expect("plan");
        let x = color_flow(&f, &a, &p).expect("color");
        let y = color_flow(&f, &a, &p).expect("color");
        assert_eq!(x, y);
        let all = materialize_all_homes(&f, &p, &LayoutCtx::default()).expect("homes");
        let uncolored_size = all.frame_size;
        let uncolored_slots = all.slots.len();
        let all = color_flow(&f, &a, &all).expect("materialized color");
        assert!(all.homes.iter().all(|home| !matches!(home, Home::None)));
        assert!(all.frame_size <= uncolored_size);
        assert_eq!(
            uncolored_slots - all.slots.len(),
            2,
            "three noninterfering values reuse one physical slot"
        );
        assert_eq!(
            uncolored_size - all.frame_size,
            16,
            "the linked frame footprint loses two eight-byte homes"
        );
        crate::frame_plan::validate(&f, &a, &all).expect("materialized plan");
    }
}
