//! Explicit storage planning for FlowWir values.
//!
//! This is intentionally independent of the synchronous emitter's historical
//! virtual-register assignment.  A durable home and a state-local register
//! cache are different facts, even when both are used for the same temp.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::flow_liveness::{FlowLiveness, PointKind};
use crate::flowwir::{FlowInst, FlowWirFn, Transition};
use crate::mwir::Temp;
use crate::regalloc;
use crate::sema::types::Type;
use crate::syntax::ast::AccessMode;

pub type StateId = usize;
pub type SlotId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StorageClass {
    Pinned,
    Persistent,
    BoundaryLive,
    ResumeResult,
    Escaped,
    StateLocal,
    StateSpill,
}

impl StorageClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Persistent => "persistent",
            Self::BoundaryLive => "boundary-live",
            Self::ResumeResult => "resume-result",
            Self::Escaped => "escaped",
            Self::StateLocal => "state-local",
            Self::StateSpill => "state-spill",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    None,
    Frame { slot: SlotId },
    Pinned { offset: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSlot {
    pub id: SlotId,
    pub size: u32,
    pub alignment: u32,
    pub offset: u32,
    pub occupants: Vec<Temp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateAssignment {
    pub state: StateId,
    pub temp_regs: Vec<Option<u8>>,
    pub live_in_loads: Vec<Temp>,
    pub await_flushes: Vec<Temp>,
    pub exit_flushes: Vec<Temp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePlan {
    pub classes: Vec<StorageClass>,
    pub homes: Vec<Home>,
    pub states: Vec<StateAssignment>,
    pub slots: Vec<FrameSlot>,
    pub abi_prefix_bytes: u32,
    pub frame_size: u32,
}

fn align_up(value: u32, align: u32) -> u32 {
    let align = align.max(1);
    value.div_ceil(align) * align
}

fn scalar_size(ty: &Type, layout: &crate::mwir::LayoutCtx) -> Result<u32, String> {
    Ok(crate::mwir::size_of(ty, layout)? as u32)
}

fn scalar_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Unit
            | Type::Bool
            | Type::Char
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::Isize
            | Type::F32
            | Type::F64
    )
}

fn state_temp_sets(a: &FlowLiveness, n: usize) -> Vec<BTreeSet<Temp>> {
    let mut out = vec![BTreeSet::new(); n];
    for p in &a.points {
        let state = match p.kind {
            PointKind::Op { state, .. } | PointKind::Transition { state } => Some(state),
            PointKind::ResumeDef { state } => Some(state),
        };
        let Some(state) = state else { continue };
        out[state].extend(p.uses.iter().copied());
        out[state].extend(p.defs.iter().copied());
        out[state].extend(p.live_in.iter().copied());
        out[state].extend(p.live_out.iter().copied());
    }
    out
}

fn used_in_state(a: &FlowLiveness, state: usize, t: Temp) -> bool {
    a.points.iter().any(|p| {
        matches!(
            p.kind,
            PointKind::Op { state: s, .. } | PointKind::Transition { state: s } if s == state
        ) && (p.uses.contains(&t) || p.defs.contains(&t))
    })
}

fn escapes(f: &FlowWirFn) -> BTreeSet<Temp> {
    let mut out = BTreeSet::new();
    for state in &f.states {
        for op in &state.ops {
            if let FlowInst::Mwir(inst) = op {
                out.extend(crate::mwir_facts::inst_facts(inst).address_escapes);
            }
        }
    }
    out
}

fn pinned(f: &FlowWirFn) -> BTreeSet<Temp> {
    let mut out = BTreeSet::from([f.frame.lineage_group_slot, f.frame.lineage_deadline_slot]);
    if let Some((t, _)) = f.receiver {
        out.insert(t);
    }
    for &(t, mode) in &f.params {
        if mode != AccessMode::Read {
            out.insert(t);
        }
    }
    out
}

fn persistent(a: &FlowLiveness) -> BTreeSet<Temp> {
    a.suspends
        .iter()
        .flat_map(|s| s.save.iter().copied())
        .collect()
}

fn boundary_live(f: &FlowWirFn, a: &FlowLiveness) -> BTreeSet<Temp> {
    let mut out = BTreeSet::new();
    for (state, s) in f.states.iter().enumerate() {
        if !matches!(
            s.transition,
            Transition::Jump(_) | Transition::Branch { .. }
        ) {
            continue;
        }
        if let Some(p) = a
            .points
            .iter()
            .find(|p| p.kind == PointKind::Transition { state })
        {
            out.extend(p.live_out.iter().copied());
        }
    }
    out
}

fn resume_results(a: &FlowLiveness) -> BTreeSet<Temp> {
    a.suspends.iter().map(|s| s.result_temp).collect()
}

fn same_state_interference(a: &FlowLiveness, state: usize, left: Temp, right: Temp) -> bool {
    a.points.iter().any(|p| {
        let in_state = matches!(
            p.kind,
            PointKind::Op { state: s, .. } | PointKind::Transition { state: s } if s == state
        );
        if !in_state {
            return false;
        }
        let mentioned = |t| p.uses.contains(&t) || p.defs.contains(&t);
        if mentioned(left) && mentioned(right) {
            return true;
        }
        // A definition writes its register whether or not the defined value is
        // ever read again.  A dead def is still a write, so it destroys any
        // value that has to survive this point: the def interferes with
        // everything live out of it, not only with the values that are
        // themselves live out of it *and* mentioned here.  Missing this let a
        // dead `second = 0` initialiser take the register still holding
        // `first` across a later suspend, so the await flush spilled the wrong
        // word.
        let def_over_live =
            |def: Temp, other: Temp| p.defs.contains(&def) && p.live_out.contains(&other);
        if def_over_live(left, right) || def_over_live(right, left) {
            return true;
        }
        let live = |set: &[Temp]| set.contains(&left) && set.contains(&right);
        live(&p.live_in) || live(&p.live_out)
    })
}

fn point_may_call(f: &FlowWirFn, state: usize, index: usize) -> bool {
    match &f.states[state].ops[index] {
        FlowInst::Mwir(inst) => matches!(
            inst,
            crate::mwir::Inst::Call { .. }
                | crate::mwir::Inst::FormatScalar { .. }
                | crate::mwir::Inst::StringConcat { .. }
                | crate::mwir::Inst::Entropy { .. }
        ),
        // Flow operations may invoke scheduler/runtime helpers.  Keep values
        // that cross them out of the initial state-local cache rather than
        // assuming a callee-save convention the probe has not proved.
        _ => true,
    }
}

fn live_across_call(f: &FlowWirFn, a: &FlowLiveness, state: usize, t: Temp) -> bool {
    a.points.iter().any(|p| {
        let PointKind::Op { state: s, index } = p.kind else {
            return false;
        };
        s == state
            && point_may_call(f, state, index)
            && p.live_in.contains(&t)
            && p.live_out.contains(&t)
    })
}

fn assign_state_registers(
    f: &FlowWirFn,
    a: &FlowLiveness,
    classes: &[StorageClass],
    homes: &[Home],
) -> Vec<StateAssignment> {
    let n = f.frame.temp_count();
    let sets = state_temp_sets(a, f.states.len());
    let mut assignments = Vec::with_capacity(f.states.len());
    for state in 0..f.states.len() {
        let candidates: Vec<Temp> = sets[state]
            .iter()
            .copied()
            .filter(|t| {
                classes[t.0] == StorageClass::StateLocal
                    || (classes[t.0] == StorageClass::Persistent
                        && matches!(homes[t.0], Home::Frame { .. }))
            })
            .filter(|t| t.0 < f.frame.temp_types.len())
            .filter(|t| used_in_state(a, state, *t))
            .filter(|t| !live_across_call(f, a, state, *t))
            .filter(|t| scalar_type(&f.frame.temp_types[t.0]))
            .collect();
        let mut regs = vec![None; n];
        // A physical register may be reused by several non-interfering temps.
        // Keep every previous occupant: checking only the most recently
        // assigned temp can put a later value in a register that still
        // interferes with an earlier occupant.
        let mut occupied: BTreeMap<u8, Vec<Temp>> = BTreeMap::new();
        for t in candidates {
            for &r in regalloc::POOL {
                let conflict = occupied.get(&r).is_some_and(|others| {
                    others
                        .iter()
                        .any(|other| same_state_interference(a, state, t, *other))
                });
                if !conflict {
                    regs[t.0] = Some(r);
                    occupied.entry(r).or_default().push(t);
                    break;
                }
            }
        }
        let entry = a.state_entries.get(state).copied();
        let live_in_loads = entry
            .map(|id| {
                a.points[id]
                    .live_in
                    .iter()
                    .copied()
                    .filter(|t| regs[t.0].is_some())
                    .filter(|t| matches!(homes[t.0], Home::Frame { .. } | Home::Pinned { .. }))
                    .collect()
            })
            .unwrap_or_default();
        let durable = |t: &Temp| matches!(homes[t.0], Home::Frame { .. } | Home::Pinned { .. });
        let await_flushes = a
            .suspends
            .iter()
            .find(|s| s.state == state)
            .map(|s| s.save.iter().filter(|t| durable(t)).copied().collect())
            .unwrap_or_default();
        let exit_flushes = a
            .points
            .iter()
            .find(|p| p.kind == PointKind::Transition { state })
            .map(|p| p.live_out.iter().filter(|t| durable(t)).copied().collect())
            .unwrap_or_default();
        assignments.push(StateAssignment {
            state,
            temp_regs: regs,
            live_in_loads,
            await_flushes,
            exit_flushes,
        });
    }
    assignments
}

/// Build a conservative first frame plan.  It intentionally does not alter
/// code generation; callers can validate and dump it before using it.
pub fn plan_flow(
    f: &FlowWirFn,
    analysis: &FlowLiveness,
    layout: &crate::mwir::LayoutCtx,
) -> Result<FramePlan, String> {
    if f.frame.temp_count() != f.frame.temp_types.len() {
        return Err("FlowWir frame temp count disagrees with its type vector".to_string());
    }
    let n = f.frame.temp_count();
    let pinned_set = pinned(f);
    let persistent_set = persistent(analysis);
    let boundary_set = boundary_live(f, analysis);
    let resume_set = resume_results(analysis);
    let escaped_set = escapes(f);
    let mut classes = vec![StorageClass::StateLocal; n];
    for i in 0..n {
        let t = Temp(i);
        classes[i] = if pinned_set.contains(&t) {
            StorageClass::Pinned
        } else if escaped_set.contains(&t) {
            StorageClass::Escaped
        } else if persistent_set.contains(&t) {
            StorageClass::Persistent
        } else if boundary_set.contains(&t) {
            StorageClass::BoundaryLive
        } else if resume_set.contains(&t) {
            StorageClass::ResumeResult
        } else {
            StorageClass::StateLocal
        };
    }

    let mut homes = vec![Home::None; n];
    let mut abi_offset = 0u32;
    for i in 0..n {
        if classes[i] != StorageClass::Pinned {
            continue;
        }
        let size = scalar_size(&f.frame.temp_types[i], layout)?;
        homes[i] = Home::Pinned { offset: abi_offset };
        abi_offset = align_up(abi_offset + size, 8);
    }

    let mut slots = Vec::new();
    let mut cursor = abi_offset;
    for i in 0..n {
        if classes[i] == StorageClass::StateLocal
            && scalar_size(&f.frame.temp_types[i], layout)? <= 8
        {
            continue;
        }
        let size = scalar_size(&f.frame.temp_types[i], layout)?.max(8);
        let alignment = 8u32;
        cursor = align_up(cursor, alignment);
        let id = slots.len();
        slots.push(FrameSlot {
            id,
            size,
            alignment,
            offset: cursor,
            occupants: vec![Temp(i)],
        });
        homes[i] = Home::Frame { slot: id };
        cursor = cursor
            .checked_add(size)
            .ok_or_else(|| "FlowWir frame offset overflow".to_string())?;
    }

    let mut states = assign_state_registers(f, analysis, &classes, &homes);
    // A state-local scalar that did not get a register is a state spill.  Give
    // it an ordinary durable slot so emission never has an unrepresented home.
    for state in &states {
        for (i, reg) in state.temp_regs.iter().enumerate() {
            if reg.is_none()
                && classes[i] == StorageClass::StateLocal
                && used_in_state(analysis, state.state, Temp(i))
            {
                // It is harmless for one temp to be visited by more than one
                // state; the first slot is reused in the metadata below.
                if matches!(homes[i], Home::None) {
                    classes[i] = StorageClass::StateSpill;
                    let size = scalar_size(&f.frame.temp_types[i], layout)?.max(8);
                    cursor = align_up(cursor, 8);
                    let id = slots.len();
                    slots.push(FrameSlot {
                        id,
                        size,
                        alignment: 8,
                        offset: cursor,
                        occupants: vec![Temp(i)],
                    });
                    homes[i] = Home::Frame { slot: id };
                    cursor = cursor
                        .checked_add(size)
                        .ok_or_else(|| "FlowWir frame offset overflow".to_string())?;
                }
            }
        }
    }
    // Re-run assignment so newly spilled values are no longer presented as
    // register-only state locals.
    states = assign_state_registers(f, analysis, &classes, &homes);

    let plan = FramePlan {
        classes,
        homes,
        states,
        slots,
        abi_prefix_bytes: abi_offset,
        frame_size: align_up(cursor, 16),
    };
    validate(f, analysis, &plan)
        .map(|()| plan)
        .map_err(|e| format!("invalid FlowWir frame plan: {e}"))
}

pub fn validate(f: &FlowWirFn, analysis: &FlowLiveness, plan: &FramePlan) -> Result<(), String> {
    if plan.homes.len() != f.frame.temp_count() || plan.classes.len() != plan.homes.len() {
        return Err("frame plan temp vectors have inconsistent lengths".to_string());
    }
    for t in analysis.suspends.iter().flat_map(|s| s.save.iter()) {
        if matches!(plan.homes[t.0], Home::None) {
            return Err(format!("persistent temp {t} has no durable home"));
        }
    }
    for (i, home) in plan.homes.iter().enumerate() {
        if let Home::Frame { slot } = home {
            let Some(s) = plan.slots.get(*slot) else {
                return Err(format!("temp t{i} refers to missing frame slot {slot}"));
            };
            if s.offset % s.alignment != 0 {
                return Err(format!("slot {slot} is not aligned"));
            }
        }
    }
    for p in &analysis.points {
        let state = match p.kind {
            PointKind::Op { state, .. }
            | PointKind::Transition { state }
            | PointKind::ResumeDef { state } => state,
        };
        for t in p.uses.iter().chain(p.defs.iter()) {
            let state_reg = plan
                .states
                .get(state)
                .and_then(|s| s.temp_regs.get(t.0))
                .copied()
                .flatten();
            if matches!(plan.homes[t.0], Home::None) && state_reg.is_none() {
                return Err(format!(
                    "used temp {t} has neither a home nor a state register"
                ));
            }
        }
        if let Some(assignment) = plan.states.get(state) {
            for (left, &left_reg) in assignment.temp_regs.iter().enumerate() {
                let Some(left_reg) = left_reg else { continue };
                for (right, &right_reg) in assignment.temp_regs.iter().enumerate().skip(left + 1) {
                    if right_reg == Some(left_reg)
                        && same_state_interference(analysis, state, Temp(left), Temp(right))
                    {
                        return Err(format!(
                            "state s{state} assigns interfering temps t{left} and t{right} to x{left_reg}"
                        ));
                    }
                }
            }
            for t in &assignment.live_in_loads {
                if assignment.temp_regs.get(t.0).copied().flatten().is_none() {
                    return Err(format!("state s{state} entry-loads unassigned temp {t}"));
                }
                if matches!(plan.homes[t.0], Home::None) {
                    return Err(format!("state s{state} entry-loads register-only temp {t}"));
                }
            }
        }
    }
    Ok(())
}

fn home_text(home: &Home) -> String {
    match home {
        Home::None => "none".to_string(),
        Home::Frame { slot } => format!("slot{slot}"),
        Home::Pinned { offset } => format!("pinned+{offset}"),
    }
}

/// Stable `frame` stage dump for FlowWir functions.
pub fn dump_program(
    program: &crate::flowwir::FlowWirProgram,
    layout: &crate::mwir::LayoutCtx,
) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("Frame\n");
    for (key, f) in &program.fns {
        let a = crate::flow_liveness::analyze(f)?;
        let p = plan_flow(f, &a, layout)?;
        let p = crate::frame_color::color_flow(f, &a, &p)?;
        let _ = writeln!(out, "  function {key} frame_size={}", p.frame_size);
        for i in 0..p.homes.len() {
            let states: Vec<String> = p
                .states
                .iter()
                .filter_map(|s| s.temp_regs[i].map(|r| format!("s{}:x{}", s.state, r)))
                .collect();
            let _ = writeln!(
                out,
                "    temp t{i} class={} home={} states=[{}]",
                p.classes[i].as_str(),
                home_text(&p.homes[i]),
                states.join(",")
            );
        }
        for s in &p.slots {
            let occupants = s
                .occupants
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let _ = writeln!(
                out,
                "    slot{} offset={} size={} align={} occupants=[{}]",
                s.id, s.offset, s.size, s.alignment, occupants
            );
        }
        for s in &p.states {
            let loads = s
                .live_in_loads
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let await_flushes = s
                .await_flushes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let flushes = s
                .exit_flushes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let _ = writeln!(
                out,
                "    state s{} entry_load=[{}] await_flush=[{}] exit_flush=[{}]",
                s.state, loads, await_flushes, flushes
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow_liveness;
    use crate::flowwir::{AwaitKind, FrameLayout, State};
    use crate::mwir::LayoutCtx;
    use crate::sema::types::Type;

    fn f() -> FlowWirFn {
        FlowWirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            frame: FrameLayout {
                temp_types: vec![Type::U64, Type::U64, Type::U64],
                lineage_group_slot: Temp(0),
                lineage_deadline_slot: Temp(1),
            },
            states: vec![
                State {
                    ops: vec![FlowInst::Mwir(crate::mwir::Inst::ConstInt {
                        dst: Temp(0),
                        ty: Type::U64,
                        value: 1,
                    })],
                    transition: Transition::Await {
                        what: AwaitKind::ActorCall {
                            target_temp: Temp(1),
                            method_key: "A.f".into(),
                            arg_temps: vec![Temp(0)],
                            take_arg_temps: vec![],
                        },
                        resume_state: 1,
                        result_temp: Temp(2),
                    },
                },
                State {
                    ops: vec![],
                    transition: Transition::Return(Some(Temp(0))),
                },
            ],
        }
    }

    #[test]
    fn every_suspend_live_temp_gets_a_home() {
        let f = f();
        let a = flow_liveness::analyze(&f).expect("analysis");
        let p = plan_flow(&f, &a, &LayoutCtx::default()).expect("plan");
        assert!(matches!(p.homes[0], Home::Frame { .. }));
        assert!(!matches!(p.homes[2], Home::None));
        validate(&f, &a, &p).expect("validation");
    }

    #[test]
    fn one_instruction_never_reuses_a_state_register_for_its_input_and_output() {
        let f = FlowWirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            frame: FrameLayout {
                temp_types: vec![Type::U64, Type::U64, Type::U64, Type::U64],
                lineage_group_slot: Temp(2),
                lineage_deadline_slot: Temp(3),
            },
            states: vec![State {
                ops: vec![
                    FlowInst::Mwir(crate::mwir::Inst::ConstInt {
                        dst: Temp(0),
                        ty: Type::U64,
                        value: 1,
                    }),
                    FlowInst::Mwir(crate::mwir::Inst::ArithWrapping {
                        dst: Temp(1),
                        op: crate::syntax::ast::BinOp::AddW,
                        ty: Type::U64,
                        lhs: Temp(0),
                        rhs: Temp(0),
                    }),
                ],
                transition: Transition::Return(Some(Temp(1))),
            }],
        };
        let analysis = flow_liveness::analyze(&f).expect("analysis");
        let plan = plan_flow(&f, &analysis, &LayoutCtx::default()).expect("plan");
        let input = plan.states[0].temp_regs[0].expect("input register");
        let output = plan.states[0].temp_regs[1].expect("output register");
        assert_ne!(
            input, output,
            "a general Flow operation may need its inputs after starting to define its output"
        );
    }

    #[test]
    fn a_dead_definition_does_not_take_the_register_of_a_value_live_across_the_suspend() {
        // t0 is written before the first await and read after the second one,
        // so state s1 must still hold it when the s1 suspend flushes it.  t1 is
        // a dead initialiser in s1 (s2 redefines it before any read), which is
        // exactly the `second: u64 = 0` line of a two-await body.  A dead def
        // is still a register write, so it must not be handed t0's register.
        let await_to = |resume_state, result_temp| Transition::Await {
            what: AwaitKind::ActorCall {
                target_temp: Temp(4),
                method_key: "A.f".into(),
                arg_temps: vec![],
                take_arg_temps: vec![],
            },
            resume_state,
            result_temp,
        };
        let const_int = |dst, value| {
            FlowInst::Mwir(crate::mwir::Inst::ConstInt {
                dst,
                ty: Type::U64,
                value,
            })
        };
        let copy = |dst, src| FlowInst::Mwir(crate::mwir::Inst::Copy { dst, src });
        let f = FlowWirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            frame: FrameLayout {
                temp_types: vec![Type::U64; 9],
                lineage_group_slot: Temp(7),
                lineage_deadline_slot: Temp(8),
            },
            states: vec![
                State {
                    ops: vec![const_int(Temp(4), 0), const_int(Temp(0), 1)],
                    transition: await_to(1, Temp(5)),
                },
                State {
                    ops: vec![copy(Temp(2), Temp(0)), const_int(Temp(1), 0)],
                    transition: await_to(2, Temp(6)),
                },
                State {
                    ops: vec![const_int(Temp(1), 7), copy(Temp(3), Temp(0))],
                    transition: Transition::Return(Some(Temp(3))),
                },
            ],
        };
        let analysis = flow_liveness::analyze(&f).expect("analysis");
        let plan = plan_flow(&f, &analysis, &LayoutCtx::default()).expect("plan");
        let live = plan.states[1].temp_regs[0].expect("the persistent value is cached in s1");
        assert!(
            plan.states[1].await_flushes.contains(&Temp(0)),
            "s1 must flush the persistent value at its suspend"
        );
        assert_ne!(
            plan.states[1].temp_regs[1],
            Some(live),
            "a dead def in s1 took x{live}, the register the s1 suspend flushes as t0"
        );
        validate(&f, &analysis, &plan).expect("validation");
    }
}
