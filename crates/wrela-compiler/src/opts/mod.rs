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
/// narrow immediates. plans/codegen-pareto.md adds five more: item B's
/// one-word `ADR` addressing (decision 1730), item C's three arithmetic
/// substitutions, and item E's per-function register allocation
/// (decision 1760).
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
    /// Item B: one `ADR` where an `ADRP`+`ADD` pair stood.
    AdrAddressing,
    /// Item C3: `narrow_to_width` emits `UBFX`/`SBFX`, not `LSL`+`LSR`.
    BfxNarrow,
    /// Item C2: the narrow checked-arithmetic range test is one masked
    /// test against one abort.
    MaskCheck,
    /// Item C5: `MOVN` and bitmask-immediate constant materialization.
    WideImmForms,
    /// Item E: per-function linear-scan register allocation.
    RegAlloc,
    /// Item F5: a call in tail position is a jump (decision 1773).
    TailCalls,
    /// Item F1/F2/F4: the whole-program convention — a per-function
    /// register pool measured from that function's own emission, and a
    /// per-callee clobber set in place of a blanket call barrier
    /// (decision 1771).
    InterprocRegs,
    /// Item F3: a function with nothing left to keep gets no frame
    /// (decision 1772).
    Frameless,
}

/// Fixed release order. Add opts here — nowhere else.
///
/// Order is part of the product (decision 1423): BoundsElide then
/// NarrowImm, then item B's `AdrAddressing`, then item C's three in the
/// order their transforms compose — `WideImmForms` after `MaskCheck`
/// because `MaskCheck` deletes most of the constant materializations
/// `WideImmForms` would otherwise shorten, and the gate is run in this
/// order so that is the credit each one actually earns rather than the
/// credit it would earn alone — and **`RegAlloc` last**.
///
/// `AdrAddressing` was appended by item B under decision 1733, before
/// item C existed; it is independent of all four others (it shortens
/// rodata address materialization and changes nothing they read), so its
/// position ahead of C's three is convention, not dependency.
///
/// **RegAlloc is last, and that is a decision, not an accident**
/// (plans/codegen-pareto.md decision 1763). The allocator reads the
/// emitter's *output* — which temps are touched as whole scalars, where
/// a returning call sits, which registers are already spoken for — so it
/// must decide against the same emission the image will get. Any opt
/// that changes what `emit_one` produces has to run before it, or the
/// probe measures a program that is never built. Items B and C all change
/// emission, so all of them precede it.
///
/// **Item F's three ids all follow `RegAlloc`, and decision 1763 still
/// holds exactly as written (decision 1774).** 1763's requirement is
/// that every opt whose transform the *probe* must see runs before the
/// allocator. None of item F's three is such an opt: `InterprocRegs`
/// changes which register the allocator may choose, `Frameless` is read
/// off the allocation's own result, and `TailCalls` is applied at
/// emission only where `Frameless` already removed the frame — the probe
/// deliberately never substitutes it (decision 1776), so it measures the
/// conservative program in every case. They are ordered by their
/// dependence: each one's transform is only reachable once the one
/// before it has fired.
pub const RELEASE_OPTS: &[OptId] = &[
    OptId::BoundsElide,
    OptId::NarrowImm,
    OptId::AdrAddressing,
    OptId::BfxNarrow,
    OptId::MaskCheck,
    OptId::WideImmForms,
    OptId::RegAlloc,
    OptId::InterprocRegs,
    OptId::Frameless,
    OptId::TailCalls,
];

