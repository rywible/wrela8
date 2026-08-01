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
/// An id here is *nameable*, not *shipped*: `RELEASE_OPTS` is the
/// shipped list and [`PARKED_OPTS`] is the refused-but-kept list, and
/// every id belongs to exactly one of them (`unit:
/// every_opt_id_is_either_shipped_or_parked`).
///
/// M19 (decision 1421) named two: lower-side bounds elision and codegen
/// narrow immediates. `BoundsElide` is **parked** — deleted by
/// plans/codegen-pareto-2.md item L (decision 1970) and restored,
/// disabled, by item N (decision 1911) under CLAUDE.md's parking rule.
/// plans/codegen-pareto.md adds five more: item B's
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
    /// **Parked** (decision 1911) — see [`PARKED_OPTS`]. plans/M18.md
    /// item I / plans/M19.md item D: a `[T; N]` index whose index is an
    /// integer literal proved in range lowers to `Project`/`SetField`
    /// instead of `IndexGet`/`IndexSet`, dropping the runtime bounds
    /// check. Not in `RELEASE_OPTS`.
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
    /// Item F1/F2/F4: the whole-program convention — a per-function
    /// register pool measured from that function's own emission, and a
    /// per-callee clobber set in place of a blanket call barrier
    /// (decision 1771).
    InterprocRegs,
    /// Item F3: a function with nothing left to keep gets no frame
    /// (decision 1772).
    Frameless,
    /// Item F5: a call in tail position is a `B`, not `BL`+`RET`
    /// (decision 1909 — rankable since the corpus started firing them).
    TailCalls,
    /// Item B4, landed by plans/codegen-pareto-2.md item L: an
    /// unconditional branch to the next emitted word is deleted, where
    /// deleting it merges no Lane 2 block (decision 1973).
    BranchCleanup,
    /// plans/codegen-pareto-2.md item J / decision 1924: constant propagation and folding over an
    /// extended basic block, through the evaluator's own arithmetic,
    /// plus resolution of a constant-condition branch. Item J's SCCP
    /// slot, named for what it is — MWIR is not SSA.
    ConstProp,
    /// Item J / decision 1925: value numbering of pure scalar
    /// computations over an extended basic block.
    Gvn,
    /// Item J / decision 1926: deletion of non-trapping pure
    /// instructions whose result is read nowhere, and of unreachable
    /// instructions.
    Dce,
    /// **plans/codegen-pareto-2.md item P / decision 1980: the shrinking
    /// inliner — present, wired, and deliberately *not* in
    /// [`RELEASE_OPTS`].**
    ///
    /// The ladder's 2a. Item J built it, measured it, refused it
    /// (decision 1935) and deleted it before it was ever committed, so
    /// the numbers that re-ranked the ladder's #1 candidate could not be
    /// reproduced from this repository. CLAUDE.md's rule changed on the
    /// strength of that: a refused opt is *parked*, not deleted — it
    /// stays in the tree carrying its refusal measurement, its mechanism
    /// and the named condition that would make it worth re-asking, and it
    /// must still compile and pass `diff-eval` so it cannot rot into a
    /// miscompile while parked.
    ///
    /// An id rather than a bare knob because the refusal has to be
    /// re-derivable by exactly the machinery that ranks everything else:
    /// `compare_opt_lists(&[..], &[.., Inline])`. Membership of
    /// `RELEASE_OPTS` is the product decision and this is not in it.
    Inline,
}

