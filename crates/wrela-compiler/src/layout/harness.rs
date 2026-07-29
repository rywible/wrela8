//! Runtime emission + boot harness (plans/M10.md item K).
//!
//! Extracted from `layout.rs` after the M10 migrations: floor stubs,
//! specialized inject helpers, JIT materializers of specialized twins,
//! and the `wrela test` entry driver / abort-tail / image packing. Pure
//! image layout (sections, placement, pool backing, reports) stays in
//! the parent module. Public items are re-exported from `layout` so
//! existing `layout::…` call sites keep stable paths.

use std::collections::BTreeMap;

use wrela_machine::machine_info as mi;
use wrela_machine::{console, layout as machine_layout, machine_info, mmio};

use super::{
    BootCtx, BootInitArg, BootInitCall, CheckpointBlock, DeviceRegs, GroupServiceCtx, ImageLayout,
    IrqVectorEntry, LayoutError, PoolPlacement, RuntimePlacement, RuntimeTables, RuntimeWiring,
    Section, WakeDrainEntry, build_irq_host_injects, checkpoint_irq_shape, device_register_windows,
    driver_irq_vector, driver_state_addr, driver_wake_pending_addr, group_service_ctx,
    group_service_shape, image_pool_backings, intern_fallible_init_abort_messages, pad_to,
    patch_adrp_add, patch_bl, patch_load_imm_words, place_device_regs, place_pools,
    place_pools_unchecked, place_runtime_tables, resolve_cross_core_edge,
    resolve_mailbox_actor_addrs, resolve_xreply_edge, round_up, steer_rtdata_base,
    turns_deref_needs_rtdata, unresolved_call_target, verify_device_windows, verify_pool_windows,
    verify_section_sizes, wake_needs_rtdata,
};
use crate::codegen::{CodegenProgram, Reloc};
use crate::encode;

// --- scratch registers for stub emission (never x0..x8/x29/x30/sp) -----

const X_SP: u8 = 31;
const SCRATCH_A: u8 = 9;
const SCRATCH_B: u8 = 10;
const SCRATCH_C: u8 = 11;
/// The same bit pattern as `X_SP` (register field `31`), used only where
/// the instruction's own Rt/source-register position is meant — `STR`'s
/// own `Rt=11111` always denotes `XZR`, never `SP` (unlike `ADD`
/// (immediate)'s Rd/Rn field, where `31` means `SP`) — a separate name so
/// a reader never has to reason about which encoding class is in play at
/// each call site.
#[allow(dead_code)]
const X_ZR: u8 = 31;

/// Placeholder failure exit codes (module doc's own "Entry/abort
/// contract" section) — distinct only so a post-mortem guest memory dump
/// can tell, at a glance, which placeholder path halted; item E is
/// expected to keep writing *a* documented exit code at this same
/// `machine_info::OFF_EXIT_CODE` offset even once these bodies grow real
/// console output, whether or not it reuses these exact numeric values.
pub const EXIT_CODE_NO_RUNTIME: u64 = 0xE000_0001;
pub const EXIT_CODE_ABORT_FIXED: u64 = 0xE000_0002;
pub const EXIT_CODE_ABORT_VAL: u64 = 0xE000_0003;

pub(super) fn push_load_imm(words: &mut Vec<u32>, reg: u8, value: i64) {
    let bits = value as u64;
    let h0 = (bits & 0xFFFF) as u16;
    let h1 = ((bits >> 16) & 0xFFFF) as u16;
    let h2 = ((bits >> 32) & 0xFFFF) as u16;
    let h3 = ((bits >> 48) & 0xFFFF) as u16;
    words.push(encode::enc_movz(reg, h0, 0, true));
    words.push(encode::enc_movk(reg, h1, 16, true));
    words.push(encode::enc_movk(reg, h2, 32, true));
    words.push(encode::enc_movk(reg, h3, 48, true));
}

/// The shared halt sequence the entry stub and the build-image abort
/// landings end in — module doc's own "shared halt sequence" paragraph.
/// plans/M10.md item C / decision 655: the named `build_abort_stub`
/// helpers are gone; build images keep only this minimal halt so
/// `Reloc::AbortFixed`/`AbortVal` still have a landing address (the test
/// print path lives in force-rooted `__wrela_abort` / `__wrela_abort_val`).
pub(super) fn push_halt(words: &mut Vec<u32>, exit_code: u64) {
    push_load_imm(words, SCRATCH_A, exit_code as i64);
    let exit_code_addr = machine_layout::MACHINE_INFO_BASE + machine_info::OFF_EXIT_CODE;
    push_load_imm(words, SCRATCH_B, exit_code_addr as i64);
    words.push(encode::enc_str_x_imm(SCRATCH_A, SCRATCH_B, 0));
    push_load_imm(words, SCRATCH_C, mmio::EXIT_MMIO_ADDR as i64);
    words.push(encode::enc_str_x_imm(SCRATCH_A, SCRATCH_C, 0));
    words.push(encode::enc_brk(0));
}

pub(super) fn build_entry_stub() -> Vec<u32> {
    let mut words = Vec::new();
    // Production stub: single-core high-DRAM stack (plans/M15.md item D).
    // Actor images use `build_primary_entry_trampoline` with the sealed N.
    let sp_top = machine_layout::core_stack_base_n(0, 1) + machine_layout::CORE_STACK_SIZE;
    push_load_imm(&mut words, SCRATCH_A, sp_top as i64);
    words.push(encode::enc_add_imm(X_SP, SCRATCH_A, 0, true)); // `mov sp, x9`
    push_halt(&mut words, EXIT_CODE_NO_RUNTIME);
    words
}
/// plans/M6.md item E, decision 7/06 §4: `__wrela_checkpoint_service` is
/// now real — the shared target every `codegen::Reloc::CheckpointService`
/// `BL` resolves to. M11 I: the checkpoint section holds only the
/// floor-cat2 LR trampoline around `BL __wrela_rt_checkpoint` (decision
/// 821); vector0 / pending / IRQ / wake algorithms live in force-rooted
/// wrela. When `link_body` is false (no runtime wiring), the section is a
/// bare `ret` so images without the wrela body still link.
pub fn build_checkpoint_and_vector_stub(group: Option<&GroupServiceCtx>) -> CheckpointBlock {
    build_checkpoint_and_vector_stub_ex(group, &[], &[], true)
}

/// plans/M11.md item I: checkpoint section trampoline. `irq_vectors` /
/// `wake_drains` no longer change section words — kept for call-site
/// stability. `link_body` false ⇒ bare `ret` (no wrela body in `code`).
pub fn build_checkpoint_and_vector_stub_ex(
    group: Option<&GroupServiceCtx>,
    _irq_vectors: &[IrqVectorEntry],
    _wake_drains: &[WakeDrainEntry],
    link_body: bool,
) -> CheckpointBlock {
    let has_deadline = group.is_some_and(|g| g.arena_capacity > 0);
    let emitted = crate::codegen::emit_checkpoint_service_trampoline(has_deadline, link_body);
    CheckpointBlock {
        words: emitted.words,
        checkpoint_service_word: emitted.checkpoint_service_word,
        deadline_poll_word: emitted.deadline_poll_word,
        has_deadline_poll: emitted.has_deadline_poll,
        relocs: emitted.relocs,
    }
}
/// plans/M10.md item F (decision 630): force-root one specialized
/// `rt_select_and_run <Actor>` body per mailbox root into `program.fns`.
/// Call after `RuntimeWiring::derive` and before the code section is
/// laid out.
/// M11 J: overwrite `__method_R_M` placeholders with specialized call stubs
/// (state in x0, stage in x8, bl method — decision 831). Alias
/// `rt_enqueue <Actor>` onto `__enqueue_N` trampolines (decision 834).
pub(super) fn inject_rt_enqueue_and_dispatch_fns(
    program: &mut CodegenProgram,
    wiring: &RuntimeWiring,
) -> Result<(), LayoutError> {
    let extras = crate::rtconfig::extras_from_tables(&wiring.tables).map_err(|e| {
        LayoutError::new(format!(
            "runtime wiring tables disagree with placement: {e}"
        ))
    })?;
    let mut flat = 0usize;
    for mb in &extras.mailboxes {
        for method in &mb.methods {
            let key = format!("__method_{flat}");
            program.fns.insert(
                key,
                crate::codegen::emit_method_call_stub(&method.key, mb.state),
            );
            flat += 1;
        }
    }
    for (i, name) in wiring.tables.enqueue_actors.iter().enumerate() {
        let tramp = format!("__enqueue_{i}");
        let Some(f) = program.fns.get(&tramp).cloned() else {
            // Named rejection (not the producer-bug prefix) — callers that
            // skip `lower_and_codegen_image` force-roots hit this; fuzz
            // treats it as Rejected layout, not a Bug panic.
            return Err(LayoutError::new(format!(
                "missing enqueue trampoline `{tramp}` after live codegen — lower with \
                 `ImageForceRootOpts::with_wiring` before `layout_test_image`"
            )));
        };
        program
            .fns
            .insert(crate::codegen::rt_enqueue_symbol(name), f);
    }
    Ok(())
}

/// M11 H (decisions 811 / 814) / M15 E (1053–1054): prepend floor-cat1 SP
/// install onto each generated `__wrela_secondary_entry_<core>` trampoline
/// (`core in 1..N_CORES`) and republish under `rt_secondary_core_entry <core>`
/// for VMM `core_entries`. Parameterized on `wiring.tables.cores` — no spare
/// pool to `CORE_SLOTS-1`.
pub(super) fn inject_rt_cross_core_fns(
    program: &mut CodegenProgram,
    wiring: &RuntimeWiring,
) -> Result<(), LayoutError> {
    for core in 1..wiring.tables.cores {
        let tramp = format!("__wrela_secondary_entry_{core}");
        let Some(mut f) = program.fns.get(&tramp).cloned() else {
            return Err(LayoutError::new(format!(
                "missing secondary trampoline `{tramp}` after live codegen — lower with \
                 `ImageForceRootOpts::with_wiring` before `layout_test_image`"
            )));
        };
        let sp = crate::codegen::emit_secondary_sp_install(core, wiring.tables.cores);
        let sp_len = sp.len();
        for r in &mut f.relocs {
            shift_reloc_words(r, sp_len);
        }
        let mut code = sp;
        code.append(&mut f.code);
        f.code = code;
        let key = crate::codegen::rt_secondary_core_entry_symbol(core);
        program.fns.insert(key, f);
    }
    Ok(())
}

fn shift_reloc_words(r: &mut crate::codegen::Reloc, delta: usize) {
    use crate::codegen::Reloc;
    match r {
        Reloc::Call { word, .. }
        | Reloc::AbortFixed { word }
        | Reloc::AbortVal { word }
        | Reloc::CheckpointService { word }
        | Reloc::TurnFrameAddr { word, .. }
        | Reloc::TurnIdImm { word, .. }
        | Reloc::TurnsBase { word }
        | Reloc::TurnStride { word }
        | Reloc::GroupArenaBase { word }
        | Reloc::IrqVector { word, .. }
        | Reloc::WakePending { word, .. }
        | Reloc::MailboxAddr { word, .. }
        | Reloc::RrCursor { word, .. }
        | Reloc::RingAddr { word, .. }
        | Reloc::DriverState { word, .. }
        | Reloc::DeviceRegsBase { word, .. }
        | Reloc::PoolBase { word, .. }
        | Reloc::PoolSlot { word, .. } => *word += delta,
        Reloc::Rodata { word_adrp, .. } => *word_adrp += delta,
    }
}