/// Enable exactly the named opts (decision 1452). Product modes go
/// through [`apply_mode`]; tests and candidate A/B use this directly.
pub fn apply_opts(opts: &[OptId]) {
    crate::lower::set_bounds_elide(opts.contains(&OptId::BoundsElide));
    crate::codegen::set_narrow_imm(opts.contains(&OptId::NarrowImm));
    crate::codegen::set_adr_addressing(opts.contains(&OptId::AdrAddressing));
    crate::codegen::set_bfx_narrow(opts.contains(&OptId::BfxNarrow));
    crate::codegen::set_mask_check(opts.contains(&OptId::MaskCheck));
    crate::codegen::set_wide_imm_forms(opts.contains(&OptId::WideImmForms));
    crate::regalloc::set_regalloc(opts.contains(&OptId::RegAlloc));
    crate::codegen::set_tail_calls(opts.contains(&OptId::TailCalls));
    crate::regalloc::set_interproc_regs(opts.contains(&OptId::InterprocRegs));
    crate::codegen::set_frameless_fns(opts.contains(&OptId::Frameless));
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
    use crate::codegen::{
        adr_addressing, bfx_narrow, frameless_fns, mask_check, narrow_imm, tail_calls,
        wide_imm_forms,
    };
    use crate::lower::bounds_elide;

    /// Every knob `apply_opts` drives, read back. Written as one list so a
    /// new `OptId` whose knob is never wired reads as a failure here
    /// rather than as a silently inert entry in `RELEASE_OPTS`.
    fn live_knobs() -> Vec<(OptId, bool)> {
        vec![
            (OptId::BoundsElide, bounds_elide()),
            (OptId::NarrowImm, narrow_imm()),
            (OptId::AdrAddressing, adr_addressing()),
            (OptId::BfxNarrow, bfx_narrow()),
            (OptId::MaskCheck, mask_check()),
            (OptId::WideImmForms, wide_imm_forms()),
            (OptId::RegAlloc, crate::regalloc::regalloc()),
            (OptId::TailCalls, tail_calls()),
            (OptId::InterprocRegs, crate::regalloc::interproc_regs()),
            (OptId::Frameless, frameless_fns()),
        ]
    }

    #[test]
    fn dev_disables_every_opt() {
        apply_mode(CompileMode::Release);
        assert!(live_knobs().iter().all(|(_, on)| *on));

        apply_mode(CompileMode::Dev);
        for (id, on) in live_knobs() {
            assert!(
                !on,
                "{id:?} still enabled under Dev — `dev` keeps spill-everything \
                 and every reference form (M19 freeze 1407)"
            );
        }
    }

    /// plans/codegen-pareto.md decision 1763: RegAlloc is last because
    /// it decides against the emitter's *output*, so every opt that
    /// changes emission must already be on when its probe runs.
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
                OptId::AdrAddressing,
                OptId::BfxNarrow,
                OptId::MaskCheck,
                OptId::WideImmForms,
                OptId::RegAlloc,
                OptId::InterprocRegs,
                OptId::Frameless,
                OptId::TailCalls,
            ]
        );
        // Decision 1774: everything the *probe* must see precedes
        // `RegAlloc`; only the two opts that read the allocation's own
        // result follow it.
        let after: Vec<OptId> = RELEASE_OPTS
            .iter()
            .skip_while(|o| **o != OptId::RegAlloc)
            .skip(1)
            .copied()
            .collect();
        assert_eq!(
            after,
            vec![OptId::InterprocRegs, OptId::Frameless, OptId::TailCalls],
            "only allocation-reading opts may follow the allocator"
        );
    }

    #[test]
    fn apply_opts_enables_only_named() {
        apply_opts(&[OptId::NarrowImm]);
        assert!(!bounds_elide());
        assert!(narrow_imm());
        assert!(!adr_addressing());
        assert!(!crate::regalloc::regalloc());

        apply_opts(&[OptId::BoundsElide]);
        assert!(bounds_elide());
        assert!(!narrow_imm());
        assert!(!adr_addressing());
        assert!(!crate::regalloc::regalloc());

        apply_opts(&[OptId::AdrAddressing]);
        assert!(!bounds_elide());
        assert!(!narrow_imm());
        assert!(adr_addressing());
        assert!(!crate::regalloc::regalloc());

        apply_opts(&[OptId::RegAlloc]);
        assert!(!bounds_elide());
        assert!(!narrow_imm());
        assert!(!adr_addressing());
        assert!(crate::regalloc::regalloc());

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
