//! Ordinary MWIR liveness and its stable textual representation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::cfg::{self, BlockId, Cfg};
use crate::mwir::{MwirFn, MwirProgram, Temp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstLiveness {
    pub before: Vec<Temp>,
    pub after: Vec<Temp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Liveness {
    pub cfg: Cfg,
    pub live_in: Vec<Vec<Temp>>,
    pub live_out: Vec<Vec<Temp>>,
    pub instructions: Vec<InstLiveness>,
}

fn set(values: impl IntoIterator<Item = Temp>) -> BTreeSet<Temp> {
    values.into_iter().collect()
}

fn vec_set(values: BTreeSet<Temp>) -> Vec<Temp> {
    values.into_iter().collect()
}

pub fn analyze(f: &MwirFn) -> Result<Liveness, String> {
    let cfg = cfg::build_cfg(f)?;
    let mut live_in = vec![Vec::new(); cfg.blocks.len()];
    let mut live_out = vec![Vec::new(); cfg.blocks.len()];

    loop {
        let mut changed = false;
        for b in (0..cfg.blocks.len()).rev() {
            let mut out = BTreeSet::new();
            for &succ in &cfg.blocks[b].successors {
                out.extend(live_in[succ].iter().copied());
            }
            let mut input = set(cfg.blocks[b].use_set.iter().copied());
            let defs = set(cfg.blocks[b].def_set.iter().copied());
            input.extend(out.iter().copied().filter(|t| !defs.contains(t)));
            let new_out = vec_set(out);
            let new_in = vec_set(input);
            if live_out[b] != new_out || live_in[b] != new_in {
                live_out[b] = new_out;
                live_in[b] = new_in;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut instructions = vec![
        InstLiveness {
            before: Vec::new(),
            after: Vec::new(),
        };
        f.body.len()
    ];
    for block in &cfg.blocks {
        let mut current = set(live_out[block.id].iter().copied());
        for i in (block.range.start..block.range.end).rev() {
            instructions[i].after = vec_set(current.clone());
            let facts = crate::mwir_facts::inst_facts(&f.body[i]);
            let defs = set(facts.defs.iter().copied());
            let mut before = set(facts.uses.iter().copied());
            before.extend(current.iter().copied().filter(|t| !defs.contains(t)));
            instructions[i].before = vec_set(before.clone());
            current = before;
        }
    }

    Ok(Liveness {
        cfg,
        live_in,
        live_out,
        instructions,
    })
}

pub fn analyze_program(program: &MwirProgram) -> Result<BTreeMap<String, Liveness>, String> {
    program
        .fns
        .iter()
        .map(|(key, f)| Ok((key.clone(), analyze(f)?)))
        .collect()
}

fn temps(values: &[Temp]) -> String {
    let mut s = String::from("[");
    for (i, t) in values.iter().enumerate() {
        if i != 0 {
            s.push(',');
        }
        let _ = write!(s, "{t}");
    }
    s.push(']');
    s
}

fn blocks(values: &[BlockId]) -> String {
    let mut s = String::from("[");
    for (i, b) in values.iter().enumerate() {
        if i != 0 {
            s.push(',');
        }
        let _ = write!(s, "b{b}");
    }
    s.push(']');
    s
}

/// Dump all synchronous CFG and liveness facts in stable function/instruction
/// order.  The dump intentionally includes empty sets.
pub fn dump_program(program: &MwirProgram) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("CFG\n");
    for (key, f) in &program.fns {
        let analysis = analyze(f)?;
        let _ = writeln!(out, "  function {key}");
        for block in &analysis.cfg.blocks {
            let _ = writeln!(
                out,
                "    b{} range=[{}, {}) succ={} pred={} use={} def={} live_in={} live_out={}",
                block.id,
                block.range.start,
                block.range.end,
                blocks(&block.successors),
                blocks(&block.predecessors),
                temps(&block.use_set),
                temps(&block.def_set),
                temps(&analysis.live_in[block.id]),
                temps(&analysis.live_out[block.id]),
            );
            for i in block.range.clone() {
                let _ = writeln!(
                    out,
                    "      i{i:04} before={} after={}",
                    temps(&analysis.instructions[i].before),
                    temps(&analysis.instructions[i].after),
                );
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::BlockId;
    use crate::mwir::Inst;
    use crate::sema::types::Type;

    fn diamond() -> MwirFn {
        MwirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            temp_types: vec![Type::Bool, Type::U64],
            body: vec![
                Inst::ConstBool {
                    dst: Temp(0),
                    value: false,
                },
                Inst::JumpIfFalse {
                    cond: Temp(0),
                    target: 4,
                },
                Inst::ConstInt {
                    dst: Temp(1),
                    ty: Type::U64,
                    value: 10,
                },
                Inst::Jump { target: 5 },
                Inst::ConstInt {
                    dst: Temp(1),
                    ty: Type::U64,
                    value: 20,
                },
                Inst::Return {
                    value: Some(Temp(1)),
                },
            ],
        }
    }

    #[test]
    fn diamond_kills_the_join_definition_at_the_entry() {
        let l = analyze(&diamond()).expect("liveness");
        assert_eq!(l.live_out[0], Vec::<Temp>::new());
        assert_eq!(l.live_out[1], vec![Temp(1)]);
        assert_eq!(l.live_out[2], vec![Temp(1)]);
        assert_eq!(l.live_in[3], vec![Temp(1)]);
        assert_eq!(l.instructions[0].after, vec![Temp(0)]);
        assert_eq!(l.instructions[1].before, vec![Temp(0)]);
    }

    #[test]
    fn unreachable_blocks_are_not_dropped() {
        let mut f = diamond();
        f.body[3] = Inst::Jump { target: 5 };
        let l = analyze(&f).expect("liveness");
        assert!(l.cfg.blocks.iter().any(|b| b.range.start == 4));
    }

    #[test]
    fn dump_is_repeatable_and_uses_half_open_ranges() {
        let p = MwirProgram {
            fns: BTreeMap::from([("f".to_string(), diamond())]),
            rodata: Vec::new(),
        };
        let a = dump_program(&p).expect("dump");
        assert_eq!(a, dump_program(&p).expect("dump"));
        assert!(a.contains("range=[0, 2)"));
        assert!(a.contains("succ=[b1,b2]"));
        let _: BlockId = 0;
    }
}