/// M11 H (decisions 812 / 814): overwrite `__boot_call_<i>` placeholders
/// with specialized init-call bodies (Relocs). `__wrela_rt_boot_init` is
/// already force-rooted / reinjected. Also alias `rt_boot_init 0` for any
/// lingering synthetic-key callers.
pub(super) fn inject_boot_init_fn(program: &mut CodegenProgram, wiring: &RuntimeWiring) {
    let to_arg = |a: &BootInitArg| -> crate::codegen::BootInitArgSpec {
        match a {
            BootInitArg::Word(w) => crate::codegen::BootInitArgSpec::Word(*w),
            BootInitArg::DeviceRegsBase(i) => crate::codegen::BootInitArgSpec::DeviceRegsBase(*i),
            BootInitArg::PoolBase(n) => crate::codegen::BootInitArgSpec::PoolBase(n.clone()),
            BootInitArg::OwnSlot {
                pool,
                index,
                slot_bytes,
            } => crate::codegen::BootInitArgSpec::OwnSlot {
                pool: pool.clone(),
                index: *index,
                slot_bytes: *slot_bytes,
            },
            BootInitArg::OwnHandleArray {
                pool,
                count,
                slot_bytes,
            } => crate::codegen::BootInitArgSpec::OwnHandleArray {
                pool: pool.clone(),
                count: *count,
                slot_bytes: *slot_bytes,
            },
        }
    };
    let to_call = |c: &BootInitCall| -> crate::codegen::BootInitCallSpec {
        crate::codegen::BootInitCallSpec {
            key: c.key.clone(),
            args: c.args.iter().map(to_arg).collect(),
            fallible: c.fallible,
            err_msg: c.err_msg,
        }
    };
    // Call order: drivers then actors (M7 H1 / decision 680).
    let mut call_i = 0usize;
    for ((d, &size), call) in wiring
        .tables
        .drivers
        .iter()
        .zip(wiring.driver_state_sizes.iter())
        .zip(wiring.driver_init_calls.iter())
    {
        let Some(c) = call else { continue };
        let slot = crate::codegen::BootInitSlotSpec {
            name: d.name.clone(),
            is_driver: true,
            state_size: size,
            init: Some(to_call(c)),
        };
        let key = format!("__boot_call_{call_i}");
        program
            .fns
            .insert(key, crate::codegen::emit_boot_init_call(&slot));
        call_i += 1;
    }
    for ((a, &size), call) in wiring
        .tables
        .actors
        .iter()
        .zip(wiring.state_sizes.iter())
        .zip(wiring.init_calls.iter())
    {
        let Some(c) = call else { continue };
        let slot = crate::codegen::BootInitSlotSpec {
            name: a.name.clone(),
            is_driver: false,
            state_size: size,
            init: Some(to_call(c)),
        };
        let key = format!("__boot_call_{call_i}");
        program
            .fns
            .insert(key, crate::codegen::emit_boot_init_call(&slot));
        call_i += 1;
    }
    assert_eq!(
        call_i, wiring.tables.n_boot_calls,
        "boot call stub count disagrees with tables.n_boot_calls"
    );
    // Alias synthetic key → wrela body for any residual bl_call_key sites.
    if let Some(f) = program.fns.get("__wrela_rt_boot_init").cloned() {
        program.fns.insert(crate::codegen::rt_boot_init_symbol(), f);
    }
}

/// M11 I / decision 823: overwrite `__irq_call_*` / `__wake_call_*` with
/// specialized `x0 = driver_state; bl handler/task` bodies.
pub(super) fn inject_checkpoint_irq_fns(program: &mut CodegenProgram, wiring: &RuntimeWiring) {
    for (i, (handler_key, driver_state)) in wiring.irq_calls.iter().enumerate() {
        let key = format!("__irq_call_{i}");
        let spec = crate::codegen::CheckpointIrqSpec {
            vector: wiring.tables.irq_vector_bits.get(i).copied().unwrap_or(0),
            handler_key: handler_key.clone(),
            driver_state: *driver_state,
        };
        program
            .fns
            .insert(key, crate::codegen::emit_checkpoint_irq_call(&spec));
    }
    for (i, (task_key, driver_state)) in wiring.wake_calls.iter().enumerate() {
        let key = format!("__wake_call_{i}");
        // M12 item D: pending bits live in WAKE.wake_pending[i]; the stub
        // only materializes x0 = driver_state for the @task body.
        let spec = crate::codegen::CheckpointWakeSpec {
            driver_state: *driver_state,
            wake_pending_off: 0,
            task_key: task_key.clone(),
        };
        program
            .fns
            .insert(key, crate::codegen::emit_checkpoint_wake_call(&spec));
    }
}
// M11 J: `build_rt_enqueue` / `build_rt_select_and_run` deleted with
// `emit_rt_enqueue` / `emit_rt_select_and_run`; enqueue/select oracles live
// in boot-*/expected/test.txt (decision 705).

pub const DEADLOCK_MSG: &str =
    "runtime deadlock: no turn is ready and the root turn has not completed";
// ===========================================================================
// plans/M5.md item E: the runtime test image's own harness.
//
// `layout_program`/`build_entry_stub`/`build_abort_stub` above are the
// *ordinary* image's entry/abort — `wrela build`/`--stage=report`'s own
// path, untouched by this item (the four pre-existing report goldens pin
// that placeholder entry's exact bytes; CLAUDE.md's "existing goldens must
// not move" wins over the module doc's older, pre-item-E speculation that
// item E would replace that body wholesale — it does not, for exactly this
// reason, recorded here instead of silently contradicting the paragraph
// above).
//
// Item E instead adds a **second**, wholly separate placement function,
// `layout_test_image`, used only by `wrela test`'s runtime tier (`bin/
// wrela.rs`): a real driver that calls every `@test(runtime)` fn in
// declaration order, prints each one's own report line over the console
// ring (decision 12), and an extended `__wrela_abort`/`__wrela_abort_val`
// that also print before halting — the exact obligation the pre-existing
// module doc named, now honestly split into its own code path instead of
// silently changing the shared one.
//
// ## The landing pad (the plan's own required mechanism, precisely)
//
// Every runtime test's call goes through one fixed continuation slot,
// `wrela_machine::machine_info::OFF_TEST_CONTINUATION`: right before the
// entry driver's `BL` to a test fn, it stores the address of *its own next
// step* (the point right after that test's own "ok" line and passed-
// counter increment — i.e. the top of the next test's own block, or the
// summary block for the last test) into that slot. `__wrela_abort`/
// `__wrela_abort_val`'s own bodies, extended by this item, print the
// `FAILED ...` line, increment the fail counter, then `LDR` that slot and
// `BR` to it directly — never `RET` — resuming the driver exactly at the
// point the aborted test's own "ok" line would have run, skipping it. This
// is what makes an abort *inside* an arbitrarily deep call chain (a test
// calling a helper calling a helper whose checked arithmetic overflows)
// still land back in the flat, straight-line entry driver: the slot is a
// fixed absolute guest address, not anything SP-relative, so it survives
// regardless of how many un-popped frames sit between the abort site and
// the driver's own stack depth at the point of the original `BL`.
//
// ## Why the entry driver needs no runtime loop at all
//
// Every `@test(runtime)` fn's name is known at compile time (the whole
// point of `wrela test`'s runtime tier: one fixed image per build), so the
// driver is fully unrolled — one straight-line block per test, in
// declaration order, then one summary block — never a real loop over a
// test list. This is why the landing pad's own continuation address can be
// a single fixed slot reused test-to-test: only one test is ever "in
// flight" at a time (M5 is synchronous, one vCPU), so there is never a
// need to remember more than the *current* test's own resume point.
//
// ## The M5-G adversarial-sweep find/fix: one descriptor per LINE, plus a
// static build-time bound (ledger `machine.console.transcript-pinned`)
//
// The original design (this module doc, pre-fix) span one
// `__wrela_ring_write` call — and so one consumed descriptor — per
// *piece* of a report line: a passing test alone cost 2 calls (the
// `test <name>: ` prefix, then `ok\n`); a failing test cost 3-4 (prefix,
// `FAILED `, the message, the newline; `__wrela_abort_val`'s own
// interpolated shape costs one more). With `console::QUEUE_SIZE` fixed
// at 16, a mere 7-8 `@test(runtime)` fns — ordinary, not adversarial —
// silently exhausted the queue mid-summary: `__wrela_ring_write` (below,
// pre-fix) treated a full queue as a silent no-op, so the transcript
// truncated (`"7 passed,"` then nothing) and `wrela test` failed with
// "the wrela VMM's own transcript is not well-formed". Found by the
// M5-G sweep's own adversarial pass, reproduced with a plain 8-test file
// of trivial `assert 1 == 1`-shaped tests.
//
// The fix makes 02 §12.2's "statically bounded output" literally true,
// in two parts:
//
// 1. **One descriptor per printed line, never per call.** Three new
//    fixed subroutines replace the old single `__wrela_ring_write`:
//    `__wrela_line_begin` (snapshots the data-bump cursor as the new
//    line's own start, `machine_info::OFF_LINE_START`), `__wrela_ring_
//    append` (copies bytes into the data region and advances the data-
//    bump cursor — *no* descriptor published, *no* doorbell rung), and
//    `__wrela_line_commit` (publishes exactly *one* descriptor spanning
//    `[OFF_LINE_START, current data-bump)`, then rings the doorbell).
//    Every report line — a test's `ok`/`FAILED ...` line, or the one
//    summary line — is now `line_begin`, one or more `ring_append`
//    calls, `line_commit`: exactly one descriptor, regardless of how
//    many pieces compose the line. `build_entry_driver`/
//    `__wrela_abort`/`__wrela_abort_val` (force-rooted wrela) all follow this
//    pattern; an abort continues (never restarts) the line the entry
//    driver's own prefix already began, so a `FAILED` line is still one
//    descriptor covering "test <name>: FAILED ...\n" in full.
// 2. **A static bound, checked at test-image *build* time, never at
//    runtime.** `check_transcript_bound` (below) computes the worst-case
//    line count (`runtime_tests.len() + 1`) and worst-case byte count
//    (every test's own worst-case line — the longer of its "ok" line or
//    a "FAILED" line whose message is over-approximated by the longest
//    string already interned in `program.rodata`, module doc on
//    `check_transcript_bound` itself has the exact formula) *before*
//    `layout_test_image` ever assembles a single instruction. Either
//    bound exceeded fails the build closed with a named diagnostic
//    (`LayoutError`, surfaced by `bin/wrela.rs`'s existing "could not lay
//    out the test image" wrapper) — never a truncated transcript.
//    `console::QUEUE_SIZE` also grew, 16 -> 256 (`wrela-machine`'s own
//    module doc has the ring-geometry consequences), so an ordinary
//    build with genuinely many tests still fits comfortably; the static
//    check is what makes the bound provably *safe* to rely on, not the
//    bigger number alone.
//
// `__wrela_ring_append` / `__wrela_line_commit` overflow used to `BRK`
// (`BRK_LINE_APPEND_OVERFLOW`/`BRK_LINE_COMMIT_OVERFLOW`). M10 B2–B5
// migrated those routines to force-rooted wrela and dissolved the BRKs
// into **status returns** (plans/M10.md decision 592) — not into
// ordinary array-bounds checks that would re-enter the abort/console
// path (decision 590). Once `check_transcript_bound` has passed, no
// line the generated driver ever composes can exceed either bound; a
// nonzero status from the compiled routines means the static check and
// the generated code have drifted — a real bug in this module, not a
// reachable guest condition.
//
// **Disclosed simplification of the split-ring contract (unchanged by
// this fix)**: this producer never reorders or skips a descriptor index,
// so nothing here ever populates `avail.ring[]` — the VMM's own console
// model (`wrela-vmm`) reads descriptors `0..avail.idx` directly by index,
// which is exactly what a real virtio consumer would get by walking
// `avail.ring[]` *because* this producer's own `avail.ring[i]` would
// always equal `i`. The `used` ring is never populated or read either
// (M5 has no completion tracking to negotiate: the guest never waits on
// it, and the transcript is read only after the guest halts, decision
// 12).

