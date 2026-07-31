//! Compile modes and the fixed in-code release opt list
//! (plans/M19.md item B / decisions 1420–1423).
//!
//! `apply_mode` is the single front door for product modes. `apply_opts`
//! sets the same TLS knobs from an explicit list so item E can A/B a
//! candidate order offline (decision 1452). The release order is a dumb
//! `const` slice — not a plugin registry (freeze 1402 / 1406). Edit +
//! re-rank here; nowhere else.

pub mod correct;
pub mod win;

/// Compile mode: `Dev` leaves opts off; `Release` runs `RELEASE_OPTS`
/// in fixed order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileMode {
    Dev,
    Release,
}

/// Named opts that `apply_mode(Release)` may enable.
///
/// Two ids at M19 (decision 1421): lower-side bounds elision and codegen
/// narrow immediates. plans/codegen-pareto.md item C adds three more, one
/// per arithmetic substitution that passes the ∀ gate.
///
/// **Item C's five sub-items are not five ids** (decision 1745). C2, C3
/// and C5 are separable transforms and each gets its own id, so the gate
/// can attribute a refusal to one of them. C1 (W/X width selection) has
/// no id: the ∀ gate scores it at exactly zero on every case, so freeze
/// 1714 keeps it out and it lands unconditionally as a reported form
/// change (decision 1746). C4 (constant-divisor strength reduction) has
/// no id because it did not land. Both are written up in
/// plans/codegen-pareto-C.md. An id whose transform cannot be ranked
/// would be a claimed win with no evidence behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptId {
    BoundsElide,
    NarrowImm,
    /// Item C3: `narrow_to_width` emits `UBFX`/`SBFX`, not `LSL`+`LSR`.
    BfxNarrow,
    /// Item C2: the narrow checked-arithmetic range test is one masked
    /// test against one abort.
    MaskCheck,
    /// Item C5: `MOVN` and bitmask-immediate constant materialization.
    WideImmForms,
}

/// Fixed release order. Add opts here — nowhere else.
///
/// Order is part of the product (decision 1423): BoundsElide then
/// NarrowImm, then item C's three in the order their transforms compose —
/// `WideImmForms` after `MaskCheck` because `MaskCheck` deletes most of
/// the constant materializations `WideImmForms` would otherwise shorten,
/// and the gate is run in this order so that is the credit each one
/// actually earns rather than the credit it would earn alone.
pub const RELEASE_OPTS: &[OptId] = &[
    OptId::BoundsElide,
    OptId::NarrowImm,
    OptId::BfxNarrow,
    OptId::MaskCheck,
    OptId::WideImmForms,
];

/// Enable exactly the named opts (decision 1452). Product modes go
/// through [`apply_mode`]; tests and candidate A/B use this directly.
pub fn apply_opts(opts: &[OptId]) {
    crate::lower::set_bounds_elide(opts.contains(&OptId::BoundsElide));
    crate::codegen::set_narrow_imm(opts.contains(&OptId::NarrowImm));
    crate::codegen::set_bfx_narrow(opts.contains(&OptId::BfxNarrow));
    crate::codegen::set_mask_check(opts.contains(&OptId::MaskCheck));
    crate::codegen::set_wide_imm_forms(opts.contains(&OptId::WideImmForms));
}

/// Single front door for product-mode TLS knobs (decision 1422).
pub fn apply_mode(mode: CompileMode) {
    match mode {
        CompileMode::Dev => apply_opts(&[]),
        CompileMode::Release => apply_opts(RELEASE_OPTS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::{bfx_narrow, mask_check, narrow_imm, wide_imm_forms};
    use crate::lower::bounds_elide;

    /// Every knob `apply_opts` drives, read back. Written as one list so a
    /// new `OptId` whose knob is never wired reads as a failure here
    /// rather than as a silently inert entry in `RELEASE_OPTS`.
    fn live_knobs() -> Vec<(OptId, bool)> {
        vec![
            (OptId::BoundsElide, bounds_elide()),
            (OptId::NarrowImm, narrow_imm()),
            (OptId::BfxNarrow, bfx_narrow()),
            (OptId::MaskCheck, mask_check()),
            (OptId::WideImmForms, wide_imm_forms()),
        ]
    }

    #[test]
    fn dev_disables_every_opt() {
        apply_mode(CompileMode::Release);
        assert!(live_knobs().iter().all(|(_, on)| *on));

        apply_mode(CompileMode::Dev);
        for (id, on) in live_knobs() {
            assert!(!on, "{id:?} still enabled under Dev");
        }
    }

    #[test]
    fn release_enables_every_opt_in_the_list() {
        apply_mode(CompileMode::Dev);
        assert!(live_knobs().iter().all(|(_, on)| !*on));

        apply_mode(CompileMode::Release);
        for (id, on) in live_knobs() {
            assert_eq!(
                on,
                RELEASE_OPTS.contains(&id),
                "{id:?} knob disagrees with RELEASE_OPTS membership"
            );
        }
    }

    /// Every `OptId` reaches exactly one knob, and no other. A knob that
    /// is never set by its own id is an opt the ∀ gate would rank at
    /// identity while believing it measured something.
    #[test]
    fn each_opt_id_drives_exactly_its_own_knob() {
        for &id in RELEASE_OPTS {
            apply_opts(&[id]);
            for (other, on) in live_knobs() {
                assert_eq!(on, other == id, "enabling {id:?} also moved {other:?}");
            }
        }
        apply_mode(CompileMode::Release);
    }

    #[test]
    fn release_opts_order_is_the_written_down_order() {
        assert_eq!(
            RELEASE_OPTS,
            &[
                OptId::BoundsElide,
                OptId::NarrowImm,
                OptId::BfxNarrow,
                OptId::MaskCheck,
                OptId::WideImmForms,
            ]
        );
    }

    #[test]
    fn apply_opts_enables_only_named() {
        apply_opts(&[OptId::NarrowImm]);
        assert!(!bounds_elide());
        assert!(narrow_imm());

        apply_opts(&[OptId::BoundsElide]);
        assert!(bounds_elide());
        assert!(!narrow_imm());

        apply_mode(CompileMode::Release);
    }

    /// plans/M19.md item F / decisions 1460–1469: prove `dev` dumps still
    /// succeed for a representative case without doubling every golden.
    #[test]
    fn dump_asm_and_cost_succeed_under_dev() {
        use crate::codegen::{codegen_program, dump as dump_asm};
        use crate::cost::{load_default, score_program};
        use crate::lower::lower_program;
        use crate::mwir;
        use crate::sema;
        use crate::syntax::{lexer, parser};

        const SRC: &str = r#"
module examples.opts_dev_dump

pub fn add_one(x: u64) -> u64:
    return x +% 1
"#;

        let tokens = lexer::lex(SRC).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");

        apply_mode(CompileMode::Dev);
        let mwir = lower_program(&typed).expect("lower under Dev");
        let prog = codegen_program(&mwir, &layout).expect("codegen under Dev");
        let asm = dump_asm(&prog);
        assert!(
            asm.contains("Fn key=add_one"),
            "dev asm dump must name the fn:\n{asm}"
        );
        let table = load_default().expect("cost table");
        let cost = score_program(&prog, &table, &crate::placement::PlacementTable::default())
            .expect("cost under Dev");
        assert!(cost.total_proxy_cycles > 0, "dev cost dump must score > 0");

        apply_mode(CompileMode::Release);
    }
}
