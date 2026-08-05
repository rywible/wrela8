use std::collections::BTreeMap;

use wrela_machine::machine_info as mi;
use wrela_machine::{console, layout as machine_layout, machine_info, mmio};

use super::{
    BootCtx, BootInitArg, BootInitCall, CheckpointBlock, DeviceRegs, GroupServiceCtx, ImageLayout,
    IrqVectorEntry, LayoutError, PoolPlacement, RuntimePlacement, RuntimeTables, RuntimeWiring,
    Section, WakeDrainEntry, build_irq_host_injects, checkpoint_irq_shape, device_register_windows,
    driver_irq_vector, driver_state_addr, driver_wake_pending_addr, group_service_ctx,
    group_service_shape, image_pool_backings, intern_fallible_init_abort_messages, pad_to,
    patch_adr, patch_adrp_add, patch_bl, patch_load_imm_words, place_device_regs, place_pools,
    place_pools_unchecked, place_runtime_tables, resolve_cross_core_edge,
    resolve_mailbox_actor_addrs, resolve_xreply_edge, round_up, steer_rtdata_base,
    turns_deref_needs_rtdata, unresolved_call_target, verify_device_windows, verify_pool_windows,
    verify_section_sizes, wake_needs_rtdata,
};
use crate::codegen::{CodegenProgram, Reloc};
use crate::encode;

const X_SP: u8 = 31;
const SCRATCH_A: u8 = 9;
const SCRATCH_B: u8 = 10;
const SCRATCH_C: u8 = 11;
#[allow(dead_code)]
const X_ZR: u8 = 31;

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
    let sp_top = machine_layout::core_stack_base_n(0, 1) + machine_layout::CORE_STACK_SIZE;
    push_load_imm(&mut words, SCRATCH_A, sp_top as i64);
    words.push(encode::enc_add_imm(X_SP, SCRATCH_A, 0, true));
    push_halt(&mut words, EXIT_CODE_NO_RUNTIME);
    words
}
pub fn build_checkpoint_and_vector_stub(group: Option<&GroupServiceCtx>) -> CheckpointBlock {
    build_checkpoint_and_vector_stub_ex(group, &[], &[], true)
}

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
        | Reloc::PoolSlot { word, .. }
        | Reloc::RodataAdr { word, .. } => *word += delta,
        Reloc::Rodata { word_adrp, .. } => *word_adrp += delta,
    }
}

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
            BootInitArg::WordArray(words) => {
                crate::codegen::BootInitArgSpec::WordArray(words.clone())
            }
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
    if let Some(f) = program.fns.get("__wrela_rt_boot_init").cloned() {
        program.fns.insert(crate::codegen::rt_boot_init_symbol(), f);
    }
}

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