/// Every absolute guest address the harness subroutines below bake in via
/// `Asm::load_imm` — bundled so the exact same generator functions can be
/// re-run in a unit test against a host-mmap'd stand-in region instead of
/// the real (unmapped-in-a-test-process) machine addresses, rather than
/// hand-verifying the encoded bytes by eye (this module's own oracle for
/// the hand-assembled routines below: real execution on this machine's own
/// aarch64 CPU, `#[cfg(test)] mod harness_jit`, below).
///
/// Every consumer (`push_abort_tail` and the JIT self-tests) is
/// `#[cfg(test)]`, so this bundle is too — plans/M11-L-findings.md L-19.
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(super) struct HarnessAddrs {
    /// `machine_info::` field base — production: `machine_layout::MACHINE_INFO_BASE`.
    info_base: u64,
    /// `console::RING_BASE` (descriptor table + avail ring + doorbell).
    /// Retained after M10 B5 so this bundle still names the full console
    /// map; only `info_base` is still read by hand-asm abort/entry (console
    /// routines live in force-rooted wrela).
    #[allow(dead_code)]
    ring_base: u64,
    /// `console::DATA_BASE` (the byte buffers descriptors point into).
    #[allow(dead_code)]
    data_base: u64,
    /// `mmio::EXIT_MMIO_ADDR` — only the entry driver's own summary/halt
    /// tail uses this (never `ring_write`/`fmt_dec`/the abort stubs);
    /// unexercised by the JIT self-tests above (which never call the
    /// entry driver directly — a real MMIO trap needs a real VMM, item
    /// E's own boot golden is that routine's oracle).
    #[allow(dead_code)]
    exit_mmio_addr: u64,
}

#[cfg(test)]
impl HarnessAddrs {
    fn production() -> HarnessAddrs {
        HarnessAddrs {
            info_base: machine_layout::MACHINE_INFO_BASE,
            ring_base: console::RING_BASE,
            data_base: console::DATA_BASE,
            exit_mmio_addr: mmio::EXIT_MMIO_ADDR,
        }
    }
}

/// A tiny, self-contained word-list builder for the hand-assembled harness
/// routines below — distinct from `codegen.rs`'s `FnCtx` (which is built
/// around mwir's own per-instruction two-pass sizing scheme; this module's
/// code is never generated from mwir at all, just written directly, one
/// fixed shape per fn) but the same spirit: `start` is this fragment's own
/// absolute word index within the eventual combined "entry" section
/// (`__wrela_abort` / `__wrela_abort_val` / checkpoint stubs / optional
/// runtime glue / the entry driver — console begin/append/commit/fmt_dec
/// live in `code` as force-rooted wrela since M10 B2–B5). All of those
/// fragments are assembled into *one* combined word list, in that fixed
/// order, so every local call/branch between them is a directly computed
/// `BL`/`B`/`B.cond` — no `Reloc` needed for anything that stays inside
/// this one section. Only a `Reloc::Call` (an `@test(runtime)` fn,
/// elsewhere in the `code` section) or `Reloc::Rodata` (a literal string,
/// elsewhere in the `rodata` section) crosses out of it, and those reuse
/// the exact same `Reloc` variants/resolution already proven by item D.
pub(super) struct Asm {
    start: usize,
    pub(super) words: Vec<u32>,
    pub(super) relocs: Vec<Reloc>,
}

impl Asm {
    pub(super) fn new(start: usize) -> Asm {
        Asm {
            start,
            words: Vec::new(),
            relocs: Vec::new(),
        }
    }

    /// This fragment's own current absolute word index (its own `start`
    /// plus how many words it has emitted so far) — the address every
    /// local branch/call computes its delta against.
    fn abs(&self) -> usize {
        self.start + self.words.len()
    }

    pub(super) fn push(&mut self, w: u32) {
        self.words.push(w);
    }

    /// Materializes a 64-bit constant into `reg`, always exactly four
    /// words (`MOVZ` + three unconditional `MOVK`s) — `codegen.rs`'s own
    /// `load_imm`, duplicated here rather than threaded through as a
    /// shared helper (this module's fragments are not `FnCtx`s; CLAUDE.md's
    /// "prefer long obvious files" licenses the small duplication over a
    /// generic seam neither side otherwise needs).
    fn load_imm(&mut self, reg: u8, value: u64) {
        let h0 = (value & 0xFFFF) as u16;
        let h1 = ((value >> 16) & 0xFFFF) as u16;
        let h2 = ((value >> 32) & 0xFFFF) as u16;
        let h3 = ((value >> 48) & 0xFFFF) as u16;
        self.push(encode::enc_movz(reg, h0, 0, true));
        self.push(encode::enc_movk(reg, h1, 16, true));
        self.push(encode::enc_movk(reg, h2, 32, true));
        self.push(encode::enc_movk(reg, h3, 48, true));
    }

    /// Emits a placeholder `load_imm reg, #0` (four words), remembering
    /// where it started so `patch_load_imm_words` can later overwrite it
    /// with the real value once known — the entry driver's own forward
    /// reference for the landing pad's continuation address (module doc
    /// above): the value (the address of the *next* test's own setup)
    /// isn't known until after this test's whole pass-path block has been
    /// emitted.
    fn load_imm_placeholder(&mut self, reg: u8) -> usize {
        let m = self.words.len();
        self.load_imm(reg, 0);
        m
    }

    /// A `BL` to an `@test(runtime)` fn — a real `Reloc::Call` (the target
    /// lives in the `code` section, placed elsewhere, base unknown until
    /// the whole image is laid out) — `layout_test_image`'s own resolution
    /// loop patches it exactly like an ordinary compiled call.
    pub(super) fn bl_call_key(&mut self, key: &str) {
        let w = self.abs();
        self.push(encode::enc_bl(0));
        self.relocs.push(Reloc::Call {
            word: w,
            key: key.to_string(),
        });
    }

    /// A forward conditional branch whose target isn't known yet — mirrors
    /// `codegen.rs`'s own `emit_skip`/`patch_skip` (`SkipKind`), a small,
    /// deliberate duplicate for the same reason `load_imm` is (this
    /// module's fragments are not `FnCtx`s). Only the JIT self-tests below
    /// still build a fragment that needs one.
    #[cfg(test)]
    fn skip_placeholder(&mut self) -> usize {
        let w = self.words.len();
        self.push(0);
        w
    }

    #[cfg(test)]
    fn patch_cbnz(&mut self, marker: usize, reg: u8) {
        let target = self.abs();
        let this = self.start + marker;
        let delta = (target as i64 - this as i64) * 4;
        self.words[marker] = encode::enc_cbnz(reg, delta as i32, true);
    }
}

/// Appends one literal byte string to the growing rodata pool (shared
/// across the whole test image: `program.rodata`'s own already-interned
/// entries, plus every harness literal this module adds after them) and
/// returns its own byte offset within the eventual concatenated rodata
/// section — the same value `Reloc::Rodata::byte_offset` needs, computed
/// the identical way `codegen.rs`'s private `RodataPool::byte_offset`
/// does, just against a plain `(Vec<Vec<u8>>, running-total-cursor)` pair
/// instead of a `BTreeMap` index (no dedup here: every harness string is
/// already used at most a handful of times and interned at most once by
/// its own call site below, so content-addressing would add bookkeeping
/// this module does not need).
pub(super) fn append_rodata(
    rodata: &mut Vec<Vec<u8>>,
    cursor: &mut usize,
    bytes: Vec<u8>,
) -> usize {
    let off = *cursor;
    *cursor += bytes.len();
    rodata.push(bytes);
    off
}

/// The shared tail every abort body ends in: clear the re-entrancy latch
/// (plans/M10.md item B1 / decision 591), increment
/// `machine_info::OFF_TEST_FAILED` and long-jump to the landing pad's own
/// continuation address (module doc's own "landing pad" section) — never
/// `RET`. Clobbers `x9`/`x10`.
///
/// plans/M10.md item C: live abort print clears latch / increments fail in
/// wrela (`finish_abort`) then `bl`s `__wrela_abort_tail` (floor category 4
/// `LDR`+`BR`, decision 650). Kept for the unit-test latch probe and for
/// the hand-asm builders retained until C's delete commit.
#[cfg(test)]
pub(super) fn push_abort_tail(a: &mut Asm, addrs: &HarnessAddrs) {
    // Clear latch before the continuation jump so a later green test never
    // observes a stale "already aborting" bit from a prior abort.
    a.load_imm(9, addrs.info_base + mi::OFF_ABORT_LATCH);
    a.push(encode::enc_movz(10, 0, 0, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));
    a.load_imm(9, addrs.info_base + mi::OFF_TEST_FAILED);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    a.push(encode::enc_add_imm(10, 10, 1, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));
    a.load_imm(9, addrs.info_base + mi::OFF_TEST_CONTINUATION);
    a.push(encode::enc_ldr_x_imm(9, 9, 0));
    a.push(encode::enc_br(9));
}

/// plans/M10.md item C / decision 650: floor category 4 long-jump only —
/// materialize `OFF_TEST_CONTINUATION`, `LDR x9, [x9]`, `BR x9`. Overwrites
/// the compiled `__wrela_abort_tail` stub so test images never `ret` from
/// abort.
pub(super) fn build_abort_tail_codegen_fn() -> crate::codegen::CodegenFn {
    let addr = machine_layout::MACHINE_INFO_BASE + mi::OFF_TEST_CONTINUATION;
    use crate::cost::{CostRule, EmittedWord};
    let mut words: Vec<EmittedWord> = Vec::new();
    let bits = addr;
    let h0 = (bits & 0xFFFF) as u16;
    let h1 = ((bits >> 16) & 0xFFFF) as u16;
    let h2 = ((bits >> 32) & 0xFFFF) as u16;
    let h3 = ((bits >> 48) & 0xFFFF) as u16;
    words.push(EmittedWord::new(
        encode::enc_movz(9, h0, 0, true),
        format!("movz x9, #{h0:#x}"),
        CostRule::MovWide,
        Some(9),
        &[],
    ));
    words.push(EmittedWord::new(
        encode::enc_movk(9, h1, 16, true),
        format!("movk x9, #{h1:#x}, lsl #16"),
        CostRule::MovWide,
        Some(9),
        &[],
    ));
    words.push(EmittedWord::new(
        encode::enc_movk(9, h2, 32, true),
        format!("movk x9, #{h2:#x}, lsl #32"),
        CostRule::MovWide,
        Some(9),
        &[],
    ));
    words.push(EmittedWord::new(
        encode::enc_movk(9, h3, 48, true),
        format!("movk x9, #{h3:#x}, lsl #48"),
        CostRule::MovWide,
        Some(9),
        &[],
    ));
    words.push(EmittedWord::new(
        encode::enc_ldr_x_imm(9, 9, 0),
        "ldr x9, [x9]".to_string(),
        CostRule::Load,
        Some(9),
        &[9],
    ));
    words.push(EmittedWord::new(
        encode::enc_br(9),
        "br x9".to_string(),
        CostRule::Branch,
        None,
        &[9],
    ));
    crate::codegen::CodegenFn {
        frame_size: 0,
        code: words,
        relocs: Vec::new(),
    }
}

/// Replace the compiled `__wrela_abort_tail` stub with the floor long-jump.
/// Irreducible floor inject — runs after the single live CodegenProgram.
pub(super) fn install_abort_tail_floor(program: &mut CodegenProgram) -> Result<(), LayoutError> {
    if program.fns.contains_key("__wrela_abort") || program.fns.contains_key("__wrela_abort_val") {
        if !program.fns.contains_key("__wrela_abort_tail") {
            return Err(LayoutError::new(
                "internal error: force-rooted abort needs `__wrela_abort_tail` in the emit set",
            ));
        }
        program.fns.insert(
            "__wrela_abort_tail".to_string(),
            build_abort_tail_codegen_fn(),
        );
    }
    Ok(())
}

