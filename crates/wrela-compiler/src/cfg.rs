//! Stable, source-ordered control-flow graphs for synchronous MWIR.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::mwir::{Inst, MwirFn, MwirProgram, Temp};

pub type BlockId = usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub id: BlockId,
    pub range: Range<usize>,
    pub successors: Vec<BlockId>,
    pub predecessors: Vec<BlockId>,
    pub use_set: Vec<Temp>,
    pub def_set: Vec<Temp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cfg {
    pub blocks: Vec<Block>,
    /// Instruction index to containing block.  Keeping this map explicit
    /// makes dumps and later physical block permutations independent of the
    /// emitted order.
    pub block_of_inst: Vec<BlockId>,
}

fn terminates(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::Jump { .. } | Inst::Return { .. } | Inst::Abort { .. } | Inst::AssertFail { .. }
    )
}

fn target(inst: &Inst) -> Option<usize> {
    match inst {
        Inst::Jump { target } | Inst::JumpIfFalse { target, .. } => Some(*target),
        _ => None,
    }
}

fn sorted_temps(set: BTreeSet<Temp>) -> Vec<Temp> {
    set.into_iter().collect()
}

/// Build a CFG without discarding unreachable source blocks.
pub fn build_cfg(f: &MwirFn) -> Result<Cfg, String> {
    let n = f.body.len();
    if n == 0 {
        return Ok(Cfg {
            blocks: Vec::new(),
            block_of_inst: Vec::new(),
        });
    }

    let mut leaders = BTreeSet::from([0usize]);
    for (i, inst) in f.body.iter().enumerate() {
        if let Some(t) = target(inst) {
            if t > n {
                return Err(format!(
                    "invalid MWIR jump target {t} at instruction {i}; function has {n} instructions"
                ));
            }
            if t < n {
                leaders.insert(t);
            }
        }
        if terminates(inst) || matches!(inst, Inst::JumpIfFalse { .. }) {
            if i + 1 < n {
                leaders.insert(i + 1);
            }
        }
    }

    let starts: Vec<usize> = leaders.into_iter().collect();
    let mut blocks = Vec::with_capacity(starts.len());
    let mut block_of_inst = vec![0; n];
    let mut by_start = BTreeMap::new();
    for (id, &start) in starts.iter().enumerate() {
        let end = starts.get(id + 1).copied().unwrap_or(n);
        by_start.insert(start, id);
        for slot in &mut block_of_inst[start..end] {
            *slot = id;
        }
        blocks.push(Block {
            id,
            range: start..end,
            successors: Vec::new(),
            predecessors: Vec::new(),
            use_set: Vec::new(),
            def_set: Vec::new(),
        });
    }

    for block in &mut blocks {
        let mut use_set = BTreeSet::new();
        let mut def_set = BTreeSet::new();
        for inst in &f.body[block.range.clone()] {
            let facts = crate::mwir_facts::inst_facts(inst);
            for t in facts.uses {
                if !def_set.contains(&t) {
                    use_set.insert(t);
                }
            }
            def_set.extend(facts.defs);
        }
        block.use_set = sorted_temps(use_set);
        block.def_set = sorted_temps(def_set);

        let last = block.range.end - 1;
        let mut successors = BTreeSet::new();
        match &f.body[last] {
            Inst::Jump { target } => {
                if *target < n {
                    // Every valid jump target is a leader.
                    let id = by_start.get(target).copied().ok_or_else(|| {
                        format!("malformed CFG: jump target {target} is not a CFG leader")
                    })?;
                    successors.insert(id);
                }
            }
            Inst::JumpIfFalse { target, .. } => {
                if *target < n {
                    let id = by_start.get(target).copied().ok_or_else(|| {
                        format!("malformed CFG: conditional target {target} is not a CFG leader")
                    })?;
                    successors.insert(id);
                }
                if last + 1 < n {
                    successors.insert(block_of_inst[last + 1]);
                }
            }
            inst if terminates(inst) => {}
            _ => {
                if last + 1 < n {
                    successors.insert(block_of_inst[last + 1]);
                }
            }
        }
        block.successors = successors.into_iter().collect();
    }

    let successor_copy: Vec<Vec<BlockId>> = blocks.iter().map(|b| b.successors.clone()).collect();
    for (from, succs) in successor_copy.iter().enumerate() {
        for &to in succs {
            blocks[to].predecessors.push(from);
        }
    }
    for block in &mut blocks {
        block.predecessors.sort_unstable();
        block.predecessors.dedup();
    }

    Ok(Cfg {
        blocks,
        block_of_inst,
    })
}

pub fn build_program(program: &MwirProgram) -> Result<BTreeMap<String, Cfg>, String> {
    program
        .fns
        .iter()
        .map(|(key, f)| Ok((key.clone(), build_cfg(f)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mwir::{Inst, Temp};
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
    fn diamond_has_source_ordered_blocks_and_kill() {
        let cfg = build_cfg(&diamond()).expect("cfg");
        assert_eq!(
            cfg.blocks
                .iter()
                .map(|b| (b.range.start, b.range.end, b.successors.clone()))
                .collect::<Vec<_>>(),
            vec![
                (0, 2, vec![1, 2]),
                (2, 4, vec![3]),
                (4, 5, vec![3]),
                (5, 6, vec![]),
            ]
        );
        assert_eq!(cfg.blocks[0].use_set, Vec::<Temp>::new());
        assert_eq!(cfg.blocks[0].def_set, vec![Temp(0)]);
        assert_eq!(cfg.blocks[3].use_set, vec![Temp(1)]);
    }

    #[test]
    fn exit_target_is_allowed_and_out_of_range_is_rejected() {
        let mut f = diamond();
        f.body[3] = Inst::Jump {
            target: f.body.len(),
        };
        assert!(build_cfg(&f).is_ok());
        f.body[3] = Inst::Jump {
            target: f.body.len() + 1,
        };
        assert!(build_cfg(&f).is_err());
    }
}
