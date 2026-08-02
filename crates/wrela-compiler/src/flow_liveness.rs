//! FlowWir control flow and liveness across suspension points.
//!
//! Flow states are kept as first-class source identities.  The analysis uses a
//! node for every local operation plus transition and resume-definition nodes;
//! this is a little less clever than flattening the machine and, importantly,
//! means an await result is defined exactly where the runtime delivers it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::flowwir::{AwaitKind, FlowInst, FlowWirFn, FlowWirProgram, Transition};
use crate::mwir::{Inst, Temp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PointKind {
    Op { state: usize, index: usize },
    Transition { state: usize },
    ResumeDef { state: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowPoint {
    pub id: usize,
    pub kind: PointKind,
    pub uses: Vec<Temp>,
    pub defs: Vec<Temp>,
    pub successors: Vec<usize>,
    pub predecessors: Vec<usize>,
    pub live_in: Vec<Temp>,
    pub live_out: Vec<Temp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowBlock {
    pub state: usize,
    pub id: usize,
    pub range: std::ops::Range<usize>,
    pub successors: Vec<usize>,
    pub predecessors: Vec<usize>,
    pub use_set: Vec<Temp>,
    pub def_set: Vec<Temp>,
    pub live_in: Vec<Temp>,
    pub live_out: Vec<Temp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuspendLive {
    pub state: usize,
    pub resume_state: usize,
    pub result_temp: Temp,
    pub request_uses: Vec<Temp>,
    pub save: Vec<Temp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowLiveness {
    pub points: Vec<FlowPoint>,
    pub blocks: Vec<Vec<FlowBlock>>,
    pub state_entries: Vec<usize>,
    pub resume_defs: BTreeMap<usize, usize>,
    pub suspends: Vec<SuspendLive>,
}

fn sorted(values: impl IntoIterator<Item = Temp>) -> Vec<Temp> {
    let mut out: Vec<Temp> = values.into_iter().collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn transition_facts(t: &Transition) -> (Vec<Temp>, Vec<Temp>) {
    match t {
        Transition::Return(v) => (v.iter().copied().collect(), Vec::new()),
        Transition::Await { what, .. } => (await_uses(what), Vec::new()),
        Transition::Jump(_) | Transition::Abort { .. } => (Vec::new(), Vec::new()),
        Transition::Branch { cond_temp, .. } => (vec![*cond_temp], Vec::new()),
    }
}

fn await_uses(what: &AwaitKind) -> Vec<Temp> {
    match what {
        AwaitKind::ActorCall {
            target_temp,
            arg_temps,
            take_arg_temps,
            ..
        } => std::iter::once(*target_temp)
            .chain(arg_temps.iter().copied())
            .chain(take_arg_temps.iter().copied())
            .collect(),
        AwaitKind::GroupJoin { group_temp, .. } => vec![*group_temp],
        AwaitKind::Receipt { receipt_temp } => vec![*receipt_temp],
    }
}

fn flow_inst_facts(op: &FlowInst) -> (Vec<Temp>, Vec<Temp>) {
    match op {
        FlowInst::Mwir(inst) => {
            let f = crate::mwir_facts::inst_facts(inst);
            (f.uses, f.defs)
        }
        FlowInst::SelfPath { dst, .. } | FlowInst::Now { dst } | FlowInst::Entropy { dst, .. } => {
            (Vec::new(), vec![*dst])
        }
        FlowInst::Duration { dst, n } => (vec![*n], vec![*dst]),
        FlowInst::Send {
            dst,
            target,
            arg_temps,
            take_arg_temps,
            ..
        } => (
            std::iter::once(*target)
                .chain(arg_temps.iter().copied())
                .chain(take_arg_temps.iter().copied())
                .collect(),
            vec![*dst],
        ),
        FlowInst::GroupCreate {
            group_temp,
            capacity,
            deadline,
        } => (
            capacity.iter().chain(deadline).copied().collect(),
            vec![*group_temp],
        ),
        FlowInst::GroupStart {
            group_temp,
            arg_temps,
            ..
        } => (
            std::iter::once(*group_temp)
                .chain(arg_temps.iter().copied())
                .collect(),
            Vec::new(),
        ),
        FlowInst::GroupClose { group_temp, .. } => (vec![*group_temp], Vec::new()),
    }
}

fn local_target(op: &FlowInst) -> Option<usize> {
    match op {
        FlowInst::Mwir(Inst::Jump { target })
        | FlowInst::Mwir(Inst::JumpIfFalse { target, .. }) => Some(*target),
        _ => None,
    }
}

fn local_terminal(op: &FlowInst) -> bool {
    matches!(
        op,
        FlowInst::Mwir(Inst::Jump { .. })
            | FlowInst::Mwir(Inst::Return { .. })
            | FlowInst::Mwir(Inst::Abort { .. })
            | FlowInst::Mwir(Inst::AssertFail { .. })
    )
}

fn local_conditional(op: &FlowInst) -> bool {
    matches!(op, FlowInst::Mwir(Inst::JumpIfFalse { .. }))
}

fn leaders(ops: &[FlowInst]) -> Result<Vec<usize>, String> {
    let n = ops.len();
    let mut starts = BTreeSet::new();
    if n > 0 {
        starts.insert(0);
    }
    for (i, op) in ops.iter().enumerate() {
        if let Some(t) = local_target(op) {
            if t > n {
                return Err(format!(
                    "invalid FlowWir local jump target {t} at operation {i}; state has {n} operations"
                ));
            }
            if t < n {
                starts.insert(t);
            }
        }
        if local_terminal(op) || local_conditional(op) {
            if i + 1 < n {
                starts.insert(i + 1);
            }
        }
    }
    Ok(starts.into_iter().collect())
}

fn entry_for(state_entries: &[usize], state: usize) -> Result<usize, String> {
    state_entries
        .get(state)
        .copied()
        .ok_or_else(|| format!("invalid FlowWir state {state}"))
}

fn validate_temp(t: Temp, n: usize, context: &str) -> Result<(), String> {
    if t.0 >= n {
        Err(format!(
            "FlowWir temp {t} in {context} is outside frame temp count {n}"
        ))
    } else {
        Ok(())
    }
}

/// Build and solve FlowWir liveness.  Malformed states, jumps, and resume
/// values are errors rather than best-effort facts.
pub fn analyze(f: &FlowWirFn) -> Result<FlowLiveness, String> {
    let temp_count = f.frame.temp_count();
    let mut points = Vec::new();
    let mut op_point: Vec<Vec<usize>> = Vec::with_capacity(f.states.len());
    let mut transition_point = Vec::with_capacity(f.states.len());
    let mut local_blocks = Vec::with_capacity(f.states.len());
    let mut local_block_of: Vec<Vec<usize>> = Vec::with_capacity(f.states.len());

    for (state, s) in f.states.iter().enumerate() {
        let starts = leaders(&s.ops)?;
        let mut blocks = Vec::with_capacity(starts.len());
        let mut by_start = BTreeMap::new();
        let mut block_of = vec![0usize; s.ops.len()];
        for (id, &start) in starts.iter().enumerate() {
            let end = starts.get(id + 1).copied().unwrap_or(s.ops.len());
            by_start.insert(start, id);
            for x in &mut block_of[start..end] {
                *x = id;
            }
            blocks.push(FlowBlock {
                state,
                id,
                range: start..end,
                successors: Vec::new(),
                predecessors: Vec::new(),
                use_set: Vec::new(),
                def_set: Vec::new(),
                live_in: Vec::new(),
                live_out: Vec::new(),
            });
        }

        let mut op_ids = Vec::with_capacity(s.ops.len());
        for (index, op) in s.ops.iter().enumerate() {
            let (uses, defs) = flow_inst_facts(op);
            for t in uses.iter().chain(defs.iter()) {
                validate_temp(*t, temp_count, &format!("s{state}.b? op {index}"))?;
            }
            let id = points.len();
            points.push(FlowPoint {
                id,
                kind: PointKind::Op { state, index },
                uses: sorted(uses),
                defs: sorted(defs),
                successors: Vec::new(),
                predecessors: Vec::new(),
                live_in: Vec::new(),
                live_out: Vec::new(),
            });
            op_ids.push(id);
        }
        op_point.push(op_ids);

        let (uses, defs) = transition_facts(&s.transition);
        for t in uses.iter().chain(defs.iter()) {
            validate_temp(*t, temp_count, &format!("s{state} transition"))?;
        }
        let tid = points.len();
        points.push(FlowPoint {
            id: tid,
            kind: PointKind::Transition { state },
            uses: sorted(uses),
            defs: sorted(defs),
            successors: Vec::new(),
            predecessors: Vec::new(),
            live_in: Vec::new(),
            live_out: Vec::new(),
        });
        transition_point.push(tid);

        for block in &mut blocks {
            let mut uses = BTreeSet::new();
            let mut defs = BTreeSet::new();
            for i in block.range.clone() {
                let p = &points[op_point[state][i]];
                for t in &p.uses {
                    if !defs.contains(t) {
                        uses.insert(*t);
                    }
                }
                defs.extend(p.defs.iter().copied());
            }
            block.use_set = uses.into_iter().collect();
            block.def_set = defs.into_iter().collect();

            if block.range.is_empty() {
                continue;
            }
            let last = block.range.end - 1;
            let last_id = op_point[state][last];
            let mut succ = BTreeSet::new();
            match &s.ops[last] {
                FlowInst::Mwir(Inst::Jump { target }) => {
                    if *target < s.ops.len() {
                        succ.insert(*target);
                    } else {
                        // The transition point is represented below; a local
                        // jump to state-end has the transition as successor.
                        points[last_id].successors.push(tid);
                    }
                }
                FlowInst::Mwir(Inst::JumpIfFalse { target, .. }) => {
                    if *target < s.ops.len() {
                        succ.insert(*target);
                    } else {
                        points[last_id].successors.push(tid);
                    }
                    if last + 1 < s.ops.len() {
                        succ.insert(last + 1);
                    } else {
                        points[last_id].successors.push(tid);
                    }
                }
                op if local_terminal(op) => {}
                _ => {
                    if last + 1 < s.ops.len() {
                        succ.insert(last + 1);
                    } else {
                        points[last_id].successors.push(tid);
                    }
                }
            }
            for index in succ {
                points[last_id].successors.push(op_point[state][index]);
            }
            points[last_id].successors.sort_unstable();
            points[last_id].successors.dedup();
        }
        // Within a block, local operations fall through unless the operation
        // already supplied its own successors.
        for block in &blocks {
            for pair in block.range.clone().collect::<Vec<_>>().windows(2) {
                let from = op_point[state][pair[0]];
                if points[from].successors.is_empty() {
                    points[from].successors.push(op_point[state][pair[1]]);
                }
            }
        }
        // Convert local successor indices to block successor IDs for the
        // stable block dump after all operation edges are known.
        for block in &mut blocks {
            if block.range.is_empty() {
                continue;
            }
            let last_id = op_point[state][block.range.end - 1];
            let mut bs = BTreeSet::new();
            for &p in &points[last_id].successors {
                match points[p].kind {
                    PointKind::Op { state: st, index } if st == state => {
                        bs.insert(block_of[index]);
                    }
                    _ => {}
                }
            }
            block.successors = bs.into_iter().collect();
        }
        for (from, block) in blocks.clone().iter().enumerate() {
            for &to in &block.successors {
                blocks[to].predecessors.push(from);
            }
        }
        for block in &mut blocks {
            block.predecessors.sort_unstable();
            block.predecessors.dedup();
        }
        local_block_of.push(block_of);
        local_blocks.push(blocks);
    }

    let state_entries: Vec<usize> = f
        .states
        .iter()
        .enumerate()
        .map(|(state, _s)| {
            op_point[state]
                .first()
                .copied()
                .unwrap_or(transition_point[state])
        })
        .collect();

    // Resume definitions are keyed by their target state.  A state with two
    // incoming awaits must agree on the delivered temp; otherwise the IR is
    // ambiguous and we fail closed.
    let mut resume_defs = BTreeMap::new();
    let mut resume_temps = BTreeMap::<usize, Temp>::new();
    for (state, s) in f.states.iter().enumerate() {
        let Transition::Await {
            resume_state,
            result_temp,
            ..
        } = &s.transition
        else {
            continue;
        };
        if *resume_state >= f.states.len() {
            return Err(format!(
                "state s{state} resumes malformed state s{resume_state}"
            ));
        }
        validate_temp(*result_temp, temp_count, &format!("s{state} await result"))?;
        if let Some(old) = resume_temps.insert(*resume_state, *result_temp) {
            if old != *result_temp {
                return Err(format!(
                    "state s{resume_state} has await results {old} and {result_temp}; resume definition is ambiguous"
                ));
            }
        }
    }
    for (&state, &result_temp) in &resume_temps {
        let id = points.len();
        points.push(FlowPoint {
            id,
            kind: PointKind::ResumeDef { state },
            uses: Vec::new(),
            defs: vec![result_temp],
            successors: vec![entry_for(&state_entries, state)?],
            predecessors: Vec::new(),
            live_in: Vec::new(),
            live_out: Vec::new(),
        });
        resume_defs.insert(state, id);
    }

    for (state, s) in f.states.iter().enumerate() {
        let tid = transition_point[state];
        match &s.transition {
            Transition::Return(_) | Transition::Abort { .. } => {}
            Transition::Jump(target) => {
                points[tid]
                    .successors
                    .push(entry_for(&state_entries, *target)?);
            }
            Transition::Branch {
                then_state,
                else_state,
                ..
            } => {
                points[tid]
                    .successors
                    .push(entry_for(&state_entries, *then_state)?);
                points[tid]
                    .successors
                    .push(entry_for(&state_entries, *else_state)?);
            }
            Transition::Await { resume_state, .. } => {
                points[tid]
                    .successors
                    .push(*resume_defs.get(resume_state).ok_or_else(|| {
                        format!("malformed FlowWir: missing resume definition for s{resume_state}")
                    })?);
            }
        }
        points[tid].successors.sort_unstable();
        points[tid].successors.dedup();
    }

    let edges: Vec<Vec<usize>> = points.iter().map(|p| p.successors.clone()).collect();
    for (from, succs) in edges.iter().enumerate() {
        for &to in succs {
            points[to].predecessors.push(from);
        }
    }
    for point in &mut points {
        point.predecessors.sort_unstable();
        point.predecessors.dedup();
    }

    loop {
        let mut changed = false;
        for id in (0..points.len()).rev() {
            let mut out = BTreeSet::new();
            for &succ in &points[id].successors {
                out.extend(points[succ].live_in.iter().copied());
            }
            let defs: BTreeSet<Temp> = points[id].defs.iter().copied().collect();
            let mut input: BTreeSet<Temp> = points[id].uses.iter().copied().collect();
            input.extend(out.iter().copied().filter(|t| !defs.contains(t)));
            let new_in: Vec<Temp> = input.into_iter().collect();
            let new_out: Vec<Temp> = out.into_iter().collect();
            if points[id].live_in != new_in || points[id].live_out != new_out {
                points[id].live_in = new_in;
                points[id].live_out = new_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for blocks in &mut local_blocks {
        for block in blocks {
            if block.range.is_empty() {
                continue;
            }
            let first = match state_entries[block.state] {
                _ => {
                    // The first operation of this block is recovered from the
                    // range and the per-state operation IDs below.
                    let state = block.state;
                    op_point[state][block.range.start]
                }
            };
            let last = op_point[block.state][block.range.end - 1];
            block.live_in = points[first].live_in.clone();
            block.live_out = points[last].live_out.clone();
        }
    }

    let mut suspends = Vec::new();
    for (state, s) in f.states.iter().enumerate() {
        let Transition::Await {
            resume_state,
            result_temp,
            what,
            ..
        } = &s.transition
        else {
            continue;
        };
        let save = points[transition_point[state]].live_out.clone();
        if save.contains(result_temp) {
            return Err(format!(
                "await s{state} suspend-live set incorrectly contains its resume result {result_temp}"
            ));
        }
        suspends.push(SuspendLive {
            state,
            resume_state: *resume_state,
            result_temp: *result_temp,
            request_uses: sorted(await_uses(what)),
            save,
        });
    }

    Ok(FlowLiveness {
        points,
        blocks: local_blocks,
        state_entries,
        resume_defs,
        suspends,
    })
}

pub fn analyze_program(program: &FlowWirProgram) -> Result<BTreeMap<String, FlowLiveness>, String> {
    program
        .fns
        .iter()
        .map(|(key, f)| Ok((key.clone(), analyze(f)?)))
        .collect()
}

fn temps(values: &[Temp]) -> String {
    let mut out = String::from("[");
    for (i, t) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{t}");
    }
    out.push(']');
    out
}

fn point_succ(points: &[usize]) -> String {
    let mut out = String::from("[");
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "p{p}");
    }
    out.push(']');
    out
}

/// The FlowWir part of the `cfg` dump.
pub fn dump_program(program: &FlowWirProgram) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("FlowCFG\n");
    for (key, f) in &program.fns {
        let a = analyze(f)?;
        let _ = writeln!(out, "  function {key}");
        for (state, blocks) in a.blocks.iter().enumerate() {
            for b in blocks {
                let _ = writeln!(
                    out,
                    "    s{state}.b{} range=[{}, {}) succ={} pred={} use={} def={} live_in={} live_out={}",
                    b.id,
                    b.range.start,
                    b.range.end,
                    point_succ(&b.successors),
                    point_succ(&b.predecessors),
                    temps(&b.use_set),
                    temps(&b.def_set),
                    temps(&b.live_in),
                    temps(&b.live_out),
                );
                for i in b.range.clone() {
                    let id = a
                        .points
                        .iter()
                        .find(|p| p.kind == PointKind::Op { state, index: i })
                        .map(|p| p.id)
                        .ok_or_else(|| "malformed FlowWir: missing operation point".to_string())?;
                    let p = &a.points[id];
                    let _ = writeln!(
                        out,
                        "      s{state}.b{}/i{i:04} uses={} defs={} live_in={} live_out={} succ={}",
                        b.id,
                        temps(&p.uses),
                        temps(&p.defs),
                        temps(&p.live_in),
                        temps(&p.live_out),
                        point_succ(&p.successors),
                    );
                }
            }
            let tid = a
                .points
                .iter()
                .find(|p| p.kind == PointKind::Transition { state });
            if let Some(p) = tid {
                let label = match &f.states[state].transition {
                    Transition::Await {
                        resume_state,
                        result_temp,
                        ..
                    } => format!(
                        "s{state}.await succ={} uses={} live_out={} result={result_temp} resume=s{resume_state}",
                        point_succ(&p.successors),
                        temps(&p.uses),
                        temps(&p.live_out)
                    ),
                    _ => format!(
                        "s{state}.transition succ={} uses={} live_out={}",
                        point_succ(&p.successors),
                        temps(&p.uses),
                        temps(&p.live_out)
                    ),
                };
                let _ = writeln!(out, "    {label}");
            }
        }
        for (&state, &id) in &a.resume_defs {
            let p = &a.points[id];
            let _ = writeln!(
                out,
                "    s{state}.resume_def succ={} defs={} live_in={} live_out={}",
                point_succ(&p.successors),
                temps(&p.defs),
                temps(&p.live_in),
                temps(&p.live_out),
            );
        }
        for s in &a.suspends {
            let _ = writeln!(
                out,
                "    suspend s{} -> s{} save={} result={}",
                s.state,
                s.resume_state,
                temps(&s.save),
                s.result_temp
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flowwir::{AwaitKind, FrameLayout, State};
    use crate::sema::types::Type;
    use crate::syntax::ast::AccessMode;

    fn flow() -> FlowWirFn {
        FlowWirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            frame: FrameLayout {
                temp_types: vec![Type::U64, Type::U64, Type::U64, Type::U64],
                lineage_group_slot: Temp(0),
                lineage_deadline_slot: Temp(1),
            },
            states: vec![
                State {
                    ops: vec![FlowInst::Mwir(Inst::ConstInt {
                        dst: Temp(0),
                        ty: Type::U64,
                        value: 40,
                    })],
                    transition: Transition::Await {
                        what: AwaitKind::ActorCall {
                            target_temp: Temp(1),
                            method_key: "A.f".to_string(),
                            arg_temps: vec![Temp(0)],
                            take_arg_temps: Vec::new(),
                        },
                        resume_state: 1,
                        result_temp: Temp(2),
                    },
                },
                State {
                    ops: vec![FlowInst::Mwir(Inst::ArithWrapping {
                        dst: Temp(3),
                        op: crate::syntax::ast::BinOp::Add,
                        ty: Type::U64,
                        lhs: Temp(0),
                        rhs: Temp(2),
                    })],
                    transition: Transition::Return(Some(Temp(3))),
                },
            ],
        }
    }

    #[test]
    fn request_only_target_is_not_saved_and_result_is_defined_on_resume() {
        let a = analyze(&flow()).expect("flow liveness");
        assert_eq!(a.suspends[0].request_uses, vec![Temp(0), Temp(1)]);
        assert_eq!(a.suspends[0].save, vec![Temp(0)]);
        assert!(!a.suspends[0].save.contains(&Temp(2)));
        let resume = &a.points[a.resume_defs[&1]];
        assert_eq!(resume.defs, vec![Temp(2)]);
        assert!(resume.live_out.contains(&Temp(2)));
    }

    #[test]
    fn malformed_resume_state_and_result_fail_closed() {
        let mut f = flow();
        if let Transition::Await { resume_state, .. } = &mut f.states[0].transition {
            *resume_state = 99;
        }
        assert!(analyze(&f).is_err());
        let mut f = flow();
        if let Transition::Await { result_temp, .. } = &mut f.states[0].transition {
            *result_temp = Temp(99);
        }
        assert!(analyze(&f).is_err());
    }

    #[test]
    fn unused_taken_operands_are_not_durable() {
        let mut f = flow();
        if let Transition::Await { what, .. } = &mut f.states[0].transition {
            *what = AwaitKind::ActorCall {
                target_temp: Temp(1),
                method_key: "A.f".to_string(),
                arg_temps: Vec::new(),
                take_arg_temps: vec![Temp(3)],
            };
        }
        let a = analyze(&f).expect("flow liveness");
        assert!(!a.suspends[0].save.contains(&Temp(3)));
    }

    #[test]
    fn access_mode_import_is_kept_for_future_state_assignment_tests() {
        let _ = AccessMode::Read;
    }
}