/// M11 K: free-turn index into `RT.turns` for an async `@test(runtime)` key.
fn free_turn_index(tables: &RuntimeTables, name: &str) -> Option<usize> {
    let messageable = tables
        .drivers
        .iter()
        .filter(|d| d.mailbox.is_some())
        .count();
    let base = tables.actors.len() + messageable;
    tables
        .free_turns
        .iter()
        .position(|(k, _)| k == name)
        .map(|k| base + k)
}

/// M11 K / decision 851: build test-runner facts for rtconfig reinject.
pub(super) fn test_runner_facts(
    runtime_tests: &[String],
    async_tests: &std::collections::BTreeSet<String>,
    tables: Option<&RuntimeTables>,
) -> Vec<crate::rtconfig::TestRunnerFact> {
    runtime_tests
        .iter()
        .map(|name| {
            let is_async = async_tests.contains(name);
            let turn_index = if is_async {
                tables.and_then(|t| free_turn_index(t, name)).unwrap_or(0)
            } else {
                0
            };
            crate::rtconfig::TestRunnerFact {
                name: name.clone(),
                is_async,
                turn_index,
            }
        })
        .collect()
}

/// M11 K: overwrite `__test_call_*` / `__test_prefix_*` with specialized bodies.
pub(super) fn inject_test_runner_fns(
    program: &mut CodegenProgram,
    tests: &[crate::rtconfig::TestRunnerFact],
    test_args: &BTreeMap<String, Vec<u64>>,
    rodata: &mut Vec<Vec<u8>>,
    rodata_cursor: &mut usize,
) {
    for (i, t) in tests.iter().enumerate() {
        let args = test_args.get(&t.name).cloned().unwrap_or_default();
        program.fns.insert(
            format!("__test_call_{i}"),
            crate::codegen::emit_test_call_stub(&t.name, &args),
        );
        let prefix = format!("test {}: ", t.name).into_bytes();
        let len = prefix.len() as u64;
        let off = append_rodata(rodata, rodata_cursor, prefix);
        program.fns.insert(
            format!("__test_prefix_{i}"),
            crate::codegen::emit_test_prefix_stub(off, len),
        );
    }
}

/// M11 K / decision 852: thin floor trampoline — SP install, arm
/// `OFF_TEST_CONTINUATION` at `__wrela_rt_primary_entry` (patched after
/// code placement), `BL __wrela_rt_primary_boot`, `brk`. Replaces
/// `build_entry_driver` (94 Unclassified).
///
/// Returns `(asm, continuation_load_imm_marker)`.
pub(super) fn build_primary_entry_trampoline(start: usize, n_cores: usize) -> (Asm, usize) {
    let mut a = Asm::new(start);
    let n = n_cores.max(1);
    let sp_top = machine_layout::core_stack_base_n(0, n) + machine_layout::CORE_STACK_SIZE;
    a.load_imm(9, sp_top);
    a.push(encode::enc_add_imm(31, 9, 0, true)); // mov sp, x9
    let cont_marker = a.load_imm_placeholder(9);
    a.load_imm(
        10,
        machine_layout::MACHINE_INFO_BASE + mi::OFF_TEST_CONTINUATION,
    );
    a.push(encode::enc_str_x_imm(9, 10, 0));
    a.bl_call_key("__wrela_rt_primary_boot");
    a.push(encode::enc_brk(0));
    (a, cont_marker)
}

/// Deleted by M11 item K (decision 850): `@test(runtime)` roots run as
/// ordinary supervised turns in `__wrela_rt_primary_entry` (stdlib
/// `runtime.wr`); the harness keeps only [`build_primary_entry_trampoline`].

/// Max decimal digits `__wrela_fmt_dec` ever writes, sign included:
/// `i64::MIN` (`-9223372036854775808`) and `u64::MAX`
/// (`18446744073709551615`) both render as exactly 20 ASCII characters —
/// the widest either the summary line's two counts or an `AbortVal`
/// line's interpolated value can ever be.
const MAX_DECIMAL_DIGITS: u64 = 20;

/// The worst-case transcript shape `check_transcript_bound` (below)
/// checks against `console::QUEUE_SIZE`/`console::DATA_SIZE` — module
/// doc's own "M5-G adversarial-sweep find/fix" section names this as the
/// static-bound half of the fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptBound {
    /// One descriptor per test's own line, plus one for the summary, plus
    /// two for the Lane 1 counter dump (`lane1 turns=…` / `lane1 hits=…`) —
    /// exact for the test+summary half; lane1 is a fixed +2.
    pub lines: u64,
    /// Every test's own worst-case line length, summed, plus the
    /// summary's own exact worst case (`worst_case_test_line_bytes`/
    /// `worst_case_summary_line_bytes`, below).
    pub worst_case_bytes: u64,
}

/// The longest single string already interned in `program.rodata` — the
/// documented over-approximation this module's static bound stands on
/// (module doc's own `check_transcript_bound` paragraph): rather than
/// tracing which abort site is actually reachable from which test (real
/// call-graph analysis this module does not have), every test's own
/// worst-case *message* is bounded by the single longest literal string
/// anywhere in the whole program's rodata pool, applied uniformly. Sound
/// (it can only over-count, never under-count, since every reachable
/// message is itself one of `program.rodata`'s own entries) and simple;
/// the plan's own explicit license for "the longest string in the whole
/// rodata pool ... documented over-approximation" over the alternative,
/// harder-to-get-right "each test's own longest reachable message".
fn longest_rodata_len(program: &CodegenProgram) -> u64 {
    program
        .rodata
        .iter()
        .map(|entry| entry.len() as u64)
        .max()
        .unwrap_or(0)
}

/// One test's own worst-case printed line length: `"test " + name + ": "`
/// (exact — both are known at compile time), then the *longer* of the two
/// shapes that line can ever take: `"ok\n"` (always exactly 3 bytes) or a
/// `FAILED` line. A `FAILED` line's own worst case covers *both*
/// `__wrela_abort` (one message, `"FAILED " + msg + "\n"`) and
/// `__wrela_abort_val` (`"FAILED " + prefix + <=20 digits + suffix +
/// "\n"`) uniformly, by allowing for *two* copies of the longest rodata
/// string (covers `AbortVal`'s prefix+suffix pair; strictly more than
/// `AbortFixed`'s one-string shape ever needs) plus the full 20-digit
/// interpolation width — an honest over-approximation applied to every
/// test regardless of which shape (if either) it can actually reach.
fn worst_case_test_line_bytes(name: &str, longest_msg: u64) -> u64 {
    let prefix_len = "test ".len() as u64 + name.len() as u64 + ": ".len() as u64;
    let ok_len = "ok\n".len() as u64;
    let failed_len =
        "FAILED ".len() as u64 + 2 * longest_msg + MAX_DECIMAL_DIGITS + "\n".len() as u64;
    prefix_len + ok_len.max(failed_len)
}

/// The summary line's own worst case, exact rather than over-approximated
/// (both counts are `u64`s, bounded by `MAX_DECIMAL_DIGITS` each):
/// `"<=20 digits> passed, <=20 digits> failed\n"`.
fn worst_case_summary_line_bytes() -> u64 {
    2 * MAX_DECIMAL_DIGITS + " passed, ".len() as u64 + " failed\n".len() as u64
}

/// Computes the exact worst-case shape `layout_test_image`'s own static
/// bound check (below) enforces — a pure function of `program`/
/// `runtime_tests` alone, callable standalone by unit tests without
/// building a whole image.
pub fn compute_transcript_bound(
    program: &CodegenProgram,
    runtime_tests: &[String],
) -> TranscriptBound {
    // The deadlock diagnostic (`DEADLOCK_MSG`) is a FAILED-line message
    // the *harness* interns, after this bound has already been checked —
    // so it must be accounted for here explicitly, not discovered via
    // `program.rodata` (the park-and-resume redesign's one addition to
    // this bound; still an over-approximation, never an undercount).
    let longest_msg = longest_rodata_len(program).max(DEADLOCK_MSG.len() as u64);
    let mut worst_case_bytes = worst_case_summary_line_bytes();
    for name in runtime_tests {
        worst_case_bytes += worst_case_test_line_bytes(name, longest_msg);
    }
    // Integrity Phase 2 Item I: two trailing `lane1 …` lines from
    // `__wrela_lane1_dump` (scalars + compact hits). Hits worst-case is
    // METHOD_CALL_POOL_COUNT `flat:count,` pairs (20+1+20+1 each) under
    // a fixed `lane1 hits=` prefix — over-approx, never under.
    // Item M: one `lane2 hits=` line only when `--block-count` is on
    // (otherwise dump is a no-op and must not inflate the default bound).
    const LANE1_LINES: u64 = 2;
    const LANE1_SCALAR_LINE_BYTES: u64 = 12 + 20 + 9 + 20 + 10 + 20 + 1; // turns/run_one/messages
    const LANE1_HITS_PREFIX: u64 = 11; // "lane1 hits="
    const LANE1_HIT_PAIR: u64 = 20 + 1 + 20 + 1; // flat:count,
    let lane1_hits_bytes =
        LANE1_HITS_PREFIX + (crate::rtconfig::METHOD_CALL_POOL_COUNT as u64) * LANE1_HIT_PAIR + 1;
    worst_case_bytes += LANE1_SCALAR_LINE_BYTES + lane1_hits_bytes;
    let mut lines = runtime_tests.len() as u64 + 1 + LANE1_LINES;
    if crate::codegen::block_count_enabled() {
        const LANE2_HITS_PREFIX: u64 = 11; // "lane2 hits="
        let lane2_hits_bytes = LANE2_HITS_PREFIX
            + (crate::rtconfig::BLOCK_BOUND_PRINT_PAIRS as u64) * LANE1_HIT_PAIR
            + 1;
        worst_case_bytes += lane2_hits_bytes;
        lines += 1;
    }
    TranscriptBound {
        lines,
        worst_case_bytes,
    }
}

/// The static bound check itself (module doc's own "M5-G adversarial-
/// sweep find/fix" section, part 2): `Err` — a real, named, build-time
/// diagnostic, never a runtime truncation — the moment either dimension
/// of `compute_transcript_bound`'s own result would overflow the
/// machine's fixed console geometry (`wrela-machine`'s own
/// `console::QUEUE_SIZE`/`console::DATA_SIZE`). Called first thing in
/// `layout_test_image`, below, before a single harness instruction is
/// ever assembled — 02 §12.2's "statically bounded output" made literally
/// true at build time, per this item's own task.
pub fn check_transcript_bound(
    program: &CodegenProgram,
    runtime_tests: &[String],
) -> Result<(), LayoutError> {
    let bound = compute_transcript_bound(program, runtime_tests);
    if bound.lines > console::QUEUE_SIZE || bound.worst_case_bytes > console::DATA_SIZE {
        return Err(LayoutError::new(format!(
            "this test image's worst-case transcript ({} byte(s) across {} line(s)) exceeds \
             the machine's console bound ({} byte(s) across {} line(s))",
            bound.worst_case_bytes,
            bound.lines,
            console::DATA_SIZE,
            console::QUEUE_SIZE
        )));
    }
    Ok(())
}

