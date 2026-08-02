//! The single source of truth for MWIR dataflow facts.
//!
//! This module is deliberately boring.  A number of consumers need to know
//! what an instruction reads and writes, but that question is not the same as
//! whether the instruction may be removed.  Keeping the two questions here
//! prevents an optimisation from accidentally treating a side effect as a
//! dead definition.

use crate::mwir::{Inst, Temp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effects {
    None,
    MayTrap,
    Observable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstFacts {
    pub uses: Vec<Temp>,
    pub defs: Vec<Temp>,
    pub address_escapes: Vec<Temp>,
    pub effects: Effects,
}

impl InstFacts {
    fn new(
        uses: impl IntoIterator<Item = Temp>,
        defs: impl IntoIterator<Item = Temp>,
        address_escapes: impl IntoIterator<Item = Temp>,
        effects: Effects,
    ) -> Self {
        Self {
            uses: sorted_unique(uses),
            defs: sorted_unique(defs),
            address_escapes: sorted_unique(address_escapes),
            effects,
        }
    }
}

fn sorted_unique(values: impl IntoIterator<Item = Temp>) -> Vec<Temp> {
    let mut values: Vec<Temp> = values.into_iter().collect();
    values.sort_unstable();
    values.dedup();
    values
}

/// Return the dataflow and removability facts for one MWIR instruction.
///
/// Mutation is represented as both a read and a definition.  In particular,
/// `SetField` and the indexed-set forms read the old aggregate before writing
/// its new value.  Call writebacks are definitions as well as uses: the
/// callee receives the old value and may replace it.
pub(crate) fn inst_facts(inst: &Inst) -> InstFacts {
    use Effects::{MayTrap, None, Observable};
    use Inst::*;

    match inst {
        ConstInt { dst, .. }
        | ConstBool { dst, .. }
        | ConstFloat { dst, .. }
        | ConstChar { dst, .. }
        | ConstUnit { dst }
        | ConstText { dst, .. } => InstFacts::new([], [*dst], [], None),

        Copy { dst, src }
        | EnumTag { dst, src }
        | Not { dst, src }
        | EnumPayload { dst, src, .. }
        | FormatScalar { dst, src, .. }
        | Neg { dst, src, .. }
        | BitNot { dst, src, .. }
        | Convert { dst, src, .. } => {
            let effects = match inst {
                Neg { abort, .. } | Convert { abort, .. } if !abort.is_empty() => MayTrap,
                _ => None,
            };
            InstFacts::new([*src], [*dst], [], effects)
        }

        MakeAggregate { dst, elems } => InstFacts::new(elems.iter().copied(), [*dst], [], None),
        MakeEnum { dst, payload, .. } => InstFacts::new(payload.iter().copied(), [*dst], [], None),
        StringConcat { dst, lhs, rhs, .. } => InstFacts::new([*lhs, *rhs], [*dst], [], MayTrap),
        Project { dst, base, .. } => InstFacts::new([*base], [*dst], [], None),

        SetField { base, value, .. } => InstFacts::new([*base, *value], [*base], [], None),
        IndexGet {
            dst, base, index, ..
        }
        | IndexGetProven {
            dst, base, index, ..
        }
        | PlacedIndexGet {
            dst, base, index, ..
        }
        | PlacedIndexGetProven {
            dst, base, index, ..
        }
        | BytesIndexGet { dst, base, index } => {
            InstFacts::new([*base, *index], [*dst], [], MayTrap)
        }
        IndexSet {
            base, index, value, ..
        }
        | PlacedIndexSet {
            base, index, value, ..
        }
        | IndexSetProven {
            base, index, value, ..
        }
        | PlacedIndexSetProven {
            base, index, value, ..
        } => InstFacts::new([*base, *index, *value], [*base], [], MayTrap),

        ArithChecked { dst, lhs, rhs, .. }
        | ArithWrapping { dst, lhs, rhs, .. }
        | DivRem { dst, lhs, rhs, .. }
        | Shift { dst, lhs, rhs, .. }
        | Bitwise { dst, lhs, rhs, .. }
        | Compare { dst, lhs, rhs, .. }
        | BoolAnd { dst, lhs, rhs } => {
            let effects = match inst {
                ArithChecked { .. } | DivRem { .. } | Shift { .. } => MayTrap,
                _ => None,
            };
            InstFacts::new([*lhs, *rhs], [*dst], [], effects)
        }

        Jump { .. } => InstFacts::new([], [], [], None),
        JumpIfFalse { cond, .. } => InstFacts::new([*cond], [], [], None),

        Call {
            dst,
            write_backs,
            args,
            ..
        } => {
            let uses = args
                .iter()
                .copied()
                .chain(write_backs.iter().map(|(_, t)| *t));
            let defs = std::iter::once(*dst).chain(write_backs.iter().map(|(_, t)| *t));
            // A call is an ABI boundary.  Treating its arguments as escaped is
            // conservative and is important for aggregate/address allocation.
            InstFacts::new(uses, defs, args.iter().copied(), Observable)
        }
        Return { value } => InstFacts::new(value.iter().copied(), [], [], Observable),

        MmioRead { dst, base, .. } => InstFacts::new([*base], [*dst], [*base], Observable),
        MmioWrite { base, value, .. } => InstFacts::new([*base, *value], [], [*base], Observable),
        LoadIrqVector { dst, .. } => InstFacts::new([], [*dst], [], Observable),
        InterruptCellLoadAcquire { dst, .. } => InstFacts::new([], [*dst], [], Observable),
        InterruptCellStoreRelease { value, .. } => InstFacts::new([*value], [], [], Observable),
        InterruptCellSwapAcquire { dst, value, .. }
        | InterruptCellFetchOrRelease { dst, value, .. } => {
            InstFacts::new([*value], [*dst], [], Observable)
        }
        Dmb { .. } | Wake { .. } => InstFacts::new([], [], [], Observable),

        Now { dst } | Entropy { dst, .. } => InstFacts::new([], [*dst], [], Observable),
        SlotMapMint { map } => InstFacts::new([*map], [*map], [], Observable),

        MemLoad { dst, base, .. } => InstFacts::new([*base], [*dst], [*base], Observable),
        MemStore { base, value, .. } => InstFacts::new([*base, *value], [], [*base], Observable),
        PtrOffset { dst, base, .. } => InstFacts::new([*base], [*dst], [*base], None),
        TurnAddrFromId { dst, id } => InstFacts::new([*id], [*dst], [], None),
        Abort { .. } | AssertFail { .. } => InstFacts::new([], [], [], Observable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mwir::Inst;
    use crate::sema::types::Type;
    use crate::syntax::ast::BinOp;

    #[test]
    fn mutations_read_and_define_the_mutated_aggregate() {
        let f = inst_facts(&Inst::SetField {
            base: Temp(4),
            index: 0,
            value: Temp(2),
        });
        assert_eq!(f.uses, vec![Temp(2), Temp(4)]);
        assert_eq!(f.defs, vec![Temp(4)]);
        assert_eq!(f.effects, Effects::None);

        let f = inst_facts(&Inst::IndexSet {
            base: Temp(3),
            index: Temp(1),
            value: Temp(2),
            len: 4,
        });
        assert_eq!(f.uses, vec![Temp(1), Temp(2), Temp(3)]);
        assert_eq!(f.defs, vec![Temp(3)]);
    }

    #[test]
    fn definitions_are_separate_from_side_effects() {
        let pure = inst_facts(&Inst::ConstInt {
            dst: Temp(0),
            ty: Type::U64,
            value: 1,
        });
        assert_eq!(pure.defs, vec![Temp(0)]);
        assert_eq!(pure.effects, Effects::None);

        let call = inst_facts(&Inst::Call {
            dst: Temp(0),
            write_backs: vec![(0, Temp(2))],
            key: "f".to_string(),
            args: vec![Temp(1)],
        });
        assert_eq!(call.defs, vec![Temp(0), Temp(2)]);
        assert_eq!(call.effects, Effects::Observable);
        assert_eq!(call.address_escapes, vec![Temp(1)]);
    }

    #[test]
    fn facts_are_sorted_and_deduplicated() {
        let f = inst_facts(&Inst::ArithWrapping {
            dst: Temp(9),
            op: BinOp::Add,
            ty: Type::U64,
            lhs: Temp(2),
            rhs: Temp(1),
        });
        assert_eq!(f.uses, vec![Temp(1), Temp(2)]);
        assert_eq!(f.defs, vec![Temp(9)]);
    }
}