/// Fixed release order. Add opts here — nowhere else.
///
/// Order is part of the product (decision 1423): `NarrowImm` first —
/// `BoundsElide` used to precede it and is now parked ([`PARKED_OPTS`],
/// decisions 1970/1911) — then item B's `AdrAddressing`, then item C's three in the
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
/// **Item J's three ids lead the list, and that is pipeline order, not
/// preference** (decision 1928). All three rewrite MWIR, which is the
/// stage before every other id's own: `NarrowImm`, item B's and item C's
/// three all rewrite *emitted words*, and decision 1763 puts everything
/// the allocator's probe must see ahead of `RegAlloc`. Their order among
/// themselves is dependence: constant propagation folds first, GVN
/// numbers what is left, and DCE collects what the other two orphaned.
///
/// **Two ids exist and are not here, and that is the point.**
/// `Inline` (item P, decision 1980) — the ladder's 2a lost on both words
/// and cycles in every framing item J asked it in, and item P re-derives
/// that with the pipeline-position question item J's measurement could
/// not answer. `BoundsElide` (item N) — byte-identical to `dev` on every
/// program the appliance ships.
///
/// **A refused opt is not here and is not gone either** — it is in
/// [`PARKED_OPTS`], where it compiles, passes `diff-eval` and carries its
/// own refusal. `each_release_opt_...` and the two order tests below read
/// this slice, so adding an id here would be the whole product change.
pub const RELEASE_OPTS: &[OptId] = &[
    OptId::ConstProp,
    OptId::Gvn,
    OptId::Dce,
    OptId::NarrowImm,
    OptId::AdrAddressing,
    OptId::BfxNarrow,
    OptId::MaskCheck,
    OptId::WideImmForms,
    OptId::RegAlloc,
    OptId::InterprocRegs,
    OptId::Frameless,
    OptId::BranchCleanup,
    OptId::TailCalls,
];

/// **Refused, kept, disabled** (CLAUDE.md 2026-07-31; decision 1911).
///
/// An opt here is not shipped and not deleted. It stays wired to its own
/// knob, so `apply_opts(&[.., id, ..])` still turns it on, and it is held
/// to `diff-eval` like any other transform — a parked opt that has rotted
/// into a miscompile is not parked, it is a trap. Nothing in the product
/// reaches it: no `CompileMode` names it and it is absent from
/// `RELEASE_OPTS`, so its TLS knob defaults off.
///
/// Each entry carries the three things the doctrine requires — the
/// measurement that refused it, the mechanism, and the named workload or
/// capability that would make it worth re-asking.
///
/// ## `BoundsElide` — literal-index bounds-check elision
///
/// **The measurement (item H, re-verified by item L, decision 1970).**
/// Byte-identical to `dev` on all four programs the appliance ships:
/// same proxy cycles, same emitted words, same hot text, on
/// `--stage=asm`, `--stage=cost` and `--stage=image` for
/// `cost-product-{actors,appliance,blk,receipt}`. Its entire measured
/// effect was six microbenchmarks, the largest of which
/// (`cost-bounds-elide`, 1839 → 314 proxy cycles) was written for it.
/// M20's evidence block had credited it with 43.2 % of release's cycle
/// win; that credit came from fixtures, and the ∀ gate scored it `veto`
/// on the product tier — the only `veto` row that set has ever carried.
///
/// **The mechanism.** The transform fires only on an index that is an
/// integer *literal*, syntactically, at the point of the index. The
/// appliance's code indexes with loop variables, actor ids and field
/// reads, not with `a[3]`; and where a literal index does occur it is
/// usually a struct-shaped access that never became an array in the
/// first place. So the opt's precondition is a property of *fixture*
/// code, and widening the corpus to programs nobody wrote for the gate
/// took its measured effect to exactly zero. It is decision 1716's
/// self-selection failure, caught by exactly the widening item H exists
/// to do.
///
/// **The named condition for re-asking it — a *capability*, not a
/// workload (decision 1916).** The obvious candidate was a workload:
/// item M's tile compositor (`tests/golden/boot-tile-compositor`, scored
/// as `cost-product-compositor`), the repo's first compute title, tight
/// loops over `[u32; 128]` tile buffers. Item N measured it there, and
/// the answer is instructive: the opt moves the case by −127 cycles /
/// −135 words and falls at every one of the 512 points of its box — and
/// **not one of those words is in the kernel**. The only two functions
/// it changes are `sprite_is_exact` and `background_pass_is_exact`, the
/// case's own `@test(runtime)` assertions, which check `pixels[0]` and
/// `pixels[127]`. `fill_background`, `blit_scaled`, `make_sprite` and
/// `render_strip` are untouched, because — as that file says of its own
/// hot loop — every index is computed rather than constant. A compute
/// workload was not what this opt was missing.
///
/// So the condition is the capability: **`ConstProp` (or an index-range
/// analysis) able to turn a folded index into a proved-in-range constant
/// that this transform can see.** Its precondition is syntactic — a
/// `TypedExprKind::Int` at the index, at lowering time — so today its
/// only supplier is what the programmer typed, and an unrolled or
/// constant-folded index is known to the compiler and still misses,
/// because by the time it is known, lowering is over. Item J's
/// `ConstProp` runs on MWIR, one stage too late to feed it. Re-ask when
/// that gap closes, or when a title indexes a fixed-size array at
/// literals in its *hot* code rather than in its assertions.
///
/// Un-parking is a human decision, and it needs the ∀ product-tier gate
/// green — not a microbenchmark, and not a delta that lives in test
/// scaffolding.
/// **`Frameless` joins them (decision 1918).** Item F3 elides the `x30`
/// save on a function with no returning call — two fewer instructions,
/// and it won on every boot-shaped program. Item M's compute workload
/// refused it: `cost-product-compositor` rose **7526 → 7544** at every
/// `store_to_load_forwarding=1` corner, which vetoed the whole release
/// list. Narrowing the elision to cases where the frame size does not
/// move (so no slot's absolute address shifts) did **not** fix it and
/// measured slightly worse, **7526 → 7563** — so the mechanism is not the
/// frame-size shuffle that narrowing was built on, and it is not yet
/// known.
///
/// **Re-ask when:** the mechanism is identified. The honest state is that
/// two instructions are being deleted and a compute kernel gets slower,
/// and nobody can say why. That is a question about the ruler or about
/// the allocator's interaction with `x30`, not about F3's rule.
pub const PARKED_OPTS: &[OptId] = &[OptId::BoundsElide, OptId::Inline];