/// Places a codegen'd test-image program into the machine's fixed
/// contract, per module doc above: **one** combined "entry" section
/// (`__wrela_abort`, `__wrela_abort_val`, checkpoint stubs, optional
/// runtime glue, then the entry driver — console line_begin/append/
/// commit/fmt_dec live in `code` as force-rooted wrela, called via
/// `bl_call_key`), then `code` (every codegen'd fn, `@test(runtime)`
/// fns included — they are ordinary fns to `codegen.rs`, called by name
/// like any other), then `rodata` (`program.rodata`'s own already-interned
/// entries, followed by every harness literal this fn appends —
/// `append_rodata`, above). Every `Reloc` — the harness's own `Call`/
/// `Rodata` entries and every ordinary compiled fn's `Call`/`Rodata`/
/// `AbortFixed`/`AbortVal` — resolves through the identical `patch_bl`/
/// `patch_adrp_add` this file's item-D half already proved;
/// `AbortFixed`/`AbortVal` targets are force-rooted `__wrela_abort` /
/// `__wrela_abort_val` in the `code` section (plans/M10.md item C); the
/// floor category 4 long-jump is `__wrela_abort_tail` (hand-asm).
///
/// `Err` for a genuine internal inconsistency, **or** for a program whose
/// worst-case transcript provably cannot fit (`check_transcript_bound`,
/// called first, before anything else here) — module doc mirrors
/// `layout_program`'s own doc here for the former: an out-of-range
/// relocation, or a name in `runtime_tests` `codegen_program` never
/// produced (an internal invariant `bin/wrela.rs`'s own caller is expected
/// to have already checked via `TypedProgram::tests`, kept here anyway as
/// a real `Err` rather than a silent skip).
/// plans/M6.md item D: the three whole-build-closure facts needed to run
/// a real actor boot sequence (`compute_runtime_tables`'s own signature,
/// unchanged) — bundled so `layout_test_image`'s own signature grows by
/// exactly one `Option` parameter rather than three. `None` (the
/// overwhelming majority of today's corpus, and every pre-M6 golden)
/// keeps `layout_test_image` byte-identical to its pre-item-D behavior:
/// no `rtdata`, no boot sequence, no runtime-glue routines.
// `codegen_runtime_force_roots_with` / `merge_live_runtime` deleted:
// live rtconfig is swapped into the closure before the single
// `CodegenProgram` (`layout::lower_and_codegen_image`).

