use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::mwir::{self, Temp};
use crate::sema::types::{self, Type};
use crate::syntax::ast::AccessMode;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlowWirProgram {
    pub fns: BTreeMap<String, FlowWirFn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlowWirFn {
    pub receiver: Option<(Temp, AccessMode)>,
    pub params: Vec<(Temp, AccessMode)>,
    pub ret: Type,
    pub frame: FrameLayout,
    pub states: Vec<State>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameLayout {
    pub temp_types: Vec<Type>,
    pub lineage_group_slot: Temp,
    pub lineage_deadline_slot: Temp,
}

impl FrameLayout {
    pub fn temp_count(&self) -> usize {
        self.temp_types.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub ops: Vec<FlowInst>,
    pub transition: Transition,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FlowInst {
    Mwir(mwir::Inst),
    SelfPath {
        dst: Temp,
        path: Vec<String>,
    },
    Now {
        dst: Temp,
    },
    Entropy {
        dst: Temp,
        n: u64,
    },
    Duration {
        dst: Temp,
        n: Temp,
    },
    Send {
        dst: Temp,
        target: Temp,
        method_key: String,
        arg_temps: Vec<Temp>,
        take_arg_temps: Vec<Temp>,
    },
    GroupCreate {
        group_temp: Temp,
        capacity: Option<Temp>,
        deadline: Option<Temp>,
    },
    GroupStart {
        group_temp: Temp,
        callee_key: String,
        arg_temps: Vec<Temp>,
    },
    GroupClose {
        group_temp: Temp,
        cleanup_states: Vec<usize>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transition {
    Return(Option<Temp>),
    Await {
        what: AwaitKind,
        resume_state: usize,
        result_temp: Temp,
    },
    Jump(usize),
    Branch {
        cond_temp: Temp,
        then_state: usize,
        else_state: usize,
    },
    Abort {
        msg: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AwaitKind {
    ActorCall {
        target_temp: Temp,
        method_key: String,
        arg_temps: Vec<Temp>,
        take_arg_temps: Vec<Temp>,
    },
    GroupJoin {
        group_temp: Temp,
        child_count: usize,
    },
    Receipt {
        receipt_temp: Temp,
    },
}

pub fn dump(program: &FlowWirProgram) -> String {
    let mut out = String::new();
    out.push_str("Program\n");
    for (key, f) in &program.fns {
        let mut header = format!(
            "Fn key={key} ret={} states={} frame={}",
            types::render_type(&f.ret),
            f.states.len(),
            f.frame.temp_count()
        );
        let _ = write!(
            header,
            " lineage=[group={},deadline={}]",
            f.frame.lineage_group_slot, f.frame.lineage_deadline_slot
        );
        if let Some((t, mode)) = &f.receiver {
            let _ = write!(header, " receiver={t}:{}", mode.as_str());
        }
        if !f.params.is_empty() {
            let ps: Vec<String> = f
                .params
                .iter()
                .map(|(t, mode)| {
                    if *mode == AccessMode::Read {
                        t.to_string()
                    } else {
                        format!("{t}:{}", mode.as_str())
                    }
                })
                .collect();
            let _ = write!(header, " params=[{}]", ps.join(","));
        }
        push_line(&mut out, 1, &header);
        for (i, ty) in f.frame.temp_types.iter().enumerate() {
            push_line(
                &mut out,
                2,
                &format!("Temp t{i} ty={}", types::render_type(ty)),
            );
        }
        for (i, state) in f.states.iter().enumerate() {
            push_line(&mut out, 2, &format!("State {i}"));
            for (j, op) in state.ops.iter().enumerate() {
                let line = format!("{j:04}: {}", fmt_flow_inst(op));
                push_line(&mut out, 3, &line);
            }
            push_line(
                &mut out,
                3,
                &format!("Transition {}", fmt_transition(&state.transition)),
            );
        }
    }
    out
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

fn join_temps(ts: &[Temp]) -> String {
    ts.iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn fmt_flow_inst(op: &FlowInst) -> String {
    match op {
        FlowInst::Mwir(inst) => format!("Mwir {}", mwir::fmt_inst(inst)),
        FlowInst::SelfPath { dst, path } => {
            format!("SelfPath dst={dst} path=self.{}", path.join("."))
        }
        FlowInst::Now { dst } => format!("Now dst={dst}"),
        FlowInst::Entropy { dst, n } => format!("Entropy dst={dst} n={n}"),
        FlowInst::Duration { dst, n } => format!("Duration dst={dst} n={n}"),
        FlowInst::Send {
            dst,
            target,
            method_key,
            arg_temps,
            take_arg_temps,
        } => {
            if take_arg_temps.is_empty() {
                format!(
                    "Send dst={dst} target={target} method={method_key} args=[{}]",
                    join_temps(arg_temps)
                )
            } else {
                format!(
                    "Send dst={dst} target={target} method={method_key} args=[{}] take=[{}]",
                    join_temps(arg_temps),
                    join_temps(take_arg_temps)
                )
            }
        }
        FlowInst::GroupCreate {
            group_temp,
            capacity,
            deadline,
        } => {
            let mut s = format!("GroupCreate group={group_temp}");
            if let Some(c) = capacity {
                let _ = write!(s, " capacity={c}");
            }
            if let Some(d) = deadline {
                let _ = write!(s, " deadline={d}");
            }
            s
        }
        FlowInst::GroupStart {
            group_temp,
            callee_key,
            arg_temps,
        } => format!(
            "GroupStart group={group_temp} callee={callee_key} args=[{}]",
            join_temps(arg_temps)
        ),
        FlowInst::GroupClose {
            group_temp,
            cleanup_states,
        } => {
            let cs: Vec<String> = cleanup_states.iter().map(|i| i.to_string()).collect();
            format!("GroupClose group={group_temp} cleanup=[{}]", cs.join(","))
        }
    }
}

fn fmt_transition(t: &Transition) -> String {
    match t {
        Transition::Return(value) => match value {
            Some(v) => format!("Return value={v}"),
            None => "Return".to_string(),
        },
        Transition::Await {
            what,
            resume_state,
            result_temp,
        } => format!(
            "Await what={} resume={resume_state} result={result_temp}",
            fmt_await_kind(what)
        ),
        Transition::Jump(target) => format!("Jump target={target}"),
        Transition::Branch {
            cond_temp,
            then_state,
            else_state,
        } => format!("Branch cond={cond_temp} then={then_state} else={else_state}"),
        Transition::Abort { msg } => format!("Abort msg={msg:?}"),
    }
}

fn fmt_await_kind(k: &AwaitKind) -> String {
    match k {
        AwaitKind::ActorCall {
            target_temp,
            method_key,
            arg_temps,
            take_arg_temps,
        } => {
            if take_arg_temps.is_empty() {
                format!(
                    "ActorCall{{target={target_temp},method={method_key},args=[{}]}}",
                    join_temps(arg_temps)
                )
            } else {
                format!(
                    "ActorCall{{target={target_temp},method={method_key},args=[{}],take=[{}]}}",
                    join_temps(arg_temps),
                    join_temps(take_arg_temps)
                )
            }
        }
        AwaitKind::GroupJoin {
            group_temp,
            child_count,
        } => format!("GroupJoin{{group={group_temp},children={child_count}}}"),
        AwaitKind::Receipt { receipt_temp } => {
            format!("Receipt{{receipt={receipt_temp}}}")
        }
    }
}