/// Every id the compiler knows, shipped then parked. Both lists in one
/// place so a caller that means "all of them" cannot pick up only the
/// shipped half.
pub fn all_opts() -> Vec<OptId> {
    RELEASE_OPTS.iter().chain(PARKED_OPTS).copied().collect()
}

/// An id by its `Debug` name, for the one caller that takes an opt name
/// from a command line (`cargo xtask diff-eval --with-opt <Name>`,
/// decision 1913). Derived from [`all_opts`] rather than a second
/// hand-written table, so a new id is spellable the day it exists and a
/// removed one stops being spellable the day it does not.
pub fn opt_by_name(name: &str) -> Option<OptId> {
    all_opts().into_iter().find(|id| format!("{id:?}") == name)
}

/// Enable exactly the named opts (decision 1452). Product modes go
/// through [`apply_mode`]; tests and candidate A/B use this directly.
/// A [`PARKED_OPTS`] id may be named here and it will fire — that is
/// what keeps a park honest.
pub fn apply_opts(opts: &[OptId]) {
    crate::mwir_opt::set_inline(opts.contains(&OptId::Inline));
    crate::lower::set_bounds_elide(opts.contains(&OptId::BoundsElide));
    crate::mwir_opt::set_const_prop(opts.contains(&OptId::ConstProp));
    crate::mwir_opt::set_gvn(opts.contains(&OptId::Gvn));
    crate::mwir_opt::set_dce(opts.contains(&OptId::Dce));
    crate::codegen::set_narrow_imm(opts.contains(&OptId::NarrowImm));
    crate::codegen::set_adr_addressing(opts.contains(&OptId::AdrAddressing));
    crate::codegen::set_bfx_narrow(opts.contains(&OptId::BfxNarrow));
    crate::codegen::set_mask_check(opts.contains(&OptId::MaskCheck));
    crate::codegen::set_wide_imm_forms(opts.contains(&OptId::WideImmForms));
    crate::regalloc::set_regalloc(opts.contains(&OptId::RegAlloc));
    crate::regalloc::set_interproc_regs(opts.contains(&OptId::InterprocRegs));
    crate::codegen::set_frameless_fns(opts.contains(&OptId::Frameless));
    crate::codegen::set_tail_calls(opts.contains(&OptId::TailCalls));
    crate::codegen::set_branch_cleanup(opts.contains(&OptId::BranchCleanup));
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
        adr_addressing, bfx_narrow, branch_cleanup, frameless_fns, mask_check, narrow_imm,
        wide_imm_forms,
    };
    /// Every knob `apply_opts` drives, read back. Written as one list so a
    /// new `OptId` whose knob is never wired reads as a failure here
    /// rather than as a silently inert entry in `RELEASE_OPTS` — or, for
    /// a [`PARKED_OPTS`] id, as a park that cannot actually be entered.
    fn live_knobs() -> Vec<(OptId, bool)> {
        vec![
            (OptId::Inline, crate::mwir_opt::inlining()),
            (OptId::BoundsElide, crate::lower::bounds_elide()),
            (OptId::ConstProp, crate::mwir_opt::const_prop()),
            (OptId::Gvn, crate::mwir_opt::gvn()),
            (OptId::Dce, crate::mwir_opt::dce()),
            (OptId::NarrowImm, narrow_imm()),
            (OptId::AdrAddressing, adr_addressing()),
            (OptId::BfxNarrow, bfx_narrow()),
            (OptId::MaskCheck, mask_check()),
            (OptId::WideImmForms, wide_imm_forms()),
            (OptId::RegAlloc, crate::regalloc::regalloc()),
            (OptId::InterprocRegs, crate::regalloc::interproc_regs()),
            (OptId::Frameless, frameless_fns()),
            (OptId::TailCalls, crate::codegen::tail_calls()),
            (OptId::BranchCleanup, branch_cleanup()),
        ]
    }

    #[test]
    fn dev_disables_every_opt() {
        apply_mode(CompileMode::Release);
        // Every knob `RELEASE_OPTS` names, and only those: item P's
        // `Inline` is wired and parked, so `release` must leave it off.
        for (id, on) in live_knobs() {
            assert_eq!(on, RELEASE_OPTS.contains(&id), "{id:?} under release");
        }
        assert!(
            live_knobs()
                .iter()
                .all(|(id, on)| *on == RELEASE_OPTS.contains(id))
        );

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

    /// **Every id is shipped or parked, never both and never neither**
    /// (decision 1911). `live_knobs` is the exhaustive list of ids, so
    /// this is also what catches an id added to `OptId` and to no list:
    /// a parked opt is a member of the tree with a reason attached, and
    /// an id in neither list is just an unreachable knob.
    #[test]
    fn every_opt_id_is_either_shipped_or_parked() {
        for (id, _) in live_knobs() {
            let shipped = RELEASE_OPTS.contains(&id);
            let parked = PARKED_OPTS.contains(&id);
            assert!(
                shipped != parked,
                "{id:?} is {} — every id belongs to exactly one of \
                 RELEASE_OPTS and PARKED_OPTS",
                if shipped {
                    "in both lists"
                } else {
                    "in neither list"
                }
            );
        }
        assert_eq!(
            live_knobs().len(),
            RELEASE_OPTS.len() + PARKED_OPTS.len(),
            "an id in a list has no knob in `live_knobs`"
        );
        assert!(
            PARKED_OPTS.contains(&OptId::BoundsElide),
            "`BoundsElide` is parked, not deleted (decision 1911): item H \
             measured it byte-identical to `dev` on all four product \
             programs, and CLAUDE.md's 2026-07-31 rule keeps a refused opt \
             in the tree with its refusal, its mechanism and its re-ask \
             condition attached"
        );
        assert!(!RELEASE_OPTS.contains(&OptId::BoundsElide));
    }

    /// A parked opt whose knob is dead is not parked, it is broken. The
    /// product never names it, so this is the only thing standing
    /// between `PARKED_OPTS` and a comment.
    #[test]
    fn a_parked_opt_can_still_be_switched_on() {
        for &id in PARKED_OPTS {
            apply_mode(CompileMode::Release);
            assert!(
                live_knobs().iter().all(|(other, on)| !*on || *other != id),
                "{id:?} is parked but `Release` enables it"
            );
            apply_opts(&[id]);
            assert!(
                live_knobs().iter().any(|(other, on)| *other == id && *on),
                "{id:?} is parked and its knob does not respond to \
                 `apply_opts` — a park with a dead knob cannot be \
                 diff-eval'd and rots into a miscompile"
            );
        }
        apply_mode(CompileMode::Release);
    }

    /// Every `OptId` reaches exactly one knob, and no other. A knob that
    /// is never set by its own id is an opt the ∀ gate would rank at
    /// identity while believing it measured something. Parked ids are
    /// included: their knobs are held to the same standard.
    #[test]
    fn each_opt_id_drives_exactly_its_own_knob() {
        // Over every *wired* id, shipped and parked: a parked opt whose
        // knob were cross-wired would make its own refusal measurement a
        // measurement of something else.
        for &id in RELEASE_OPTS.iter().chain(PARKED_OPTS) {
            apply_opts(&[id]);
            for (other, on) in live_knobs() {
                assert_eq!(on, other == id, "enabling {id:?} also moved {other:?}");
            }
        }
        apply_mode(CompileMode::Release);
    }

    /// **The park, asserted** (item P / decision 1980, CLAUDE.md's
    /// 2026-07-31 rule). `Inline` is in `OptId` — so the refusal can be
    /// re-derived by the same machinery that ranks everything else — and
    /// is not in `RELEASE_OPTS`, because it lost. Deleting it is what
    /// item J did, and it cost the tree the reproducibility of the
    /// measurement that re-ranked the ladder's #1 candidate.
    #[test]
    fn the_inliner_is_wired_and_parked() {
        assert!(
            live_knobs().iter().any(|(id, _)| *id == OptId::Inline),
            "`OptId::Inline` must reach a knob, or the park is a graveyard"
        );
        assert!(
            !RELEASE_OPTS.contains(&OptId::Inline),
            "item P reports the number; a human decides whether it ships"
        );
        apply_opts(&[OptId::Inline]);
        assert!(crate::mwir_opt::inlining());
        apply_mode(CompileMode::Release);
        assert!(!crate::mwir_opt::inlining());
    }

    #[test]
    fn release_opts_order_is_the_written_down_order() {
        assert_eq!(
            RELEASE_OPTS,
            &[
                OptId::ConstProp,
                OptId::Gvn,
                OptId::Dce,
                OptId::NarrowImm,
                OptId::AdrAddressing,
                OptId::BfxNarrow,
                OptId::MaskCheck,
                OptId::WideImmForms,
                OptId::RegAlloc,
                OptId::InterprocRegs,
                OptId::Frameless,
                OptId::BranchCleanup,
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
            vec![
                OptId::InterprocRegs,
                OptId::Frameless,
                OptId::BranchCleanup,
                OptId::TailCalls,
            ],
            "only allocation-reading opts, B4 — which deletes a word the \
             allocator never reads — and F5, which fires only where F3 has \
             already removed the frame, may follow the allocator"
        );
    }

    #[test]
    fn apply_opts_enables_only_named() {
        apply_opts(&[OptId::NarrowImm]);
        assert!(narrow_imm());
        assert!(!adr_addressing());
        assert!(!crate::regalloc::regalloc());

        apply_opts(&[OptId::AdrAddressing]);
        assert!(!narrow_imm());
        assert!(adr_addressing());
        assert!(!crate::regalloc::regalloc());

        apply_opts(&[OptId::RegAlloc]);
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