pub fn layout_test_image(
    program: &CodegenProgram,
    runtime_tests: &[String],
    // Which of `runtime_tests` are async (state machines with the
    // TURN_STATUS_* return ABI) — the entry driver's own scheduler loop
    // wraps exactly these (`build_entry_driver`'s own doc comment).
    async_tests: &std::collections::BTreeSet<String>,
    boot: Option<BootCtx>,
    // plans/M6.md decision 11b: every runtime test's own already-resolved
    // `Actor[T]` param values (build-time actor indices, `bin/wrela.rs`'s
    // own `resolve_runtime_test_args`), in declared param order — empty
    // for every test with no params (every pre-decision-11b test, byte-
    // identical).
    test_args: &BTreeMap<String, Vec<u64>>,
) -> Result<ImageLayout, LayoutError> {
    // `program` must already be the single live CodegenProgram
    // (`layout::lower_and_codegen_image`). Remaining work is named floor
    // inject only — not a second runtime.wr codegen.
    let mut program = program.clone();
    check_transcript_bound(&program, runtime_tests)?;

    let image_base = machine_layout::IMAGE_BASE;

    let mut wiring: Option<RuntimeWiring> = match &boot {
        Some(b) => RuntimeWiring::derive(b)?,
        None => None,
    };
    let mut rodata: Vec<Vec<u8>> = program.rodata.clone();
    let mut rodata_cursor: usize = rodata.iter().map(Vec::len).sum();

    // plans/M7.md item E1 / M10 H: intern fallible-`init` abort messages
    // before inject_boot_init_fn so emit_boot_init sees err_msg offsets.
    if let Some(w) = wiring.as_mut() {
        intern_fallible_init_abort_messages(w, &mut rodata, &mut rodata_cursor);
    }
    let tests = test_runner_facts(
        runtime_tests,
        async_tests,
        wiring.as_ref().map(|w| &w.tables),
    );
    install_abort_tail_floor(&mut program)?;
    if let Some(w) = wiring.as_ref() {
        // Named floor: ImageStatic stub specialization + symbol aliases.
        super::apply_resume_remaps(&mut program, w);
        inject_rt_enqueue_and_dispatch_fns(&mut program, w)?;
        inject_rt_cross_core_fns(&mut program, w)?;
        inject_boot_init_fn(&mut program, w);
        inject_checkpoint_irq_fns(&mut program, w);
    }
    inject_test_runner_fns(
        &mut program,
        &tests,
        test_args,
        &mut rodata,
        &mut rodata_cursor,
    );
    let program = &program;

    let mut code_words: Vec<u32> = Vec::new();
    let mut fn_word_base: BTreeMap<String, usize> = BTreeMap::new();
    for (key, f) in &program.fns {
        fn_word_base.insert(key.clone(), code_words.len());
        for ew in &f.code {
            code_words.push(ew.word);
        }
    }
    for name in runtime_tests {
        if !fn_word_base.contains_key(name) {
            return Err(LayoutError::new(format!(
                "internal error: runtime test `{name}` was never codegen'd"
            )));
        }
    }
    let runtime_tables: Option<RuntimeTables> = wiring.as_ref().map(|w| w.tables.clone());

    // plans/M10.md item C: abort print bodies are force-rooted wrela in
    // `code` (`__wrela_abort` / `__wrela_abort_val`); harness no longer
    // begins with hand-asm abort. Checkpoint is first.
    let checkpoint_start = 0usize;
    // plans/M6.md item E: `__wrela_checkpoint_service` + its own
    // `__wrela_vector0_service` sibling, the exact same real routine pair
    // `layout_program`'s own `build_checkpoint_and_vector_stub` builds for
    // the ordinary image path — placed once here, in the test harness's
    // own combined word section, since a runtime-test image's compiled
    // fns (`program.fns`, below) can carry `Reloc::CheckpointService`
    // exactly like `Reloc::AbortFixed`/`AbortVal`, and the entry driver's
    // own park-resume path (below) calls the service directly too.
    // plans/M6.md item F: built twice (shape-only, then with real `rtdata`
    // addresses), exactly like the runtime glue block below — the vector-0
    // body is now the real deadline scan whenever this build has a group
    // arena. `layout_program`'s own copy of this two-pass shape has the
    // full reasoning.
    let checkpoint_shape = group_service_shape(runtime_tables.as_ref());
    let (irq_shape, wake_shape) =
        checkpoint_irq_shape(boot.as_ref(), None, runtime_tables.as_ref());
    let link_cp_body = wiring.is_some();
    let checkpoint_block = build_checkpoint_and_vector_stub_ex(
        checkpoint_shape.as_ref(),
        &irq_shape,
        &wake_shape,
        link_cp_body,
    );
    let checkpoint_service_offset = checkpoint_block.checkpoint_service_word;
    let _has_deadline_poll = checkpoint_block.has_deadline_poll;
    let checkpoint_words_len = checkpoint_block.words.len();
    // `bl_call_key` records block-relative words when built at start=0;
    // shift them to harness-absolute for the shared reloc resolver.
    let checkpoint_relocs: Vec<Reloc> = checkpoint_block
        .relocs
        .into_iter()
        .map(|r| match r {
            Reloc::Call { word, key } => Reloc::Call {
                word: word + checkpoint_start,
                key,
            },
            other => other,
        })
        .collect();
    let checkpoint_asm = Asm {
        start: checkpoint_start,
        words: checkpoint_block.words,
        relocs: checkpoint_relocs,
    };
    // `__wrela_checkpoint_service`'s own harness-absolute word index (see
    // `build_checkpoint_and_vector_stub`'s doc: `__wrela_vector0_service`
    // sits first, so the section's own start is never the right target).
    let checkpoint_service_word = checkpoint_start + checkpoint_service_offset;

    // M11 K: harness is checkpoint + thin primary-entry trampoline.
    let entry_start = checkpoint_start + checkpoint_asm.words.len();
    let core_entry_starts: Vec<(usize, usize)> = Vec::new();
    let n_cores = wiring.as_ref().map(|w| w.tables.cores).unwrap_or(1).max(1);
    let (entry_asm, cont_marker) = build_primary_entry_trampoline(entry_start, n_cores);

    let mut harness_words: Vec<u32> = Vec::new();
    let mut harness_relocs: Vec<Reloc> = Vec::new();
    debug_assert_eq!(checkpoint_asm.start, harness_words.len());
    harness_relocs.extend(checkpoint_asm.relocs);
    harness_words.extend(checkpoint_asm.words);
    debug_assert_eq!(entry_start, harness_words.len());
    harness_relocs.extend(entry_asm.relocs.clone());
    harness_words.extend(entry_asm.words.clone());

    // --- place sections: entry(harness), code, rodata? -------------------
    let mut cursor = image_base;
    let harness_base = cursor;
    let harness_size = (harness_words.len() * 4) as u64;
    cursor += harness_size;

    cursor = round_up(cursor, 4);
    let code_base = cursor;
    let code_size = (code_words.len() * 4) as u64;
    cursor += code_size;

    // Arm OFF_TEST_CONTINUATION at `__wrela_rt_primary_entry` (decision 852).
    // Callers that go through `lower_and_codegen_image` with
    // `with_test_runner` seed this key; the fuzz lower lane's sync-only
    // codegen does not. Named (not `internal error:`) so a missing seed
    // is a fail-closed rejection rather than a census bug.
    let primary_entry_word = *fn_word_base
        .get("__wrela_rt_primary_entry")
        .ok_or_else(|| {
            LayoutError::new(
                "test image is missing `__wrela_rt_primary_entry` — lower with \
                 live runtime force-roots (`layout::lower_and_codegen_image` / \
                 `ImageForceRootOpts::with_test_runner`) before `layout_test_image`"
                    .to_string(),
            )
        })?;
    let primary_entry_addr = code_base + (primary_entry_word as u64) * 4;
    patch_load_imm_words(
        &mut harness_words,
        entry_start + cont_marker,
        primary_entry_addr,
    );

    let rodata_bytes: Vec<u8> = rodata.iter().flat_map(|e| e.iter().copied()).collect();
    let rodata_base = if rodata_bytes.is_empty() {
        None
    } else {
        cursor = round_up(cursor, 8);
        Some(cursor)
    };
    if rodata_base.is_some() {
        cursor += rodata_bytes.len() as u64;
    }

    // plans/M6.md item C, decision 3; M11 item C / 722: the `rtdata`
    // section at the fixed `RTDATA_BASE`, sized exactly `tables.total_bytes`
    // — mirroring `layout_program`'s identical convention.
    let rtdata_base = if let Some(tables) = runtime_tables.as_ref() {
        let base = steer_rtdata_base(cursor, tables)?;
        cursor = base + tables.total_bytes;
        Some(base)
    } else {
        None
    };

    // plans/M7.md item D: the same `pooldata` reservation `layout_program`
    // makes, for the same reason — a test image that declares a pool
    // reserves its backing too, or the two image flavors would emit
    // different memory maps for the same source (plans/M6.md item F/G's
    // own rule that the two flavors emit and reject identically). Only
    // the *backing* is resolved here; the placement itself waits until
    // the section table exists, so `place_pools` can check its own base
    // against every section already placed.
    let pool_backings = image_pool_backings(boot.as_ref())?;

    // plans/M7.md item H1: the same `devregs` reservation `layout_program`
    // makes, at the same point in the same order (after `rtdata`, before
    // `pooldata`). Placed *here*, before the runtime block's real-address
    // pass below, because that pass is what bakes each `DeviceCap[D]`
    // argument word into boot's own `init` call.
    let device_windows = device_register_windows(boot.as_ref())?;
    let placed_regs = place_device_regs(cursor, &device_windows);
    let device_regs: Vec<DeviceRegs> = match &placed_regs {
        Some((regs, _, _, end)) => {
            cursor = *end;
            regs.clone()
        }
        None => Vec::new(),
    };
    let pool_cursor = cursor;
    let _ = cursor;
    // Early pool bases for section placement only — boot_init resolves
    // PoolBase/PoolSlot via Reloc after `place_pools` (decision 683).
    let early_pools = place_pools_unchecked(pool_cursor, &pool_backings)
        .map(|(pools, _, _, _)| pools)
        .unwrap_or_default();
    let _ = &early_pools;

    // Now that `rtdata_base` is real, rebuild the checkpoint block's
    // address-dependent bytes. Boot_init / glue no longer sit in the
    // harness (M10 H / F2).
    let (glue_symbols, real_placement): (BTreeMap<String, usize>, Option<RuntimePlacement>) =
        if let Some(w) = &wiring {
            let tables = &w.tables;
            let real_base =
                rtdata_base.expect("rtdata reserved above whenever runtime_tables is Some");
            let placement = place_runtime_tables(real_base, tables);
            let (irq_real, wake_real) =
                checkpoint_irq_shape(boot.as_ref(), Some(&placement), Some(tables));
            if group_service_ctx(&placement, tables).is_some()
                || !irq_real.is_empty()
                || !wake_real.is_empty()
            {
                let real_cp = build_checkpoint_and_vector_stub_ex(
                    group_service_ctx(&placement, tables).as_ref(),
                    &irq_real,
                    &wake_real,
                    true,
                );
                if real_cp.words.len() != checkpoint_words_len {
                    return Err(LayoutError::new(
                        "internal error: the checkpoint block's own word count changed between \
                         its sizing pass and its real-address pass",
                    ));
                }
                for (i, word) in real_cp.words.iter().enumerate() {
                    harness_words[checkpoint_start + i] = *word;
                }
                // Match layout_program: real-pass Call relocs replace the
                // shape-pass ones (word indices are identical today; keep
                // the swap so a future length-preserving reshuffle cannot
                // leave an unpatched `enc_bl(0)` in the harness).
                harness_relocs.retain(|r| match r {
                    Reloc::Call { word, .. } => *word >= entry_start,
                    Reloc::Rodata { word_adrp, .. } => *word_adrp >= entry_start,
                    _ => true,
                });
                for r in real_cp.relocs {
                    match r {
                        Reloc::Call { word, key } => harness_relocs.push(Reloc::Call {
                            word: word + checkpoint_start,
                            key,
                        }),
                        other => harness_relocs.push(other),
                    }
                }
            }
            (BTreeMap::new(), Some(placement))
        } else {
            (BTreeMap::new(), None)
        };

    // Resolves a `Reloc::TurnFrameAddr` key to its real turn-area
    // address (`RuntimePlacement::turn_area_for`'s own rule) — an
    // internal error if no tables exist or the key was never sized.
    // Internal-error audit: unreachable from any source program, for the
    // identical reason `layout_program`'s own copy of this guard is —
    // a `TurnFrameAddr` exists only for an async fn, one async fn is
    // already enough for `compute_runtime_tables` to size a non-empty
    // table set, and `turn_area_for` partitions every key of the same
    // `async_frames` map codegen keyed its relocs by.
    let turn_area_addr = |key: &str| -> Result<u64, LayoutError> {
        let (Some(tables), Some(placement)) = (&runtime_tables, &real_placement) else {
            return Err(LayoutError::new(format!(
                "internal error: async fn `{key}` needs a turn area but this image has no \
                 runtime tables"
            )));
        };
        placement.turn_area_for(key, tables).ok_or_else(|| {
            LayoutError::new(format!(
                "internal error: async fn `{key}`'s own turn area was never sized"
            ))
        })
    };
    // plans/M10.md item 0c1: the same resolution one step earlier — the
    // `TurnId` itself, for a `Reloc::TurnIdImm`. Same unreachability
    // argument as `turn_area_addr` above, since `turn_area_for` *is*
    // `turn_id_for` scaled by the stride.
    let turn_id_imm = |key: &str| -> Result<u64, LayoutError> {
        let (Some(tables), Some(placement)) = (&runtime_tables, &real_placement) else {
            return Err(LayoutError::new(format!(
                "internal error: async fn `{key}` needs a turn id but this image has no \
                 runtime tables"
            )));
        };
        placement
            .turn_id_for(key, tables)
            .map(|id| id.get() as u64)
            .ok_or_else(|| {
                LayoutError::new(format!(
                    "internal error: async fn `{key}`'s own turn id was never sized"
                ))
            })
    };

    let mut sections = vec![
        Section {
            name: "entry",
            base: harness_base,
            size: harness_size,
        },
        Section {
            name: "code",
            base: code_base,
            size: code_size,
        },
    ];
    if let Some(rb) = rodata_base {
        sections.push(Section {
            name: "rodata",
            base: rb,
            size: rodata_bytes.len() as u64,
        });
    }
    if let (Some(rb), Some(tables)) = (rtdata_base, &runtime_tables) {
        sections.push(Section {
            name: "rtdata",
            base: rb,
            size: tables.total_bytes,
        });
    }
    if let Some((_, base, size, _)) = &placed_regs {
        sections.push(Section {
            name: "devregs",
            base: *base,
            size: *size,
        });
    }
    let placed_pools = place_pools(pool_cursor, &sections, &pool_backings)?;
    let pools: Vec<PoolPlacement> = match &placed_pools {
        Some((pools, base, size, _)) => {
            sections.push(Section {
                name: "pooldata",
                base: *base,
                size: *size,
            });
            pools.clone()
        }
        None => Vec::new(),
    };

    // --- resolve relocs ----------------------------------------------------
    // Internal-error audit: every guard in this harness loop is unreachable
    // from any source program. `Call` targets here are the entry driver's
    // own `@test(runtime)` roots (already checked against `fn_word_base` at
    // the top of this fn), a declared actor's `pub` method keys and its
    // zero-argument `init` — all read from the same module set `codegen`
    // compiled, with a method that fails to lower stopping the attempt one
    // layer up. `Rodata` cannot outrun its own pool (interning a literal is
    // what fills it). The last arm is structural: the harness builders in
    // this file emit no such reloc.
    for reloc in &harness_relocs {
        match reloc {
            Reloc::Call { word, key } => {
                let target_base = *fn_word_base.get(key).ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: harness call target `{key}` was never codegen'd"
                    ))
                })?;
                let this_addr = harness_base + (*word as u64) * 4;
                let target_addr = code_base + (target_base as u64) * 4;
                patch_bl(&mut harness_words, *word, this_addr, target_addr)?;
            }
            Reloc::Rodata {
                word_adrp,
                byte_offset,
            } => {
                let rb = rodata_base.ok_or_else(|| {
                    LayoutError::new(
                        "internal error: a harness Reloc::Rodata exists but the rodata section is empty",
                    )
                })?;
                let this_addr = harness_base + (*word_adrp as u64) * 4;
                let target_addr = rb + *byte_offset as u64;
                patch_adrp_add(&mut harness_words, *word_adrp, this_addr, target_addr)?;
            }
            Reloc::TurnFrameAddr { word, key } => {
                // The entry driver's own root-turn-area loads (the
                // scheduler loop reads `resume_ready` through them).
                let addr = turn_area_addr(key)?;
                patch_load_imm_words(&mut harness_words, *word, addr);
            }
            Reloc::AbortFixed { word } => {
                // plans/M10.md item C: fallible-init Err path in boot_init.
                let target_base = *fn_word_base.get("__wrela_abort").ok_or_else(|| {
                    LayoutError::new(
                        "internal error: harness AbortFixed needs `__wrela_abort` but it was \
                         never codegen'd"
                            .to_string(),
                    )
                })?;
                let this_addr = harness_base + (*word as u64) * 4;
                let target_addr = code_base + (target_base as u64) * 4;
                patch_bl(&mut harness_words, *word, this_addr, target_addr)?;
            }
            Reloc::AbortVal { word } => {
                let target_base = *fn_word_base.get("__wrela_abort_val").ok_or_else(|| {
                    LayoutError::new(
                        "internal error: harness AbortVal needs `__wrela_abort_val` but it was \
                         never codegen'd"
                            .to_string(),
                    )
                })?;
                let this_addr = harness_base + (*word as u64) * 4;
                let target_addr = code_base + (target_base as u64) * 4;
                patch_bl(&mut harness_words, *word, this_addr, target_addr)?;
            }
            Reloc::CheckpointService { .. }
            | Reloc::TurnIdImm { .. }
            | Reloc::TurnsBase { .. }
            | Reloc::TurnStride { .. }
            | Reloc::GroupArenaBase { .. }
            | Reloc::IrqVector { .. }
            | Reloc::WakePending { .. }
            | Reloc::MailboxAddr { .. }
            | Reloc::RrCursor { .. }
            | Reloc::RingAddr { .. }
            | Reloc::DriverState { .. }
            | Reloc::DeviceRegsBase { .. }
            | Reloc::PoolBase { .. }
            | Reloc::PoolSlot { .. } => {
                return Err(LayoutError::new(
                    "internal error: the harness section itself must never emit a \
                     CheckpointService/TurnIdImm/TurnsBase/TurnStride/\
                     GroupArenaBase/IrqVector/WakePending/MailboxAddr/RrCursor/RingAddr/\
                     DriverState/DeviceRegsBase/PoolBase/PoolSlot reloc",
                ));
            }
        }
    }
    for (key, f) in &program.fns {
        let base = fn_word_base[key];
        for reloc in &f.relocs {
            match reloc {
                Reloc::Call { word, key: target } => {
                    // plans/M6.md item D: an async `Send`/`Await{ActorCall}`
                    // op's own symbolic call target is a per-actor runtime
                    // glue routine (`glue_symbols`, harness-section-relative)
                    // rather than an ordinary `program.fns` entry
                    // (`fn_word_base`, code-section-relative) — checked
                    // second (`codegen::rt_enqueue_actor`'s doc records the
                    // one disclosed way the two naming schemes could ever
                    // collide, and why nothing enforces against it yet).
                    // The `else` arm is the audit's one genuinely
                    // user-reachable find — `unresolved_call_target`.
                    // plans/M8.md item C2 / M11 G: see `layout_program`'s twin —
                    // cross-core enqueue → `__wrela_xsend_<edge>`;
                    // `rt_xreply` → `__wrela_xreply_<edge>` (decision 804).
                    let redirect = resolve_cross_core_edge(key, target, wiring.as_ref())?;
                    let xreply = wiring.as_ref().and_then(|w| resolve_xreply_edge(target, w));
                    let target_owned: String =
                        redirect.or(xreply).unwrap_or_else(|| target.clone());
                    let target = target_owned.as_str();
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = if let Some(target_base) = fn_word_base.get(target) {
                        code_base + (*target_base as u64) * 4
                    } else if let Some(glue_word) = glue_symbols.get(target) {
                        harness_base + (*glue_word as u64) * 4
                    } else {
                        return Err(unresolved_call_target(
                            target,
                            boot.as_ref().map(|b| b.graph),
                        ));
                    };
                    patch_bl(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => {
                    let rb = rodata_base.ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::Rodata exists but the rodata section is empty",
                        )
                    })?;
                    let this_addr = code_base + ((base + word_adrp) * 4) as u64;
                    let target_addr = rb + *byte_offset as u64;
                    patch_adrp_add(&mut code_words, base + word_adrp, this_addr, target_addr)?;
                }
                Reloc::AbortFixed { word } => {
                    // plans/M10.md item C: resolve to force-rooted
                    // `__wrela_abort` in `code` (print + finish_abort +
                    // floor BR via `__wrela_abort_tail`).
                    let target_base = fn_word_base.get("__wrela_abort").ok_or_else(|| {
                        LayoutError::new(
                            "internal error: Reloc::AbortFixed needs `__wrela_abort` but it was \
                             never codegen'd"
                                .to_string(),
                        )
                    })?;
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = code_base + (*target_base as u64) * 4;
                    patch_bl(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::AbortVal { word } => {
                    let target_base = fn_word_base.get("__wrela_abort_val").ok_or_else(|| {
                        LayoutError::new(
                            "internal error: Reloc::AbortVal needs `__wrela_abort_val` but it was \
                             never codegen'd"
                                .to_string(),
                        )
                    })?;
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = code_base + (*target_base as u64) * 4;
                    patch_bl(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::CheckpointService { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = harness_base + (checkpoint_service_word as u64) * 4;
                    patch_bl(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::TurnFrameAddr { word, key: fn_key } => {
                    // The compiled async fn's own persistent-frame base
                    // load (its X_FRAME setup) — patched with its turn
                    // area's real `rtdata` address.
                    let addr = turn_area_addr(fn_key)?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::TurnIdImm { word, key: fn_key } => {
                    // plans/M10.md item 0c1: this turn's own `TurnId`, as
                    // the waker of every message it awaits and as the owner
                    // half of its `OFF_TURN_REPLY_SLOT` pair.
                    let id = turn_id_imm(fn_key)?;
                    patch_load_imm_words(&mut code_words, base + word, id);
                }
                Reloc::TurnsBase { word } => {
                    // plans/M10.md item 0c3, twin of `layout_program`'s arm.
                    let addr = real_placement
                        .as_ref()
                        .map(|p| p.turns_base)
                        .ok_or_else(|| LayoutError::new(turns_deref_needs_rtdata()))?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::TurnStride { word } => {
                    let stride = real_placement
                        .as_ref()
                        .map(|p| p.turn_stride)
                        .ok_or_else(|| LayoutError::new(turns_deref_needs_rtdata()))?;
                    patch_load_imm_words(&mut code_words, base + word, stride);
                }
                Reloc::GroupArenaBase { word } => {
                    let addr = real_placement
                        .as_ref()
                        .map(|p| p.group_arena)
                        .ok_or_else(|| {
                            LayoutError::new(
                                "internal error: a `with group` op needs the group arena but this \
                             image's runtime tables never sized one"
                                    .to_string(),
                            )
                        })?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::IrqVector { word, driver } => {
                    let vector = driver_irq_vector(boot.as_ref().map(|b| b.graph), driver)?;
                    patch_load_imm_words(&mut code_words, base + word, vector);
                }
                Reloc::WakePending { word, driver } => {
                    let (p, t) = match (real_placement.as_ref(), runtime_tables.as_ref()) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(wake_needs_rtdata(driver));
                        }
                    };
                    let addr = driver_wake_pending_addr(p, t, driver)?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                // M10 D / decision 614
                Reloc::MailboxAddr { word, actor, field } => {
                    let (p, t) = match (real_placement.as_ref(), runtime_tables.as_ref()) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(LayoutError::new(
                                "internal error: a Reloc::MailboxAddr exists but this image has \
                                 no runtime tables",
                            ));
                        }
                    };
                    let addrs = resolve_mailbox_actor_addrs(p, t, actor).ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::MailboxAddr names actor `{actor}`, which this \
                             image's runtime tables never placed a mailbox for"
                        ))
                    })?;
                    let addr = match field {
                        crate::codegen::MailboxField::Ring => addrs.ring,
                        crate::codegen::MailboxField::Head => addrs.head,
                        crate::codegen::MailboxField::Tail => addrs.tail,
                        crate::codegen::MailboxField::Count => addrs.count,
                        crate::codegen::MailboxField::State => addrs.state,
                        crate::codegen::MailboxField::Turn => addrs.turn,
                    };
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                // M10 E3 / decision 621
                Reloc::RrCursor { word, core } => {
                    let p = real_placement.as_ref().ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RrCursor exists but this image has no \
                             runtime placement",
                        )
                    })?;
                    let addr = p.rr_cursors.get(*core).copied().ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::RrCursor names core {core}, but this image \
                             only placed {} rr_cursor(s)",
                            p.rr_cursors.len()
                        ))
                    })?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::RingAddr {
                    word,
                    ring_index,
                    field,
                } => {
                    let p = real_placement.as_ref().ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RingAddr exists but this image has no \
                             runtime placement",
                        )
                    })?;
                    let addrs = p.rings.get(*ring_index).copied().ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::RingAddr names ring_index {ring_index}, but \
                             this image only placed {} ring(s)",
                            p.rings.len()
                        ))
                    })?;
                    let addr = match field {
                        crate::codegen::RingField::Ring => addrs.ring,
                        crate::codegen::RingField::Head => addrs.head,
                        crate::codegen::RingField::Tail => addrs.tail,
                        crate::codegen::RingField::Count => addrs.count,
                    };
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                // M10 H / decision 682
                Reloc::DriverState { word, driver } => {
                    let (p, t) = match (real_placement.as_ref(), runtime_tables.as_ref()) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(LayoutError::new(
                                "internal error: a Reloc::DriverState exists but this image has \
                                 no runtime tables",
                            ));
                        }
                    };
                    let addr = driver_state_addr(p, t, driver)?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                // M10 H / decision 683
                Reloc::DeviceRegsBase { word, device } => {
                    let addr = device_regs
                        .iter()
                        .find(|r| r.device == *device)
                        .map(|r| r.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::DeviceRegsBase names device#{device}, \
                                 which this image never placed"
                            ))
                        })?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::PoolBase { word, pool } => {
                    let addr = pools
                        .iter()
                        .find(|p| &p.backing.name == pool)
                        .map(|p| p.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::PoolBase names pool `{pool}`, which this \
                                 image never placed"
                            ))
                        })?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::PoolSlot {
                    word,
                    pool,
                    index,
                    slot_bytes,
                } => {
                    let base_addr = pools
                        .iter()
                        .find(|p| &p.backing.name == pool)
                        .map(|p| p.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::PoolSlot names pool `{pool}`, which this \
                                 image never placed"
                            ))
                        })?;
                    let addr = base_addr + *index * *slot_bytes;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
            }
        }
    }

    // --- serialize -----------------------------------------------------
    let mut blob = Vec::new();
    for w in &harness_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    pad_to(&mut blob, image_base, code_base);
    for w in &code_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    if let Some(rb) = rodata_base {
        pad_to(&mut blob, image_base, rb);
        blob.extend_from_slice(&rodata_bytes);
    }
    if let (Some(rb), Some(tables)) = (rtdata_base, &runtime_tables) {
        pad_to(&mut blob, image_base, rb);
        blob.resize(blob.len() + tables.total_bytes as usize, 0);
    }
    if let Some((_, base, size, _)) = &placed_regs {
        pad_to(&mut blob, image_base, *base);
        blob.resize(blob.len() + *size as usize, 0);
    }
    if let Some((_, base, size, _)) = &placed_pools {
        pad_to(&mut blob, image_base, *base);
        blob.resize(blob.len() + *size as usize, 0);
    }

    verify_section_sizes(&sections, image_base, blob.len() as u64)?;
    verify_pool_windows(&sections, &pools)?;
    verify_device_windows(&sections, &device_regs)?;
    // blk filled later — ring verify runs in attach_blk_report

    let irq_host_injects = build_irq_host_injects(boot.as_ref(), &device_regs);
    // M10 F2: secondary-core entries live in `code` (decision 633).
    let core_entries: Vec<(usize, u64)> = match (wiring.as_ref(), code_base) {
        (Some(w), cb) if w.tables.cores > 1 => (1..w.tables.cores)
            .filter_map(|core| {
                let key = crate::codegen::rt_secondary_core_entry_symbol(core);
                fn_word_base
                    .get(&key)
                    .map(|&word| (core, cb + (word as u64) * 4))
            })
            .collect(),
        _ => Vec::new(),
    };
    let _ = core_entry_starts;
    let cores = wiring.as_ref().map(|w| w.tables.cores).unwrap_or(1).max(1);
    Ok(ImageLayout {
        blob,
        entry: harness_base + (entry_start as u64) * 4,
        sections,
        // plans/M6.md item D: real at last for an actor-bearing test image
        // (`bin/wrela.rs::test_cmd` now passes a real `BootCtx` — the item-C
        // sub-note's own "staged, named work" is this commit).
        runtime: runtime_tables,
        pools,
        device_regs,
        blk: None, // filled by attach_blk_report after layout
        irq_host_injects,
        core_entries,
        cores,
        placed_statics: Vec::new(),
    })
}