pub const DEADLOCK_MSG: &str =
    "runtime deadlock: no turn is ready and the root turn has not completed";

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(super) struct HarnessAddrs {
    info_base: u64,
    #[allow(dead_code)]
    ring_base: u64,
    #[allow(dead_code)]
    data_base: u64,
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

    fn abs(&self) -> usize {
        self.start + self.words.len()
    }

    pub(super) fn push(&mut self, w: u32) {
        self.words.push(w);
    }

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

    fn load_imm_placeholder(&mut self, reg: u8) -> usize {
        let m = self.words.len();
        self.load_imm(reg, 0);
        m
    }

    pub(super) fn bl_call_key(&mut self, key: &str) {
        let w = self.abs();
        self.push(encode::enc_bl(0));
        self.relocs.push(Reloc::Call {
            word: w,
            key: key.to_string(),
        });
    }

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

#[cfg(test)]
pub(super) fn push_abort_tail(a: &mut Asm, addrs: &HarnessAddrs) {
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

pub(super) fn build_primary_entry_trampoline(start: usize, n_cores: usize) -> (Asm, usize) {
    let mut a = Asm::new(start);
    let n = n_cores.max(1);
    let sp_top = machine_layout::core_stack_base_n(0, n) + machine_layout::CORE_STACK_SIZE;
    a.load_imm(9, sp_top);
    a.push(encode::enc_add_imm(31, 9, 0, true));
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

const MAX_DECIMAL_DIGITS: u64 = 20;

const fn decimal_digits(max: u64) -> u64 {
    let mut n = 1;
    let mut v = max;
    while v >= 10 {
        v /= 10;
        n += 1;
    }
    n
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptBound {
    pub lines: u64,
    pub worst_case_bytes: u64,
}

fn longest_rodata_len(program: &CodegenProgram) -> u64 {
    program
        .rodata
        .iter()
        .map(|entry| entry.len() as u64)
        .max()
        .unwrap_or(0)
}

fn worst_case_test_line_bytes(name: &str, longest_msg: u64) -> u64 {
    let prefix_len = "test ".len() as u64 + name.len() as u64 + ": ".len() as u64;
    let ok_len = "ok\n".len() as u64;
    let failed_len =
        "FAILED ".len() as u64 + 2 * longest_msg + MAX_DECIMAL_DIGITS + "\n".len() as u64;
    prefix_len + ok_len.max(failed_len)
}

fn worst_case_summary_line_bytes() -> u64 {
    2 * MAX_DECIMAL_DIGITS + " passed, ".len() as u64 + " failed\n".len() as u64
}

pub const fn lane1_pair_bytes() -> u64 {
    let n = crate::rtconfig::METHOD_CALL_POOL_COUNT as u64;
    let id_digits = decimal_digits(n - 1);
    n * (id_digits + 1 + MAX_DECIMAL_DIGITS + 1)
}

pub const fn lane2_pair_bytes() -> u64 {
    let pairs = crate::rtconfig::BLOCK_BOUND_PRINT_PAIRS as u64;
    let id_digits = decimal_digits(crate::rtconfig::BLOCK_POOL_COUNT as u64 - 1);
    pairs * (id_digits + 1 + MAX_DECIMAL_DIGITS + 1)
}

pub const fn lane2_marker_bytes() -> u64 {
    " truncated=".len() as u64 + decimal_digits(crate::rtconfig::BLOCK_POOL_COUNT as u64)
}

pub fn compute_transcript_bound(
    program: &CodegenProgram,
    runtime_tests: &[String],
) -> TranscriptBound {
    let longest_msg = longest_rodata_len(program).max(DEADLOCK_MSG.len() as u64);
    let mut worst_case_bytes = worst_case_summary_line_bytes();
    for name in runtime_tests {
        worst_case_bytes += worst_case_test_line_bytes(name, longest_msg);
    }
    const LANE1_LINES: u64 = 3;
    const LANE1_SCALAR_LINE_BYTES: u64 = 12 + 20 + 9 + 20 + 10 + 20 + 1;
    const LANE1_QUIESCE_LINE_BYTES: u64 = 21 + 1;
    const LANE1_HITS_PREFIX: u64 = 11;
    worst_case_bytes += LANE1_SCALAR_LINE_BYTES
        + LANE1_QUIESCE_LINE_BYTES
        + LANE1_HITS_PREFIX
        + lane1_pair_bytes()
        + 1;
    let mut lines = runtime_tests.len() as u64 + 1 + LANE1_LINES;
    if crate::codegen::block_count_enabled() {
        const LANE2_HITS_PREFIX: u64 = 11;
        worst_case_bytes += LANE2_HITS_PREFIX + lane2_pair_bytes() + lane2_marker_bytes() + 1;
        lines += 1;
    }
    TranscriptBound {
        lines,
        worst_case_bytes,
    }
}

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

pub fn layout_test_image(
    program: &CodegenProgram,
    runtime_tests: &[String],
    async_tests: &std::collections::BTreeSet<String>,
    boot: Option<BootCtx>,
    test_args: &BTreeMap<String, Vec<u64>>,
) -> Result<ImageLayout, LayoutError> {
    let mut program = program.clone();
    check_transcript_bound(&program, runtime_tests)?;

    let image_base = machine_layout::IMAGE_BASE;

    let mut wiring: Option<RuntimeWiring> = match &boot {
        Some(b) => RuntimeWiring::derive(b)?,
        None => None,
    };
    let mut rodata: Vec<Vec<u8>> = program.rodata.clone();
    let mut rodata_cursor: usize = rodata.iter().map(Vec::len).sum();

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

    let checkpoint_start = 0usize;
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
    let checkpoint_service_word = checkpoint_start + checkpoint_service_offset;

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

    let mut cursor = image_base;
    let harness_base = cursor;
    let harness_size = (harness_words.len() * 4) as u64;
    cursor += harness_size;

    cursor = round_up(cursor, 4);
    let code_base = cursor;
    let code_size = (code_words.len() * 4) as u64;
    cursor += code_size;

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

    let rtdata_base = if let Some(tables) = runtime_tables.as_ref() {
        let base = steer_rtdata_base(cursor, tables)?;
        cursor = base + tables.total_bytes;
        Some(base)
    } else {
        None
    };

    let pool_backings = image_pool_backings(boot.as_ref())?;

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
    let early_pools = place_pools_unchecked(pool_cursor, &pool_backings)
        .map(|(pools, _, _, _)| pools)
        .unwrap_or_default();
    let _ = &early_pools;

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
            Reloc::RodataAdr { word, byte_offset } => {
                let rb = rodata_base.ok_or_else(|| {
                    LayoutError::new(
                        "internal error: a harness Reloc::RodataAdr exists but the rodata \
                         section is empty",
                    )
                })?;
                let this_addr = harness_base + (*word as u64) * 4;
                let target_addr = rb + *byte_offset as u64;
                patch_adr(&mut harness_words, *word, this_addr, target_addr)?;
            }
            Reloc::TurnFrameAddr { word, key } => {
                let addr = turn_area_addr(key)?;
                patch_load_imm_words(&mut harness_words, *word, addr);
            }
            Reloc::AbortFixed { word } => {
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
                Reloc::RodataAdr { word, byte_offset } => {
                    let rb = rodata_base.ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RodataAdr exists but the rodata section is \
                             empty",
                        )
                    })?;
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = rb + *byte_offset as u64;
                    patch_adr(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::AbortFixed { word } => {
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
                    let addr = turn_area_addr(fn_key)?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::TurnIdImm { word, key: fn_key } => {
                    let id = turn_id_imm(fn_key)?;
                    patch_load_imm_words(&mut code_words, base + word, id);
                }
                Reloc::TurnsBase { word } => {
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

    let irq_host_injects = build_irq_host_injects(boot.as_ref(), &device_regs);
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
        linked: None,
        entry: harness_base + (entry_start as u64) * 4,
        sections,
        runtime: runtime_tables,
        pools,
        device_regs,
        blk: None,
        irq_host_injects,
        core_entries,
        cores,
        placed_statics: Vec::new(),
        renderers: Vec::new(),
    })
}

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

    {
        let mut w = Vec::new();
        push_load_imm(&mut w, 9, 0x1234);
        insert(&mut out, "push_load_imm", w.len());
    }

    {
        let (asm, _) = build_primary_entry_trampoline(0, 1);
        insert(&mut out, "build_primary_entry_trampoline", asm.words.len());
    }
    {
        let addrs = HarnessAddrs::production();
        let mut a = Asm::new(0);
        push_abort_tail(&mut a, &addrs);
        insert(&mut out, "push_abort_tail", a.words.len());
    }

    out
}

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

    #[test]
    fn abort_reentrancy_latch_skips_print_on_second_entry() {
        let ram = HostRam::new(4096 * 4);
        let addrs = HarnessAddrs {
            info_base: ram.base(),
            ring_base: ram.base() + 4096,
            data_base: ram.base() + 4096 * 2,
            exit_mmio_addr: 0,
        };
        let ret = encode::enc_ret(30);
        let abort_start = 1usize;
        let mut a = Asm::new(abort_start);
        a.load_imm(9, addrs.info_base + mi::OFF_ABORT_LATCH);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        let reenter = a.skip_placeholder();
        a.push(encode::enc_movz(10, 1, 0, true));
        a.push(encode::enc_str_x_imm(10, 9, 0));
        a.patch_cbnz(reenter, 10);
        push_abort_tail(&mut a, &addrs);
        let mut words = vec![ret];
        words.extend(a.words.iter().copied());
        let land_off = words.len();
        words.push(ret);
        let page = ExecPage::new(&words);
        let land_addr = page.ptr as u64 + (land_off * 4) as u64;
        ram.write_u64(mi::OFF_TEST_CONTINUATION, land_addr);

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
            ..Default::default()
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

        let mut orphan = CodegenProgram {
            fns: BTreeMap::new(),
            rodata: Vec::new(),
            ..Default::default()
        };
        orphan.fns.insert("__wrela_abort_val".to_string(), stub);
        let err = install_abort_tail_floor(&mut orphan).unwrap_err();
        assert!(
            err.message.contains("`__wrela_abort_tail` in the emit set"),
            "got: {}",
            err.message
        );
    }
}