/// plans/M10.md item F0: live word counts for hand-emitted A64 emitters
/// under the census reference configuration documented in
/// `emitted_a64_census`. `#[cfg(test)]` so production builds keep the
/// private builders private; the census unit test is the only caller.
#[cfg(test)]
pub(crate) fn emitted_a64_census_live_counts() -> std::collections::BTreeMap<&'static str, usize> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<&'static str, usize> = BTreeMap::new();
    let insert = |out: &mut BTreeMap<&'static str, usize>, name: &'static str, n: usize| {
        assert!(
            out.insert(name, n).is_none(),
            "duplicate census measure key {name}"
        );
    };

    // --- reference configuration (pinned; image-dependent emitters use this) ---
    // capacity=4, slot_size=32 (one arg word beyond the 16-byte header).
    // One select target, no child polls, no drain, no xreply arms.
    // Checkpoint: empty irq/wake (M6 path), no group arena.
    // Deadline scan/poll: arena_capacity=1, one turn area.
    // Boot init: one actor, state_size=8, no init calls.
    // Entry driver: zero runtime tests, cores=1, no boot_init, no rt_run_one.
    // Drain: core=0, one request lane + one reply lane, capacity=4.
    // Group child poll: one child at index 0.
    // Secondary core entry: core=1.
    // M11 J: enqueue/select REF ActorAddrs/RingAddrs measures deleted with emitters.

    // Floor / halt.
    {
        let mut w = Vec::new();
        push_halt(&mut w, 0);
        insert(&mut out, "push_halt", w.len());
    }
    insert(&mut out, "build_entry_stub", build_entry_stub().len());
    insert(
        &mut out,
        "build_abort_tail_codegen_fn",
        build_abort_tail_codegen_fn().code.len(),
    );

    // M10 G: checkpoint + deadline emitters measured in
    // codegen::emitted_a64_census_specialization_live_counts.
    // M10 M (sweep find L-11): harness `push_turn_addr_from_id` was dead
    // production code — its only caller was this measure function; the
    // live index→address rule is `codegen::push_turn_addr_from_id`.
    // Deleted, row removed from the census.

    // Helpers measured in isolation (delta on a fresh Asm).
    {
        let mut w = Vec::new();
        push_load_imm(&mut w, 9, 0x1234);
        insert(&mut out, "push_load_imm", w.len());
    }

    // M10 F/F2: hand-asm select / cross-core / ring_enqueue deleted.
    // M11 J: JIT `build_rt_enqueue` / `build_rt_select_and_run` deleted with emitters.
    // M10 H: emit_boot_init measured in codegen::emitted_a64_census_specialization_live_counts.
    // M11 K: build_entry_driver deleted; primary trampoline is floor.
    {
        let (asm, _) = build_primary_entry_trampoline(0, 1);
        insert(&mut out, "build_primary_entry_trampoline", asm.words.len());
    }
    // Test-only residue of item C (cfg(test) helper).
    {
        let addrs = HarnessAddrs::production();
        let mut a = Asm::new(0);
        push_abort_tail(&mut a, &addrs);
        insert(&mut out, "push_abort_tail", a.words.len());
    }

    out
}
// ===========================================================================
// Item E's own oracle for the remaining hand-assembled harness routines
// above (abort path — M10 item C migrates those; console fmt/append/
// begin/commit are force-rooted wrela since B2–B5): real execution, on
// this machine's own aarch64 CPU (every development/check host this
// project targets is either Apple Silicon or aarch64 Linux — CLAUDE.md's
// own machine — so this is never a cross-architecture emulation trick).
// No assembler exists to cross-check these bytes against (decision 5), so
// instead of hand-verifying each encoding by eye the way `encode.rs`'s
// own unit tests do for single instructions, this writes the generated
// words into an executable page and calls them as an ordinary
// `extern "C" fn` — the *behavior* is the oracle, exactly the same
// principle decision 5 already states for the VMM/`diff-eval` at the
// whole-image level, applied here one level down, to routines that touch
// only a host-mmap'd stand-in region (`HarnessAddrs`'s own
// test-vs-production split, module doc above) — never the real,
// unmapped-in-a-test-process `wrela_machine` constants.

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod harness_jit {
    use super::*;
    use std::ffi::c_void;

    unsafe extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
        fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
    }

    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const PROT_EXEC: i32 = 4;
    const MAP_PRIVATE: i32 = 0x0002;
    const MAP_ANON: i32 = 0x1000;

    /// A host page (or run of pages) holding real, callable machine code —
    /// written RW, then flipped to R-X before it is ever called (two
    /// separate `mmap`/`mprotect` steps, never simultaneously W+X, so this
    /// needs no `MAP_JIT`/hardened-runtime entitlement on macOS).
    struct ExecPage {
        ptr: *mut u8,
        len: usize,
    }

    impl ExecPage {
        fn new(words: &[u32]) -> ExecPage {
            let want = words.len() * 4;
            let len = want.div_ceil(4096) * 4096;
            unsafe {
                let p = mmap(
                    std::ptr::null_mut(),
                    len,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANON,
                    -1,
                    0,
                );
                assert!(!p.is_null() && (p as isize) != -1, "mmap failed");
                let bytes = p as *mut u8;
                for (i, w) in words.iter().enumerate() {
                    std::ptr::write_unaligned(bytes.add(i * 4) as *mut u32, *w);
                }
                let r = mprotect(p, len, PROT_READ | PROT_EXEC);
                assert_eq!(r, 0, "mprotect(R-X) failed");
                ExecPage { ptr: bytes, len }
            }
        }

        /// The `_at` family: identical shape to a two-arg AAPCS64 leaf call,
        /// but entering at `byte_offset` into this same page instead of its
        /// very first byte. M11 J deleted the enqueue/select combined-page
        /// oracles; `call2_at` remains for the abort-tail latch probe.
        #[allow(dead_code)]
        fn call0_at(&self, byte_offset: usize) -> u64 {
            assert!(byte_offset < self.len);
            let f: extern "C" fn() -> u64 =
                unsafe { std::mem::transmute(self.ptr.add(byte_offset)) };
            f()
        }

        fn call2_at(&self, byte_offset: usize, a0: u64, a1: u64) -> u64 {
            assert!(byte_offset < self.len);
            let f: extern "C" fn(u64, u64) -> u64 =
                unsafe { std::mem::transmute(self.ptr.add(byte_offset)) };
            f(a0, a1)
        }

        #[allow(dead_code)]
        fn call5_at(&self, byte_offset: usize, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
            assert!(byte_offset < self.len);
            let f: extern "C" fn(u64, u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(self.ptr.add(byte_offset)) };
            f(a0, a1, a2, a3, a4)
        }
    }

    impl Drop for ExecPage {
        fn drop(&mut self) {
            unsafe {
                munmap(self.ptr as *mut c_void, self.len);
            }
        }
    }

    /// A host-mmap'd stand-in for one page of "guest RAM" a harness
    /// routine reads/writes via absolute addresses baked in at code-gen
    /// time (`HarnessAddrs`) — real memory a test can inspect afterward,
    /// standing in for `console::RING_BASE`/`DATA_BASE`/
    /// `machine_layout::MACHINE_INFO_BASE` without needing those literal,
    /// unmapped-in-this-process addresses to exist.
    struct HostRam {
        ptr: *mut u8,
        len: usize,
    }

    impl HostRam {
        fn new(len: usize) -> HostRam {
            let len = len.div_ceil(4096) * 4096;
            unsafe {
                let p = mmap(
                    std::ptr::null_mut(),
                    len,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANON,
                    -1,
                    0,
                );
                assert!(!p.is_null() && (p as isize) != -1, "mmap failed");
                // Pre-fault every page by writing it once, up front —
                // exactly what the real VMM does to the whole guest DRAM
                // region before ever starting the vCPU ("zeroes the
                // declared reservations", 06-machine.md §3): an untouched
                // anonymous mapping's pages are backed by the shared,
                // read-only system zero page until first written, and a
                // write performed by *JIT'd code running under this same
                // process* was observed (empirically, chasing down a real
                // test failure) to not reliably fault such a page in on
                // this host — pre-touching here removes that difference
                // between this test harness and the VMM's own always-
                // zeroed-first memory model, rather than working around a
                // JIT-only artifact this module's actual production code
                // path never hits.
                std::ptr::write_bytes(p as *mut u8, 0, len);
                HostRam {
                    ptr: p as *mut u8,
                    len,
                }
            }
        }

        fn base(&self) -> u64 {
            self.ptr as u64
        }

        fn read_u64(&self, off: u64) -> u64 {
            assert!((off as usize) + 8 <= self.len);
            unsafe { std::ptr::read_unaligned(self.ptr.add(off as usize) as *const u64) }
        }

        /// Plans/M6.md item C: pre-seeding a ring's `count`/`head`/`tail`
        /// (ring-full/FIFO-order test setup) needs writes, not just reads
        /// — every M5-era harness test only ever *reads* `HostRam` after
        /// letting generated code write it; this item's own tests are the
        /// first to need the reverse.
        fn write_u64(&self, off: u64, value: u64) {
            assert!((off as usize) + 8 <= self.len);
            unsafe { std::ptr::write_unaligned(self.ptr.add(off as usize) as *mut u64, value) }
        }
    }

    impl Drop for HostRam {
        fn drop(&mut self) {
            unsafe {
                munmap(self.ptr as *mut c_void, self.len);
            }
        }
    }

    /// plans/M10.md item B1 / decision 591: a second entry into
    /// `__wrela_abort` with the latch already set skips printing and
    /// lands at the shared halt/continuation tail. First entry sets the
    /// latch and clears it in that same tail.
    ///
    /// plans/M10.md item C: print bodies are wrela; this probe keeps the
    /// latch+tail shape in hand-asm so the unit test does not need a
    /// full force-rooted runtime JIT.
    #[test]
    fn abort_reentrancy_latch_skips_print_on_second_entry() {
        let ram = HostRam::new(4096 * 4);
        let addrs = HarnessAddrs {
            info_base: ram.base(),
            ring_base: ram.base() + 4096,
            data_base: ram.base() + 4096 * 2,
            exit_mmio_addr: 0,
        };
        // Combined page: [ret stub][latch probe][landing ret]
        // Probe mirrors `__wrela_abort`'s latch check + `finish_abort` /
        // `push_abort_tail` without calling console helpers.
        let ret = encode::enc_ret(30);
        let abort_start = 1usize;
        let mut a = Asm::new(abort_start);
        a.load_imm(9, addrs.info_base + mi::OFF_ABORT_LATCH);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        let reenter = a.skip_placeholder(); // cbnz x10 → shared_tail
        a.push(encode::enc_movz(10, 1, 0, true));
        a.push(encode::enc_str_x_imm(10, 9, 0));
        // (no print — first-entry body would call console helpers here)
        a.patch_cbnz(reenter, 10);
        push_abort_tail(&mut a, &addrs);
        let mut words = vec![ret];
        words.extend(a.words.iter().copied());
        let land_off = words.len();
        words.push(ret);
        let page = ExecPage::new(&words);
        let land_addr = page.ptr as u64 + (land_off * 4) as u64;
        ram.write_u64(mi::OFF_TEST_CONTINUATION, land_addr);

        // Pre-set latch: second-entry path. Must not touch ring bump,
        // must clear latch, must increment failed.
        ram.write_u64(mi::OFF_ABORT_LATCH, 1);
        ram.write_u64(mi::OFF_TEST_FAILED, 0);
        ram.write_u64(mi::OFF_RING_DATA_BUMP, 42);
        let _ = page.call2_at(abort_start * 4, 0, 0);
        assert_eq!(
            ram.read_u64(mi::OFF_ABORT_LATCH),
            0,
            "tail clears the latch"
        );
        assert_eq!(ram.read_u64(mi::OFF_TEST_FAILED), 1);
        assert_eq!(
            ram.read_u64(mi::OFF_RING_DATA_BUMP),
            42,
            "re-entry must skip console print (ring bump unchanged)"
        );
    }

    /// plans/M10.md item C / decision 650: the six floor words every abort
    /// long-jumps through were pinned by nothing at all. `dump --stage=asm`
    /// shows this symbol as the wrela `__wrela_abort_tail` stub's compiled
    /// `ret` (the stub exists only so `finish_abort`'s call type-checks),
    /// because the overwrite happens later, in `install_abort_tail_floor`
    /// after the single live CodegenProgram. So the bytes that actually run appeared
    /// in no golden and no test: dropping `install_abort_tail_floor` would
    /// have left every test image *returning* from abort instead of
    /// long-jumping to the landing pad, and the whole suite would still
    /// have been green.
    ///
    /// The address is checked by decoding it back out of the `MOVZ`/`MOVK`
    /// stream rather than by recomputing the same halfword arithmetic the
    /// builder uses — an inverse, so a wrong constant fails instead of
    /// agreeing with itself.
    #[test]
    fn install_abort_tail_floor_replaces_the_stub_with_the_long_jump() {
        let stub = crate::codegen::CodegenFn {
            frame_size: 16,
            code: vec![crate::cost::EmittedWord::new(
                encode::enc_ret(30),
                "ret".to_string(),
                crate::cost::CostRule::Branch,
                None,
                &[30],
            )],
            relocs: Vec::new(),
        };
        let mut fns = BTreeMap::new();
        fns.insert("__wrela_abort".to_string(), stub.clone());
        fns.insert("__wrela_abort_tail".to_string(), stub.clone());
        let mut program = CodegenProgram {
            fns,
            rodata: Vec::new(),
        };
        install_abort_tail_floor(&mut program).expect("install");

        let tail = &program.fns["__wrela_abort_tail"];
        assert_eq!(tail.frame_size, 0, "the floor tail owns no frame");
        assert!(tail.relocs.is_empty(), "the floor tail relocates nothing");
        assert_eq!(tail.code.len(), 6, "four immediate words, LDR, BR");
        assert!(
            !tail.code.iter().any(|ew| ew.word == encode::enc_ret(30)),
            "the compiled `ret` stub must be gone — an abort that returns \
             would resume the failing test instead of landing"
        );
        assert_eq!(tail.code[4].word, encode::enc_ldr_x_imm(9, 9, 0));
        assert_eq!(tail.code[5].word, encode::enc_br(9));

        // MOVZ/MOVK: imm16 at bits[20:5], shift/16 at bits[22:21].
        let mut addr = 0u64;
        for ew in &tail.code[..4] {
            let imm16 = ((ew.word >> 5) & 0xFFFF) as u64;
            let shift = ((ew.word >> 21) & 0x3) * 16;
            addr |= imm16 << shift;
        }
        assert_eq!(
            addr,
            machine_layout::MACHINE_INFO_BASE + mi::OFF_TEST_CONTINUATION,
            "the tail must load the landing pad's own continuation slot"
        );

        // Fail-closed: abort bodies present without the tail in the emit
        // set is an internal inconsistency, never a silent skip.
        let mut orphan = CodegenProgram {
            fns: BTreeMap::new(),
            rodata: Vec::new(),
        };
        orphan.fns.insert("__wrela_abort_val".to_string(), stub);
        let err = install_abort_tail_floor(&mut orphan).unwrap_err();
        assert!(
            err.message.contains("`__wrela_abort_tail` in the emit set"),
            "got: {}",
            err.message
        );
    }

    // --- rt_enqueue / rt_select_and_run / rt_run_one ------------------------
    // M11 J: enqueue/select JIT oracles deleted with emit_rt_enqueue /
    // emit_rt_select_and_run — boot transcripts are the oracle (decision 705).
    // M11 F: RR oracle moved to boot goldens — `emit_rt_run_one` deleted;
    // `__wrela_rt_run_one` is generic wrela over SCHED + match ladders.
}
