use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use crate::eval::value::{self, Value};
use crate::lower_queue::{self, QueueSink};
use crate::lower_shared;
use crate::mwir::{self, Inst, MwirFn, MwirProgram, Temp};
use crate::sema::bodies::{self, InstKind};
use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, TestKind, TypedCallArg, TypedDeferBody, TypedEnum, TypedExpr, TypedExprKind,
    TypedFn, TypedForIter, TypedInstantiation, TypedPattern, TypedPatternKind, TypedProgram,
    TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::{Type, TypeArg};
use crate::syntax::ast::{self, AccessMode, BinOp};

thread_local! {
    static BOUNDS_ELIDE: Cell<bool> = const { Cell::new(false) };
}

pub fn set_bounds_elide(enabled: bool) {
    BOUNDS_ELIDE.with(|c| c.set(enabled));
}

pub(crate) fn bounds_elide() -> bool {
    BOUNDS_ELIDE.with(|c| c.get())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowerError {
    pub message: String,
}

impl LowerError {
    fn unimplemented(construct: impl Into<String>) -> LowerError {
        LowerError {
            message: format!("lowering {} not implemented yet", construct.into()),
        }
    }

    fn named(message: impl Into<String>) -> LowerError {
        LowerError {
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> LowerError {
        LowerError {
            message: format!("internal error: {}", message.into()),
        }
    }
}

type LEnv = Vec<BTreeMap<String, Temp>>;

#[derive(Debug, Clone, Default)]
pub struct LowerOpts {
    pub emit_comptime_tests: bool,
    pub only: Option<BTreeSet<String>>,
}

fn is_host_only_comptime_test(program: &TypedProgram, name: &str, opts: &LowerOpts) -> bool {
    if opts.emit_comptime_tests {
        return false;
    }
    program
        .tests
        .iter()
        .any(|t| t.name == name && t.kind == TestKind::Comptime)
}

fn is_host_only_fn(program: &TypedProgram, key: &str, f: &TypedFn, opts: &LowerOpts) -> bool {
    if program.image_fn.as_deref() == Some(key) {
        return true;
    }
    if f.is_layout_assert {
        return true;
    }
    is_host_only_comptime_test(program, key, opts)
}

pub fn guest_reachable_keys(program: &TypedProgram, opts: &LowerOpts) -> BTreeSet<String> {
    guest_reachable_keys_over(&[program], opts)
}

pub fn guest_reachable_keys_closure(
    programs: &BTreeMap<String, TypedProgram>,
    opts: &LowerOpts,
) -> BTreeSet<String> {
    let progs: Vec<&TypedProgram> = programs.values().collect();
    guest_reachable_keys_over(&progs, opts)
}

pub fn guest_reachable_keys_closure_from_roots(
    programs: &BTreeMap<String, TypedProgram>,
    opts: &LowerOpts,
    roots: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    let progs: Vec<&TypedProgram> = programs.values().collect();
    let mut work = roots.into_iter().collect::<BTreeSet<_>>();
    seed_runtime_force_roots(&progs, &mut work);
    guest_reachable_keys_from_work(&progs, opts, work, false)
}

pub const RUNTIME_FORCE_ROOT_KEYS: &[&str] = &[
    "__wrela_runtime_probe",
    "__wrela_line_begin",
    "__wrela_line_commit",
    "__wrela_fmt_dec",
    "__wrela_console_append_line_buf",
    "__wrela_console_append_bytes",
    "__wrela_abort",
    "__wrela_abort_val",
];

pub(crate) const PIXELS_DISPLAY_RAW_FN: &str = "__wrela_pixels_p8_submit";
pub(crate) const PIXELS_DISPLAY_DRIVER_FN: &str = "__wrela_pixels_display_submit_and_wait";

pub const RUNTIME_WIRING_FORCE_ROOT_KEYS: &[&str] = &[
    "__wrela_deadline_poll",
    "__wrela_deadline_scan",
    "__wrela_rt_run_one",
    "__wrela_child_poll",
    "__wrela_select_count",
    "__wrela_try_select",
    "__wrela_select_root",
    "__wrela_rt_select",
    "__wrela_try_drain",
    "__wrela_rt_drain",
    "__wrela_rt_xsend",
    "__wrela_rt_xreply",
    "__wrela_resume_child",
    "__wrela_child_turn_index",
    "__wrela_child_slot",
    "__wrela_child_store_result",
    "__wrela_try_enqueue",
    "__wrela_enqueue_root",
    "__wrela_rt_enqueue",
    "__wrela_enqueue_local",
    "__wrela_call_method",
    "__wrela_deliver_reply",
    "__wrela_invoke_xreply",
    "__wrela_mb_capacity",
    "__wrela_mb_slot_words",
    "__wrela_mb_turn_index",
    "__wrela_mb_core",
    "__wrela_mb_state",
    "__wrela_mb_has_lineage",
    "__wrela_mb_method_count",
    "__wrela_mb_get_head",
    "__wrela_mb_set_head",
    "__wrela_mb_get_tail",
    "__wrela_mb_set_tail",
    "__wrela_mb_get_count",
    "__wrela_mb_set_count",
    "__wrela_mb_load_word",
    "__wrela_mb_store_word",
    "__wrela_method_suspends",
    "__wrela_method_is_aggregate",
    "__wrela_ring_capacity",
    "__wrela_ring_slot_words",
    "__wrela_ring_dst_core",
    "__wrela_ring_src_core",
    "__wrela_ring_target_handle",
    "__wrela_drain_reply_count",
    "__wrela_drain_reply_edge",
    "__wrela_drain_request_count",
    "__wrela_drain_request_edge",
    "__wrela_xsend_edge",
    "__wrela_xreply_edge",
    "__wrela_rt_boot_init",
    "__wrela_rt_secondary_entry",
    "__wrela_init_nwords",
    "__wrela_init_store_word",
    "__wrela_boot_call",
    "__wrela_vector0",
    "__wrela_rt_checkpoint",
    "__wrela_irq_mask",
    "__wrela_irq_invoke",
    "__wrela_wake_invoke",
    "__wrela_lane1_method_flat",
    "__wrela_lane1_record_method",
    "__wrela_block_hit",
];

pub const RUNTIME_TEST_FORCE_ROOT_KEYS: &[&str] = &[
    "__wrela_rt_primary_boot",
    "__wrela_rt_primary_entry",
    "__wrela_rt_summary_and_halt",
    "__wrela_append_ok_literal",
    "__wrela_append_passed_comma_literal",
    "__wrela_append_failed_tail_literal",
    "__wrela_append_deadlock_literal",
    "__wrela_abort_deadlock",
    "__wrela_test_call",
    "__wrela_test_append_prefix",
    "__wrela_test_suspends",
    "__wrela_test_turn_index",
    "__wrela_lane1_dump",
    "__wrela_lane1_append_u64",
    "__wrela_lane1_sum_turns",
    "__wrela_lane1_sum_run_one",
    "__wrela_lane1_sum_messages",
    "__wrela_lane1_sum_method_hits",
    "__wrela_lane2_dump",
    "__wrela_lane2_exit",
    "__wrela_quiesce_before_halt",
    "__wrela_secondaries_idle",
    "__wrela_lane1_quiesce_timeout_line",
];

#[derive(Debug, Clone, Default)]
pub struct ImageForceRootOpts {
    pub with_wiring: bool,
    pub with_test_runner: bool,
    pub n_tests: usize,
    pub n_boot_calls: usize,
    pub n_irq_calls: usize,
    pub n_wake_calls: usize,
    pub n_cores: usize,
    pub with_pixels: bool,
    pub pixels_rooted_functions: BTreeSet<String>,
}

pub fn seed_image_force_roots(
    only: &mut BTreeSet<String>,
    programs: &BTreeMap<String, TypedProgram>,
    opts: ImageForceRootOpts,
) {
    if opts.with_pixels {
        only.extend(opts.pixels_rooted_functions.iter().cloned());
    }
    let need_scheduler = opts.with_wiring || opts.with_test_runner;
    if need_scheduler {
        for key in RUNTIME_WIRING_FORCE_ROOT_KEYS {
            only.insert((*key).to_string());
        }
        for i in 0..crate::rtconfig::RING_POOL_COUNT {
            only.insert(format!("__wrela_xsend_{i}"));
            only.insert(format!("__wrela_xreply_{i}"));
        }
        for i in 0..crate::rtconfig::ENQUEUE_STUB_COUNT {
            only.insert(format!("__enqueue_{i}"));
        }
        let n_cores = opts.n_cores.max(1);
        for core in 1..n_cores {
            only.insert(format!("__wrela_secondary_entry_{core}"));
        }
        if n_cores > 1 {
            only.insert("__wrela_secondary_entry_body".to_string());
        }
    }
    if opts.with_wiring {
        for i in 0..opts.n_boot_calls {
            only.insert(format!("__boot_call_{i}"));
        }
        for i in 0..opts.n_irq_calls {
            only.insert(format!("__irq_call_{i}"));
        }
        for i in 0..opts.n_wake_calls {
            only.insert(format!("__wake_call_{i}"));
        }
    }
    if opts.with_test_runner {
        for key in RUNTIME_TEST_FORCE_ROOT_KEYS {
            only.insert((*key).to_string());
        }
        for i in 0..opts.n_tests {
            only.insert(format!("__test_call_{i}"));
            only.insert(format!("__test_prefix_{i}"));
        }
    }
    if !need_scheduler {
        return;
    }
    for typed in programs.values() {
        for name in typed.fns.keys().chain(typed.imported.fns.keys()) {
            if name.starts_with("__resume_")
                || name.starts_with("__method_")
                || name.starts_with("__enqueue_")
                || name.starts_with("__wrela_xsend_")
                || name.starts_with("__wrela_xreply_")
                || name.starts_with("__irq_call_")
                || name.starts_with("__wake_call_")
                || name.starts_with("__boot_call_")
                || name.starts_with("__select_")
                || name.starts_with("__wrela_secondary_entry_")
                || (opts.with_test_runner
                    && (name.starts_with("__test_call_") || name.starts_with("__test_prefix_")))
            {
                only.insert(name.clone());
            }
        }
    }
}

pub(crate) fn validate_image_force_roots(
    programs: &BTreeMap<String, TypedProgram>,
    roots: &BTreeSet<String>,
) -> Result<(), String> {
    let opts = LowerOpts {
        emit_comptime_tests: false,
        only: None,
    };
    let available = programs
        .values()
        .flat_map(|program| all_candidate_keys(program, &opts))
        .collect::<BTreeSet<_>>();
    if let Some(missing) = roots.iter().find(|root| !available.contains(*root)) {
        return Err(format!(
            "pixels::glue: generated force root `{missing}` does not resolve in the typed closure"
        ));
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn missing_image_force_root_fails_closed() {
    let programs = BTreeMap::<String, TypedProgram>::new();
    let roots = BTreeSet::from(["__missing_pixels_root".to_string()]);
    let error = validate_image_force_roots(&programs, &roots).unwrap_err();
    assert!(error.contains("__missing_pixels_root"));
}

fn guest_reachable_keys_over(programs: &[&TypedProgram], opts: &LowerOpts) -> BTreeSet<String> {
    let mut work: BTreeSet<String> = BTreeSet::new();
    for p in programs {
        seed_entry_points(p, opts, &mut work);
    }
    seed_runtime_force_roots(programs, &mut work);
    guest_reachable_keys_from_work(programs, opts, work, true)
}

fn seed_runtime_force_roots(programs: &[&TypedProgram], work: &mut BTreeSet<String>) {
    for key in RUNTIME_FORCE_ROOT_KEYS {
        if programs.iter().any(|p| lookup_typed_fn(p, key).is_some()) {
            work.insert((*key).to_string());
        }
    }
}

fn guest_reachable_keys_from_work(
    programs: &[&TypedProgram],
    opts: &LowerOpts,
    mut work: BTreeSet<String>,
    fall_back_to_all: bool,
) -> BTreeSet<String> {
    if work.is_empty() && fall_back_to_all {
        for p in programs {
            for key in all_candidate_keys(p, opts) {
                work.insert(key);
            }
        }
        return work;
    }
    let mut reachable = BTreeSet::new();
    while let Some(key) = work.pop_first() {
        if !reachable.insert(key.clone()) {
            continue;
        }
        if key == PIXELS_DISPLAY_RAW_FN {
            work.insert(PIXELS_DISPLAY_DRIVER_FN.to_string());
        }
        for p in programs {
            if let Some(f) = lookup_typed_fn(p, &key) {
                if is_host_only_fn(p, &key, f, opts) {
                    continue;
                }
                collect_callees_from_fn(p, f, &mut work);
            }
        }
    }
    reachable.retain(|key| {
        programs.iter().all(|p| match lookup_typed_fn(p, key) {
            Some(f) => !is_host_only_fn(p, key, f, opts),
            None => true,
        })
    });
    reachable
}

fn seed_entry_points(program: &TypedProgram, opts: &LowerOpts, out: &mut BTreeSet<String>) {
    for t in &program.tests {
        match t.kind {
            TestKind::Runtime | TestKind::Exhaustive => {
                out.insert(t.name.clone());
            }
            TestKind::Comptime if opts.emit_comptime_tests => {
                out.insert(t.name.clone());
            }
            TestKind::Comptime => {}
        }
    }
    for (name, f) in program
        .fns
        .iter()
        .map(|(name, function)| (name, function))
        .chain(
            program
                .imported
                .fns
                .iter()
                .map(|(name, function)| (name, function.as_ref())),
        )
    {
        let sealed_pixels_helper = name.starts_with("__wrela_pixels_p7_worker_job_")
            || ((name.starts_with("Renderer[") || name.starts_with("struct:Renderer["))
                && name.contains(".__bootstrap_worker_path_"));
        if f.is_async && !sealed_pixels_helper && !is_host_only_fn(program, name, f, opts) {
            out.insert(name.clone());
        }
    }
    seed_struct_entries(&program.structs, out);
    for (name, structure) in &program.imported.structs {
        if name != "Renderer" && name != "RendererWorker" {
            seed_one_struct(name, structure, out);
        }
    }
    for (ikey, inst) in program
        .instantiations
        .iter()
        .chain(program.imported.instantiations.iter())
    {
        if let TypedInstantiation::Struct(s) = inst {
            // Renderer workers are sealed compiler-generated implementation
            // actors. The image force-root pass selects exactly the workers
            // placed for this image; treating every type-layout
            // instantiation as a guest entry point would retain idle workers
            // for all machine cores.
            if ikey.starts_with("RendererWorker") || ikey.starts_with("struct:RendererWorker") {
                continue;
            }
            seed_one_struct(ikey, s, out);
        }
    }
}

fn seed_struct_entries(structs: &BTreeMap<String, TypedStruct>, out: &mut BTreeSet<String>) {
    for (sname, s) in structs {
        // Renderer workers are compiler-placed implementation actors. A
        // source image cannot instantiate the unspecialized generic actor,
        // and Pixels images force-root exactly the sealed specializations
        // they need. Treating the generic declaration as an ordinary public
        // actor entry point retains the entire sweep in unrelated images.
        if sname == "Renderer" || sname == "RendererWorker" {
            continue;
        }
        seed_one_struct(sname, s, out);
    }
}

fn seed_one_struct(key_prefix: &str, s: &TypedStruct, out: &mut BTreeSet<String>) {
    if !(s.is_actor || s.is_driver) {
        return;
    }
    if s.init.is_some() {
        out.insert(format!("{key_prefix}.init"));
    }
    for (member, f) in &s.methods {
        if s.is_driver || f.is_pub || f.is_task {
            out.insert(format!("{key_prefix}.{member}"));
        }
    }
}

fn all_candidate_keys(program: &TypedProgram, opts: &LowerOpts) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (name, f) in &program.fns {
        if !is_host_only_fn(program, name, f, opts) {
            out.insert(name.clone());
        }
    }
    for (name, f) in &program.imported.fns {
        if !is_host_only_fn(program, name, f, opts) {
            out.insert(name.clone());
        }
    }
    add_struct_member_keys(&program.structs, &mut out);
    for (name, structure) in &program.imported.structs {
        add_one_struct_member_keys(name, structure, &mut out);
    }
    add_enum_member_keys(&program.enums, &mut out);
    for (name, enumeration) in &program.imported.enums {
        add_one_enum_member_keys(name, enumeration, &mut out);
    }
    for (ikey, inst) in program
        .instantiations
        .iter()
        .chain(program.imported.instantiations.iter())
    {
        match inst {
            TypedInstantiation::Fn(f) => {
                if !f.is_layout_assert {
                    out.insert(ikey.clone());
                }
            }
            TypedInstantiation::Struct(s) => add_one_struct_member_keys(ikey, s, &mut out),
            TypedInstantiation::Enum(_) => {}
        }
    }
    out
}

fn add_struct_member_keys(structs: &BTreeMap<String, TypedStruct>, out: &mut BTreeSet<String>) {
    for (sname, s) in structs {
        add_one_struct_member_keys(sname, s, out);
    }
}

fn add_one_struct_member_keys(key_prefix: &str, s: &TypedStruct, out: &mut BTreeSet<String>) {
    for member in s.methods.keys().chain(s.assoc_fns.keys()) {
        out.insert(format!("{key_prefix}.{member}"));
    }
    if s.init.is_some() {
        out.insert(format!("{key_prefix}.init"));
    }
}

fn add_enum_member_keys(enums: &BTreeMap<String, TypedEnum>, out: &mut BTreeSet<String>) {
    for (ename, e) in enums {
        add_one_enum_member_keys(ename, e, out);
    }
}

fn add_one_enum_member_keys(key_prefix: &str, enumeration: &TypedEnum, out: &mut BTreeSet<String>) {
    for member in enumeration
        .methods
        .keys()
        .chain(enumeration.assoc_fns.keys())
    {
        out.insert(format!("{key_prefix}.{member}"));
    }
}

fn lookup_typed_fn<'a>(program: &'a TypedProgram, key: &str) -> Option<&'a TypedFn> {
    if let Some(f) = program.fns.get(key).or_else(|| {
        program
            .imported
            .fns
            .get(key)
            .map(|function| function.as_ref())
    }) {
        return Some(f);
    }
    if let Some((owner, member)) = key.split_once('.') {
        if let Some(s) = program.structs.get(owner).or_else(|| {
            program
                .imported
                .structs
                .get(owner)
                .map(|value| value.as_ref())
        }) {
            if member == "init" {
                return s.init.as_ref();
            }
            if let Some(f) = s.methods.get(member).or_else(|| s.assoc_fns.get(member)) {
                return Some(f);
            }
        }
        if let Some(e) = program.enums.get(owner).or_else(|| {
            program
                .imported
                .enums
                .get(owner)
                .map(|value| value.as_ref())
        }) {
            if let Some(f) = e.methods.get(member).or_else(|| e.assoc_fns.get(member)) {
                return Some(f);
            }
        }
    }
    if let Some(inst) = program
        .instantiations
        .get(key)
        .or_else(|| program.imported.instantiations.get(key))
    {
        if let TypedInstantiation::Fn(f) = inst {
            return Some(f);
        }
    }
    if let Some((ikey, member)) = key.rsplit_once('.') {
        if let Some(TypedInstantiation::Struct(s)) = program
            .instantiations
            .get(ikey)
            .or_else(|| program.imported.instantiations.get(ikey))
        {
            if member == "init" {
                return s.init.as_ref();
            }
            return s.methods.get(member).or_else(|| s.assoc_fns.get(member));
        }
    }
    None
}

fn collect_callees_from_fn(program: &TypedProgram, f: &TypedFn, out: &mut BTreeSet<String>) {
    for p in &f.params {
        if let Some(d) = &p.default {
            collect_callees_from_expr(program, d, out);
        }
    }
    collect_callees_from_stmts(program, &f.body, out);
}

fn collect_callees_from_stmts(
    program: &TypedProgram,
    stmts: &[TypedStmt],
    out: &mut BTreeSet<String>,
) {
    for s in stmts {
        collect_callees_from_stmt(program, s, out);
    }
}

fn collect_callees_from_stmt(program: &TypedProgram, stmt: &TypedStmt, out: &mut BTreeSet<String>) {
    match &stmt.kind {
        TypedStmtKind::Let { value, .. } => collect_callees_from_expr(program, value, out),
        TypedStmtKind::Assign { target, value } => {
            collect_callees_from_expr(program, target, out);
            collect_callees_from_expr(program, value, out);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            collect_callees_from_expr(program, cond, out);
            collect_callees_from_stmts(program, then_branch, out);
            for e in elifs {
                collect_callees_from_expr(program, &e.cond, out);
                collect_callees_from_stmts(program, &e.body, out);
            }
            if let Some(b) = else_branch {
                collect_callees_from_stmts(program, b, out);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            collect_callees_from_expr(program, scrutinee, out);
            for arm in arms {
                collect_callees_from_pattern(program, &arm.pattern, out);
                if let Some(g) = &arm.guard {
                    collect_callees_from_expr(program, g, out);
                }
                collect_callees_from_stmts(program, &arm.body, out);
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(a, b, _) => {
                    collect_callees_from_expr(program, a, out);
                    collect_callees_from_expr(program, b, out);
                }
                TypedForIter::Expr(e) => collect_callees_from_expr(program, e, out),
            }
            collect_callees_from_stmts(program, body, out);
        }
        TypedStmtKind::While { cond, body, .. } => {
            collect_callees_from_expr(program, cond, out);
            collect_callees_from_stmts(program, body, out);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(Some(e)) => collect_callees_from_expr(program, e, out),
        TypedStmtKind::Return(None) => {}
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            collect_callees_from_expr(program, cond, out);
            if let Some(m) = message {
                collect_callees_from_expr(program, m, out);
            }
        }
        TypedStmtKind::Defer(TypedDeferBody::Expr(e)) => collect_callees_from_expr(program, e, out),
        TypedStmtKind::Defer(TypedDeferBody::Suite(body)) => {
            collect_callees_from_stmts(program, body, out)
        }
        TypedStmtKind::ExprStmt(e) | TypedStmtKind::BareSend { expr: e, .. } => {
            collect_callees_from_expr(program, e, out)
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                collect_callees_from_expr(program, c, out);
            }
            if let Some(d) = deadline {
                collect_callees_from_expr(program, d, out);
            }
            collect_callees_from_stmts(program, body, out);
        }
    }
}

fn collect_callees_from_pattern(
    program: &TypedProgram,
    pat: &TypedPattern,
    out: &mut BTreeSet<String>,
) {
    match &pat.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {}
        TypedPatternKind::Literal(e) => collect_callees_from_expr(program, e, out),
        TypedPatternKind::Take(inner) => collect_callees_from_pattern(program, inner, out),
        TypedPatternKind::Variant { payload, .. } => {
            for p in payload {
                collect_callees_from_pattern(program, p, out);
            }
        }
        TypedPatternKind::Tuple(ps) | TypedPatternKind::Array(ps) | TypedPatternKind::Or(ps) => {
            for p in ps {
                collect_callees_from_pattern(program, p, out);
            }
        }
    }
}

fn collect_callees_from_expr(program: &TypedProgram, expr: &TypedExpr, out: &mut BTreeSet<String>) {
    match &expr.kind {
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Unit
        | TypedExprKind::Local(_)
        | TypedExprKind::Const(_)
        | TypedExprKind::Static(_)
        | TypedExprKind::PoolName(_) => {}
        TypedExprKind::FnRef(key) | TypedExprKind::GroupChild(key) => {
            out.insert(key.spelling());
        }
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base)
        | TypedExprKind::Panic(base) => collect_callees_from_expr(program, base, out),
        TypedExprKind::Index(a, b)
        | TypedExprKind::Binary(_, a, b)
        | TypedExprKind::And(a, b)
        | TypedExprKind::Or(a, b) => {
            collect_callees_from_expr(program, a, out);
            collect_callees_from_expr(program, b, out);
        }
        TypedExprKind::Is(e, p) => {
            collect_callees_from_expr(program, e, out);
            collect_callees_from_pattern(program, p, out);
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            out.insert(callee.spelling());
            if let Some(r) = receiver {
                collect_callees_from_expr(program, r, out);
            }
            for (i, a) in args.iter().enumerate() {
                match &a.value {
                    Some(e) => collect_callees_from_expr(program, e, out),
                    None => {
                        if let Some(f) = lookup_typed_fn(program, &callee.spelling()) {
                            if let Some(d) = f.params.get(i).and_then(|p| p.default.as_ref()) {
                                collect_callees_from_expr(program, d, out);
                            }
                        }
                    }
                }
            }
        }
        TypedExprKind::CallValue(f, args) => {
            collect_callees_from_expr(program, f, out);
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                collect_callees_from_expr(program, a, out);
            }
        }
        TypedExprKind::Try(inner, conv) => {
            collect_callees_from_expr(program, inner, out);
            if let Some(k) = conv {
                out.insert(k.spelling());
            }
        }
        TypedExprKind::OpCall(key, a, b) => {
            out.insert(key.spelling());
            collect_callees_from_expr(program, a, out);
            collect_callees_from_expr(program, b, out);
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                collect_callees_from_expr(program, a, out);
            }
        }
        TypedExprKind::Tuple(args) | TypedExprKind::List(args) => {
            for a in args {
                collect_callees_from_expr(program, a, out);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            crate::sema::typed::TypedClosureBody::Expr(e) => {
                collect_callees_from_expr(program, e, out)
            }
            crate::sema::typed::TypedClosureBody::Suite(stmts) => {
                collect_callees_from_stmts(program, stmts, out)
            }
        },
        TypedExprKind::StructLiteral { name, fields } => {
            for (_, e) in fields {
                collect_callees_from_expr(program, e, out);
            }
            if let Some(s) = program.structs.get(name).or_else(|| {
                program
                    .imported
                    .structs
                    .get(name)
                    .map(|value| value.as_ref())
            }) {
                let supplied: BTreeSet<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                for (fname, def) in &s.field_defaults {
                    if !supplied.contains(fname.as_str()) {
                        collect_callees_from_expr(program, def, out);
                    }
                }
            }
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                collect_callees_from_expr(program, r, out);
            }
            for (_, e) in args {
                collect_callees_from_expr(program, e, out);
            }
        }
    }
}

fn env_lookup(env: &LEnv, name: &str) -> Option<Temp> {
    for scope in env.iter().rev() {
        if let Some(t) = scope.get(name) {
            return Some(*t);
        }
    }
    None
}

fn env_insert(env: &mut LEnv, name: String, t: Temp) {
    env.last_mut().expect("at least one scope").insert(name, t);
}

struct Lowerer<'p> {
    prog: &'p TypedProgram,
    blk_capacity_sectors: Option<u64>,
    rodata: Vec<Vec<u8>>,
    rodata_index: BTreeMap<Vec<u8>, usize>,
}

struct FnBuilder<'p, 'l> {
    lw: &'l mut Lowerer<'p>,
    temp_types: Vec<Type>,
    body: Vec<Inst>,
    ret: Type,
    owner_struct: Option<String>,
}

impl<'p, 'l> FnBuilder<'p, 'l> {
    fn prog(&self) -> &'p TypedProgram {
        self.lw.prog
    }

    fn blk_capacity_sectors(&self) -> Option<u64> {
        self.lw.blk_capacity_sectors
    }

    fn fresh(&mut self, ty: Type) -> Temp {
        self.temp_types.push(ty);
        Temp(self.temp_types.len() - 1)
    }

    fn emit(&mut self, inst: Inst) -> usize {
        self.body.push(inst);
        self.body.len() - 1
    }

    fn here(&self) -> usize {
        self.body.len()
    }

    fn patch_jump(&mut self, idx: usize, target: usize) {
        match &mut self.body[idx] {
            Inst::Jump { target: t } => *t = target,
            Inst::JumpIfFalse { target: t, .. } => *t = target,
            other => panic!("patch_jump: instruction at {idx} is not a jump: {other:?}"),
        }
    }

    fn intern(&mut self, bytes: Vec<u8>) -> usize {
        if let Some(&i) = self.lw.rodata_index.get(&bytes) {
            return i;
        }
        let i = self.lw.rodata.len();
        self.lw.rodata_index.insert(bytes.clone(), i);
        self.lw.rodata.push(bytes);
        i
    }
}

struct LowerQueueSink<'a, 'p, 'l>(&'a mut FnBuilder<'p, 'l>);

impl QueueSink for LowerQueueSink<'_, '_, '_> {
    fn fresh(&mut self, ty: Type) -> Temp {
        self.0.fresh(ty)
    }
    fn emit(&mut self, inst: Inst) -> usize {
        self.0.emit(inst)
    }
    fn here(&mut self) -> usize {
        self.0.here()
    }
    fn patch(&mut self, idx: usize, target: usize) {
        self.0.patch_jump(idx, target)
    }
}

struct LoopCtx {
    break_fixups: Vec<usize>,
    continue_fixups: Vec<usize>,
    defer_marker: usize,
}

fn mmio_access_names(
    mmio_ty: &Type,
    args: &[(String, TypedExpr)],
) -> Result<(String, String), LowerError> {
    let Type::Named(cap, targs) = &crate::sema::bodies::unwrap_own(mmio_ty.clone()) else {
        return Err(LowerError::internal(
            "an MMIO access whose receiver is not an `Mmio[L]`".to_string(),
        ));
    };
    if cap != "Mmio" {
        return Err(LowerError::internal(
            "an MMIO access whose receiver is not an `Mmio[L]`".to_string(),
        ));
    }
    let Some(crate::sema::types::TypeArg::Type(Type::Named(layout, _))) = targs.first() else {
        return Err(LowerError::internal(
            "an `Mmio[L]` whose layout argument is not a named type".to_string(),
        ));
    };
    let Some((_, reg)) = args.iter().find(|(l, _)| l == "register") else {
        return Err(LowerError::internal(
            "an MMIO access with no register name".to_string(),
        ));
    };
    let TypedExprKind::Str(name) = &reg.kind else {
        return Err(LowerError::internal(
            "an MMIO access whose register name is not a literal".to_string(),
        ));
    };
    Ok((layout.clone(), name.clone()))
}

fn mmio_register_offset(
    layout: &str,
    register: &str,
    prog: &TypedProgram,
) -> Result<u64, LowerError> {
    let Some(l) = prog.layouts.iter().find(|l| l.name == layout) else {
        return Err(LowerError::unimplemented(&format!(
            "an MMIO access through `Mmio[{layout}]`, whose `@layout(mmio)` declaration lives in \
             a different module than the driver that maps it — the exact-bytes table this \
             lowering reads is per-module. Declaring the layout beside its driver works today; \
             a cross-module one"
        )));
    };
    match crate::sema::types::mmio_register(l, register) {
        Some(r) => Ok(r.offset),
        None => Err(LowerError::internal(format!(
            "`{layout}` declares no register `{register}` (the checker already refused this)"
        ))),
    }
}

fn runtime_layout_field_offset(
    layout: &str,
    field: &str,
    prog: &TypedProgram,
) -> Result<u64, LowerError> {
    lower_shared::runtime_layout_field_offset(prog, layout, field).map_err(LowerError::internal)
}

fn placed_array_field_index(
    array_place: &TypedExpr,
    prog: &TypedProgram,
) -> Result<Option<(TypedExpr, u64, u64, usize)>, LowerError> {
    lower_shared::placed_array_field_index(array_place, prog, |ty| {
        eval_array_len_with_prog(ty, prog).map_err(|e| e.message)
    })
    .map_err(LowerError::internal)
}

fn placed_struct_array_scalar_field(
    elem_place: &TypedExpr,
    field_name: &str,
    prog: &TypedProgram,
) -> Result<Option<(TypedExpr, TypedExpr, u64, u64, usize)>, LowerError> {
    lower_shared::placed_struct_array_scalar_field(elem_place, field_name, prog, |ty| {
        eval_array_len_with_prog(ty, prog).map_err(|e| e.message)
    })
    .map_err(LowerError::internal)
}

fn placed_static_addr(prog: &TypedProgram, name: &str) -> Result<u64, LowerError> {
    prog.statics
        .get(name)
        .map(|s| s.addr)
        .ok_or_else(|| LowerError::internal(format!("placed static `{name}` not in TypedProgram")))
}

fn lower_untrusted_checked_le(
    expr: &TypedExpr,
    receiver: &Option<Box<TypedExpr>>,
    type_arg: &Option<Type>,
    args: &[(String, TypedExpr)],
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    let Some(recv) = receiver else {
        return Err(LowerError::internal(
            "`Untrusted.checked_le` with no receiver".to_string(),
        ));
    };
    let Some(payload_ty) = type_arg.clone() else {
        return Err(LowerError::internal(
            "`Untrusted.checked_le` with no payload type".to_string(),
        ));
    };
    let Some((_, bound_expr)) = args.iter().find(|(l, _)| l == "bound") else {
        return Err(LowerError::internal(
            "`Untrusted.checked_le` with no bound argument".to_string(),
        ));
    };
    let payload = lower_expr(recv, b, env)?;
    let bound = lower_expr(bound_expr, b, env)?;
    let le = b.fresh(Type::Bool);
    b.emit(Inst::Compare {
        dst: le,
        op: BinOp::Le,
        ty: payload_ty.clone(),
        lhs: payload,
        rhs: bound,
    });
    let result = b.fresh(expr.ty.clone());
    let else_fixup = b.emit(Inst::JumpIfFalse {
        cond: le,
        target: usize::MAX,
    });
    b.emit(Inst::MakeEnum {
        dst: result,
        tag: value::RESULT_OK,
        payload: vec![payload],
    });
    let end_fixup = b.emit(Inst::Jump { target: usize::MAX });
    let else_pos = b.here();
    b.patch_jump(else_fixup, else_pos);
    let err_unit = b.fresh(Type::Unit);
    b.emit(Inst::ConstUnit { dst: err_unit });
    b.emit(Inst::MakeEnum {
        dst: result,
        tag: value::RESULT_ERR,
        payload: vec![err_unit],
    });
    let end_pos = b.here();
    b.patch_jump(end_fixup, end_pos);
    Ok(result)
}

pub fn lower_program(program: &TypedProgram) -> Result<MwirProgram, LowerError> {
    lower_program_with(program, &LowerOpts::default())
}

pub fn lower_program_with(
    program: &TypedProgram,
    opts: &LowerOpts,
) -> Result<MwirProgram, LowerError> {
    lower_program_with_capacity(program, opts, program.blk_capacity_sectors)
}

pub fn lower_program_with_capacity(
    program: &TypedProgram,
    opts: &LowerOpts,
    blk_capacity_sectors: Option<u64>,
) -> Result<MwirProgram, LowerError> {
    let computed;
    let reachable: &BTreeSet<String> = match &opts.only {
        Some(set) => set,
        None => {
            computed = guest_reachable_keys(program, opts);
            &computed
        }
    };
    let mut lw = Lowerer {
        prog: program,
        blk_capacity_sectors,
        rodata: Vec::new(),
        rodata_index: BTreeMap::new(),
    };
    let mut fns: BTreeMap<String, MwirFn> = BTreeMap::new();

    for (name, f) in &program.fns {
        if program.image_fn.as_deref() == Some(name.as_str()) {
            continue;
        }
        if f.is_async {
            continue;
        }
        if f.is_layout_assert {
            continue;
        }
        if is_host_only_comptime_test(program, name, opts) {
            continue;
        }
        if !reachable.contains(name) {
            continue;
        }
        let mf = lower_fn(f, None, &mut lw)?;
        fns.insert(name.clone(), mf);
    }
    for (sname, s) in &program.structs {
        lower_struct_members(sname, s, &mut lw, &mut fns, &reachable)?;
    }
    for (ename, e) in &program.enums {
        lower_enum_members(ename, e, &mut lw, &mut fns, &reachable)?;
    }
    for (ikey, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => {
                if f.is_async || f.is_layout_assert || !reachable.contains(ikey) {
                    continue;
                }
                let mf = lower_fn(f, None, &mut lw)?;
                fns.insert(ikey.clone(), mf);
            }
            TypedInstantiation::Struct(s) => {
                lower_struct_members(ikey, s, &mut lw, &mut fns, &reachable)?;
            }
            TypedInstantiation::Enum(_) => {}
        }
    }
    for (name, f) in &program.imported.fns {
        if f.is_async
            || f.is_layout_assert
            || is_host_only_comptime_test(program, name, opts)
            || fns.contains_key(name)
            || !reachable.contains(name)
        {
            continue;
        }
        let mf = lower_fn(f, None, &mut lw)?;
        fns.insert(name.clone(), mf);
    }
    for (sname, s) in &program.imported.structs {
        lower_struct_members(sname, s, &mut lw, &mut fns, &reachable)?;
    }
    for (ename, e) in &program.imported.enums {
        lower_enum_members(ename, e, &mut lw, &mut fns, &reachable)?;
    }
    for (ikey, inst) in &program.imported.instantiations {
        if fns.contains_key(ikey) {
            continue;
        }
        match inst {
            TypedInstantiation::Fn(f) => {
                if f.is_async || f.is_layout_assert || !reachable.contains(ikey) {
                    continue;
                }
                let mf = lower_fn(f, None, &mut lw)?;
                fns.insert(ikey.clone(), mf);
            }
            TypedInstantiation::Struct(s) => {
                lower_struct_members(ikey, s, &mut lw, &mut fns, &reachable)?;
            }
            TypedInstantiation::Enum(_) => {}
        }
    }
    install_pixels_packet_intrinsics(program, &mut fns)?;
    install_pixels_census_regions(program, &mut fns)?;
    install_pixels_display_driver_bridge(program, &mut fns)?;
    let direct_fp_fns = fns
        .keys()
        .filter(|key| is_renderer_internal_function(program, key))
        .cloned()
        .collect();
    Ok(MwirProgram {
        fns,
        rodata: lw.rodata,
        direct_fp_fns,
    })
}

/// Enumerate the linked symbols this module view can lower before compiler
/// intrinsic installation. Closure lowering uses this cheap inventory to
/// choose the same last-wins owner that `merge_mwir_programs` historically
/// selected, without lowering identical imported bodies in every caller.
pub fn sync_lowering_candidates(program: &TypedProgram, opts: &LowerOpts) -> BTreeSet<String> {
    fn add_fn(
        out: &mut BTreeSet<String>,
        reachable: &BTreeSet<String>,
        program: &TypedProgram,
        opts: &LowerOpts,
        key: String,
        function: &TypedFn,
        imported: bool,
    ) {
        if !function.is_async
            && !function.is_layout_assert
            && !(imported && is_host_only_comptime_test(program, &key, opts))
            && program.image_fn.as_deref() != Some(key.as_str())
            && reachable.contains(&key)
        {
            out.insert(key);
        }
    }
    fn add_struct(
        out: &mut BTreeSet<String>,
        reachable: &BTreeSet<String>,
        program: &TypedProgram,
        opts: &LowerOpts,
        prefix: &str,
        structure: &TypedStruct,
    ) {
        for (member, function) in &structure.methods {
            add_fn(
                out,
                reachable,
                program,
                opts,
                format!("{prefix}.{member}"),
                function,
                false,
            );
        }
        for (member, function) in &structure.assoc_fns {
            add_fn(
                out,
                reachable,
                program,
                opts,
                format!("{prefix}.{member}"),
                function,
                false,
            );
        }
        if let Some(function) = &structure.init {
            add_fn(
                out,
                reachable,
                program,
                opts,
                format!("{prefix}.init"),
                function,
                false,
            );
        }
    }

    let computed;
    let reachable = match &opts.only {
        Some(keys) => keys,
        None => {
            computed = guest_reachable_keys(program, opts);
            &computed
        }
    };
    let mut out = BTreeSet::new();
    for (name, function) in &program.fns {
        add_fn(
            &mut out,
            reachable,
            program,
            opts,
            name.clone(),
            function,
            false,
        );
    }
    for (name, function) in &program.imported.fns {
        add_fn(
            &mut out,
            reachable,
            program,
            opts,
            name.clone(),
            function,
            true,
        );
    }
    for (name, structure) in &program.structs {
        add_struct(&mut out, reachable, program, opts, name, structure);
    }
    for (name, structure) in &program.imported.structs {
        add_struct(&mut out, reachable, program, opts, name, structure);
    }
    for (name, enumeration) in program.enums.iter().map(|(key, value)| (key, value)).chain(
        program
            .imported
            .enums
            .iter()
            .map(|(key, value)| (key, value.as_ref())),
    ) {
        for (member, function) in &enumeration.methods {
            add_fn(
                &mut out,
                reachable,
                program,
                opts,
                format!("{name}.{member}"),
                function,
                false,
            );
        }
        for (member, function) in &enumeration.assoc_fns {
            add_fn(
                &mut out,
                reachable,
                program,
                opts,
                format!("{name}.{member}"),
                function,
                false,
            );
        }
    }
    for (key, instantiation) in program
        .instantiations
        .iter()
        .chain(&program.imported.instantiations)
    {
        match instantiation {
            TypedInstantiation::Fn(function) => add_fn(
                &mut out,
                reachable,
                program,
                opts,
                key.clone(),
                function,
                false,
            ),
            TypedInstantiation::Struct(structure) => {
                add_struct(&mut out, reachable, program, opts, key, structure)
            }
            TypedInstantiation::Enum(_) => {}
        }
    }
    out
}

fn is_renderer_internal_module(module: &str) -> bool {
    module == "core.__image_pixels"
        || module == "core.field"
        || module == "core.render"
        || module
            .strip_prefix("core.render_")
            .is_some_and(|suffix| !suffix.is_empty())
}

/// Resolve a function's declaration through loader-owned identity. A local
/// declaration's lexical `module` name is intentionally insufficient: a user
/// source can spell `module render_raster`, but it cannot acquire the loader
/// key `core.render_raster`.
fn is_renderer_internal_function(program: &TypedProgram, key: &str) -> bool {
    let Some(declared_module) = program.fn_decl_modules.get(key).map(String::as_str) else {
        return false;
    };
    let Some(declared_name) = program.fn_decl_names.get(key).map(String::as_str) else {
        return false;
    };
    if declared_name.is_empty() {
        return false;
    }
    let module = if declared_module == program.module_path {
        program.module_key.as_str()
    } else {
        declared_module
    };
    is_renderer_internal_module(module)
}

fn install_pixels_display_driver_bridge(
    program: &TypedProgram,
    fns: &mut BTreeMap<String, MwirFn>,
) -> Result<(), LowerError> {
    let Some(submit) = fns.get(PIXELS_DISPLAY_RAW_FN) else {
        return Ok(());
    };
    let canonical_fn = |name: &str, module: &str| {
        program.fn_decl_modules.get(name).map(String::as_str) == Some(module)
            && program.fn_decl_names.get(name).map(String::as_str) == Some(name)
    };
    if !canonical_fn(PIXELS_DISPLAY_RAW_FN, "core.__image_pixels") {
        return Ok(());
    }
    if submit.receiver.is_some()
        || submit.params.len() != 2
        || submit
            .params
            .first()
            .and_then(|(temp, _)| submit.temp_types.get(temp.0))
            != Some(&Type::U64)
        || submit
            .params
            .get(1)
            .and_then(|(temp, _)| submit.temp_types.get(temp.0))
            != Some(&Type::U64)
        || submit.ret != Type::Unit
    {
        return Err(LowerError::internal(
            "Pixels display driver bridge has a noncanonical signature",
        ));
    }
    if let Some(driver) = fns.get(PIXELS_DISPLAY_DRIVER_FN) {
        // `stdlib/drivers/display.wr` declares its lexical module as
        // `display`; this hook runs while that module is lowered, before the
        // linked module key is re-qualified as `drivers.display`.
        if !canonical_fn(PIXELS_DISPLAY_DRIVER_FN, "display")
            || driver.receiver.is_some()
            || driver.params.len() != 2
            || driver
                .params
                .iter()
                .any(|(temp, _)| driver.temp_types.get(temp.0) != Some(&Type::U64))
            || driver.ret != Type::Unit
        {
            return Err(LowerError::internal(format!(
                "Pixels display driver bridge target has a noncanonical identity or signature: module={:?} declaration={:?}",
                program.fn_decl_modules.get(PIXELS_DISPLAY_DRIVER_FN),
                program.fn_decl_names.get(PIXELS_DISPLAY_DRIVER_FN),
            )));
        }
        let driver = fns
            .get_mut(PIXELS_DISPLAY_DRIVER_FN)
            .expect("validated above");
        let doorbell = driver.params[0].0;
        let control = driver.params[1].0;
        let result = Temp(driver.temp_types.len());
        driver.temp_types.push(Type::Unit);
        driver.body = vec![
            Inst::Dmb {
                option: "ishst".to_string(),
            },
            Inst::MmioWrite {
                base: doorbell,
                offset: 0,
                ty: Type::U64,
                value: control,
            },
            Inst::Dmb {
                option: "ishld".to_string(),
            },
            Inst::ConstUnit { dst: result },
            Inst::Return {
                value: Some(result),
            },
        ];
    }
    let submit = fns.get_mut(PIXELS_DISPLAY_RAW_FN).expect("validated above");
    let result = Temp(submit.temp_types.len());
    submit.temp_types.push(Type::Unit);
    submit.body = vec![
        Inst::Call {
            dst: result,
            write_backs: Vec::new(),
            key: PIXELS_DISPLAY_DRIVER_FN.to_string(),
            args: vec![submit.params[0].0, submit.params[1].0],
        },
        Inst::Return {
            value: Some(result),
        },
    ];
    Ok(())
}

pub(crate) fn validate_pixels_display_driver_bridge_target(
    program: &MwirProgram,
) -> Result<(), LowerError> {
    let Some(submit) = program.fns.get(PIXELS_DISPLAY_RAW_FN) else {
        return Ok(());
    };
    let driver = program.fns.get(PIXELS_DISPLAY_DRIVER_FN).ok_or_else(|| {
        LowerError::internal("Pixels display driver bridge target is missing from the closure")
    })?;
    if submit.receiver.is_some()
        || submit.params.len() != 2
        || submit
            .params
            .first()
            .and_then(|(temp, _)| submit.temp_types.get(temp.0))
            != Some(&Type::U64)
        || submit
            .params
            .get(1)
            .and_then(|(temp, _)| submit.temp_types.get(temp.0))
            != Some(&Type::U64)
        || submit.ret != Type::Unit
    {
        return Err(LowerError::internal(
            "Pixels display driver bridge has a noncanonical merged signature",
        ));
    }
    if !matches!(
        submit.body.as_slice(),
        [
            Inst::Call { key, args, write_backs, .. },
            Inst::Return { .. },
        ] if key == PIXELS_DISPLAY_DRIVER_FN
            && args == &[submit.params[0].0, submit.params[1].0]
            && write_backs.is_empty()
    ) || !matches!(
        driver.body.as_slice(),
        [
            Inst::Dmb { option: before },
            Inst::MmioWrite { base, offset: 0, ty: Type::U64, value },
            Inst::Dmb { option: after },
            Inst::ConstUnit { .. },
            Inst::Return { .. },
        ] if before == "ishst"
            && after == "ishld"
            && *base == driver.params[0].0
            && *value == driver.params[1].0
    ) {
        return Err(LowerError::internal(
            "Pixels display driver bridge was not installed canonically",
        ));
    }
    Ok(())
}

/// Rewrite one function's census-region calls into non-emitting markers and
/// delete the temps the call site would otherwise have kept alive.
///
/// Split out so the erasure has a focused test: a marker that leaves its
/// constant argument or its unit result in `temp_types` costs a frame slot
/// and a `movz`/`str` pair, which would break the mechanism's one promise.
fn install_census_regions_in_fn(key: &str, function: &mut MwirFn) -> Result<(), LowerError> {
    // A region id must be a constant in the same function, so resolve
    // constants before rewriting any call.
    let mut constants: BTreeMap<usize, i128> = BTreeMap::new();
    for inst in &function.body {
        if let Inst::ConstInt { dst, value, .. } = inst {
            constants.insert(dst.0, *value);
        }
    }
    // `(instruction index, region, argument temp, result temp)` per call.
    let mut rewrites: Vec<(usize, u32, Temp, Temp)> = Vec::new();
    for (index, inst) in function.body.iter().enumerate() {
        let Inst::Call {
            dst,
            write_backs,
            key: call_key,
            args,
        } = inst
        else {
            continue;
        };
        if call_key != CENSUS_REGION_FN {
            continue;
        }
        if !write_backs.is_empty() || args.len() != 1 {
            return Err(LowerError::internal(
                "Pixels census region annotation has a noncanonical call site",
            ));
        }
        let Some(value) = constants.get(&args[0].0).copied() else {
            return Err(LowerError::internal(
                "Pixels census region id must be a compile-time constant",
            ));
        };
        let region = u32::try_from(value)
            .map_err(|_| LowerError::internal("Pixels census region id is out of range for u32"))?;
        if !crate::pixels::hot_census::is_sealed_region(region) {
            return Err(LowerError::internal(format!(
                "Pixels census region {region} is not in the sealed vocabulary"
            )));
        }
        rewrites.push((index, region, args[0], *dst));
    }
    if rewrites.is_empty() {
        return Ok(());
    }
    // A marker must cost nothing. Its call lowers to a constant argument
    // and a unit result, and both would otherwise keep a frame slot and
    // an emitted `movz`/`str` pair — which is exactly the emitted-byte
    // change the region mechanism promises not to make. So the constant
    // becomes a second marker for the same region (idempotent for the
    // census), and both temps are deleted with the function renumbered.
    let mut erased: BTreeSet<usize> = BTreeSet::new();
    for (index, region, arg, dst) in &rewrites {
        let mut defined_at = None;
        for (at, inst) in function.body.iter().enumerate() {
            if matches!(inst, Inst::ConstInt { dst: d, .. } if d == arg) {
                if defined_at.is_some() {
                    return Err(LowerError::internal(
                        "Pixels census region id has more than one definition",
                    ));
                }
                defined_at = Some(at);
            }
        }
        let Some(defined_at) = defined_at else {
            return Err(LowerError::internal(
                "Pixels census region id has no constant definition",
            ));
        };
        function.body[defined_at] = Inst::RegionMarker { region: *region };
        function.body[*index] = Inst::RegionMarker { region: *region };
        erased.insert(arg.0);
        erased.insert(dst.0);
    }
    let mut live: BTreeSet<usize> = BTreeSet::new();
    for inst in &mut function.body {
        crate::mwir_opt::visit_temps_mut(inst, &mut |temp| {
            live.insert(temp.0);
        });
    }
    for (temp, _) in function.params.iter().chain(function.receiver.iter()) {
        live.insert(temp.0);
    }
    if let Some(shared) = erased.iter().find(|temp| live.contains(temp)) {
        return Err(LowerError::internal(format!(
            "`{key}` shares census-region temp t{shared} with live code; a marker must \
             own its constant"
        )));
    }
    let mut renumber = Vec::with_capacity(function.temp_types.len());
    let mut kept_types = Vec::with_capacity(function.temp_types.len());
    for (old, ty) in function.temp_types.iter().enumerate() {
        if erased.contains(&old) {
            renumber.push(None);
        } else {
            renumber.push(Some(kept_types.len()));
            kept_types.push(ty.clone());
        }
    }
    function.temp_types = kept_types;
    let mut broken = None;
    for inst in &mut function.body {
        crate::mwir_opt::visit_temps_mut(inst, &mut |temp| match renumber[temp.0] {
            Some(new) => temp.0 = new,
            None => broken = Some(temp.0),
        });
    }
    for (temp, _) in function
        .params
        .iter_mut()
        .chain(function.receiver.iter_mut())
    {
        match renumber[temp.0] {
            Some(new) => temp.0 = new,
            None => broken = Some(temp.0),
        }
    }
    if let Some(temp) = broken {
        return Err(LowerError::internal(format!(
            "`{key}` still names erased census temp t{temp} after renumbering"
        )));
    }

    Ok(())
}

/// The sealed source annotation that opens a hot-path census region.
const CENSUS_REGION_FN: &str = "pixels_census_region";

/// Replace every `pixels_census_region(id)` call with a non-emitting
/// [`Inst::RegionMarker`], then delete the annotation function itself.
///
/// Fail-closed properties this installer carries:
///
/// - the annotation's signature is checked against its canonical module and
///   shape, so an unrelated function of the same name cannot be captured;
/// - the region argument must be a compile-time constant naming a sealed
///   region id — a runtime value or an unknown id is a lowering error, not a
///   marker the census would later have to guess at;
/// - the function is removed once every call site is rewritten, so no code is
///   emitted for it and the image is byte-identical to one with no markers.
fn install_pixels_census_regions(
    program: &TypedProgram,
    fns: &mut BTreeMap<String, MwirFn>,
) -> Result<(), LowerError> {
    let Some(function) = fns.get(CENSUS_REGION_FN) else {
        // No renderer closure in this program: nothing to install, and no
        // call site can exist either.
        for function in fns.values() {
            if function
                .body
                .iter()
                .any(|inst| matches!(inst, Inst::Call { key, .. } if key == CENSUS_REGION_FN))
            {
                return Err(LowerError::internal(
                    "Pixels census region annotation is called but not in the closure",
                ));
            }
        }
        return Ok(());
    };
    let canonical_identity = program
        .fn_decl_modules
        .get(CENSUS_REGION_FN)
        .map(String::as_str)
        == Some("core.render_raster")
        && program
            .fn_decl_names
            .get(CENSUS_REGION_FN)
            .map(String::as_str)
            == Some(CENSUS_REGION_FN);
    if !canonical_identity {
        return Ok(());
    }
    if function.receiver.is_some()
        || function.params.len() != 1
        || !function
            .params
            .first()
            .and_then(|(temp, _)| function.temp_types.get(temp.0))
            .is_some_and(|ty| *ty == Type::U32)
        || function.ret != Type::Unit
    {
        return Err(LowerError::internal(
            "Pixels census region annotation has a noncanonical signature",
        ));
    }

    for (key, function) in fns.iter_mut() {
        install_census_regions_in_fn(key, function)?;
    }
    fns.remove(CENSUS_REGION_FN);
    Ok(())
}

fn install_pixels_packet_intrinsics(
    program: &TypedProgram,
    fns: &mut BTreeMap<String, MwirFn>,
) -> Result<(), LowerError> {
    const I32_FROM_LANES: &str = "I32x4.from_lanes";
    const F32_FROM_LANES: &str = "F32x4.from_lanes";
    const I32_SPLAT: &str = "pixels_i32x4_backend_splat";
    const I32_ADD: &str = "pixels_i32x4_backend_add";
    const I32_SUB: &str = "pixels_i32x4_backend_sub";
    const I32_SHR: &str = "pixels_i32x4_backend_shr_arith_imm";
    const I32_AND: &str = "pixels_i32x4_backend_and";
    const I32_OR: &str = "pixels_i32x4_backend_or";
    const I32_SELECT_GT: &str = "pixels_i32x4_backend_select_gt";
    const I32_TO_F32: &str = "pixels_i32x4_backend_to_f32x4";
    const F32_SPLAT: &str = "pixels_f32x4_backend_splat";
    const F32_ADD: &str = "pixels_f32x4_backend_add";
    const F32_SUB: &str = "pixels_f32x4_backend_sub";
    const F32_MUL: &str = "pixels_f32x4_backend_mul";
    const F32_MIN: &str = "pixels_f32x4_backend_min";
    const F32_MAX: &str = "pixels_f32x4_backend_max";
    const F32_SELECT_GE: &str = "pixels_f32x4_backend_select_ge";
    const F32_SELECT_GT: &str = "pixels_f32x4_backend_select_gt";
    const F32_FMA: &str = "pixels_f32x4_backend_fma";
    const F32_TO_I32: &str = "pixels_f32x4_backend_to_i32x4";

    const INTRINSICS: &[&str] = &[
        I32_FROM_LANES,
        F32_FROM_LANES,
        I32_SPLAT,
        I32_ADD,
        I32_SUB,
        I32_SHR,
        I32_AND,
        I32_OR,
        I32_SELECT_GT,
        I32_TO_F32,
        F32_SPLAT,
        F32_ADD,
        F32_SUB,
        F32_MUL,
        F32_MIN,
        F32_MAX,
        F32_SELECT_GE,
        F32_SELECT_GT,
        F32_FMA,
        F32_TO_I32,
    ];

    let canonical_type = |visible: &str, declaration: &str| {
        program.type_decl_modules.get(visible).map(String::as_str) == Some("core.render_raster")
            && program.type_decl_names.get(visible).map(String::as_str) == Some(declaration)
    };
    let canonical_intrinsic = |name: &str| {
        if let Some((visible_type, member)) = name.split_once('.') {
            member == "from_lanes"
                && matches!(visible_type, "I32x4" | "F32x4")
                && canonical_type(visible_type, visible_type)
        } else {
            program.fn_decl_modules.get(name).map(String::as_str) == Some("core.render_raster")
                && program.fn_decl_names.get(name).map(String::as_str) == Some(name)
        }
    };

    let mut referenced = BTreeSet::new();
    for (caller, function) in fns.iter() {
        for intrinsic in function.body.iter().filter_map(|inst| match inst {
            Inst::Call { key, .. }
                if INTRINSICS.contains(&key.as_str()) && canonical_intrinsic(key) =>
            {
                Some(key)
            }
            _ => None,
        }) {
            if !is_renderer_internal_function(program, caller) {
                return Err(LowerError::internal(format!(
                    "Pixels packet intrinsic {intrinsic} is called from non-renderer function {caller}"
                )));
            }
            referenced.insert(intrinsic.clone());
        }
    }
    let declared = INTRINSICS
        .iter()
        .copied()
        .filter(|name| fns.contains_key(*name) && canonical_intrinsic(name))
        .collect::<BTreeSet<_>>();
    if declared.is_empty() && referenced.is_empty() {
        return Ok(());
    }
    for name in &referenced {
        if !fns.contains_key(name) {
            return Err(LowerError::internal(format!(
                "Pixels packet intrinsic {name} is called without its canonical declaration"
            )));
        }
    }

    let signature = |name: &str| -> Option<(Vec<fn(&Type) -> bool>, fn(&Type) -> bool)> {
        fn i32x4(ty: &Type) -> bool {
            matches!(ty, Type::Named(name, args) if name == "I32x4" && args.is_empty())
        }
        fn f32x4(ty: &Type) -> bool {
            matches!(ty, Type::Named(name, args) if name == "F32x4" && args.is_empty())
        }
        fn i32_lanes(ty: &Type) -> bool {
            matches!(ty, Type::Array(elem, len) if **elem == Type::I32 && crate::sema::bodies::literal_array_len(len) == Some(4))
        }
        fn f32_lanes(ty: &Type) -> bool {
            matches!(ty, Type::Array(elem, len) if **elem == Type::F32 && crate::sema::bodies::literal_array_len(len) == Some(4))
        }
        fn i32_scalar(ty: &Type) -> bool {
            *ty == Type::I32
        }
        fn f32_scalar(ty: &Type) -> bool {
            *ty == Type::F32
        }
        fn u8_scalar(ty: &Type) -> bool {
            *ty == Type::U8
        }

        let binary_i32 = || {
            (
                vec![i32x4 as fn(&Type) -> bool; 2],
                i32x4 as fn(&Type) -> bool,
            )
        };
        let binary_f32 = || {
            (
                vec![f32x4 as fn(&Type) -> bool; 2],
                f32x4 as fn(&Type) -> bool,
            )
        };
        Some(match name {
            I32_FROM_LANES => (vec![i32_lanes], i32x4),
            F32_FROM_LANES => (vec![f32_lanes], f32x4),
            I32_SPLAT => (vec![i32_scalar], i32x4),
            I32_ADD | I32_SUB | I32_AND | I32_OR => binary_i32(),
            I32_SHR => (vec![i32x4, u8_scalar], i32x4),
            I32_SELECT_GT => (vec![i32x4; 4], i32x4),
            I32_TO_F32 => (vec![i32x4], f32x4),
            F32_SPLAT => (vec![f32_scalar], f32x4),
            F32_ADD | F32_SUB | F32_MUL | F32_MIN | F32_MAX => binary_f32(),
            F32_SELECT_GE | F32_SELECT_GT => (vec![f32x4; 4], f32x4),
            F32_FMA => (vec![f32x4; 3], f32x4),
            F32_TO_I32 => (vec![f32x4], i32x4),
            _ => return None,
        })
    };

    for name in &declared {
        let Some(function) = fns.get(*name) else {
            continue;
        };
        let canonical_module = program.fn_decl_modules.get(*name).map(String::as_str);
        let (params, ret) = signature(name).expect("intrinsic table and signatures agree");
        let packet_type_is_canonical = |ty: &Type| match ty {
            Type::Named(visible, args)
                if args.is_empty() && matches!(visible.as_str(), "I32x4" | "F32x4") =>
            {
                canonical_type(visible, visible)
            }
            _ => true,
        };
        if function.receiver.is_some()
            || function.params.len() != params.len()
            || function
                .params
                .iter()
                .zip(&params)
                .any(|((temp, _), accepts)| !function.temp_types.get(temp.0).is_some_and(accepts))
            || !ret(&function.ret)
            || function
                .params
                .iter()
                .filter_map(|(temp, _)| function.temp_types.get(temp.0))
                .chain(std::iter::once(&function.ret))
                .any(|ty| !packet_type_is_canonical(ty))
        {
            return Err(LowerError::internal(format!(
                "Pixels packet intrinsic {name} has a noncanonical signature: module={canonical_module:?} receiver={:?} params={:?} ret={:?}",
                function.receiver,
                function
                    .params
                    .iter()
                    .map(|(temp, _)| function.temp_types.get(temp.0))
                    .collect::<Vec<_>>(),
                function.ret,
            )));
        }
    }

    for function in fns.values_mut() {
        let constant_ints = function
            .body
            .iter()
            .filter_map(|inst| match inst {
                Inst::ConstInt { dst, value, .. } => Some((*dst, *value)),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let temp_types = function.temp_types.clone();
        for inst in &mut function.body {
            let Inst::Call {
                dst,
                write_backs,
                key,
                args,
            } = inst
            else {
                continue;
            };
            let Some((params, ret)) = signature(key) else {
                continue;
            };
            if !canonical_intrinsic(key) {
                continue;
            }
            if !write_backs.is_empty()
                || args.len() != params.len()
                || args
                    .iter()
                    .zip(&params)
                    .any(|(arg, accepts)| !temp_types.get(arg.0).is_some_and(accepts))
                || !temp_types.get(dst.0).is_some_and(ret)
            {
                return Err(LowerError::internal(format!(
                    "Pixels packet intrinsic {key} has a noncanonical call site"
                )));
            }
            let binary = |kind, op| Inst::PacketBinary {
                kind,
                op,
                dst: *dst,
                lhs: args[0],
                rhs: args[1],
            };
            *inst = match key.as_str() {
                I32_FROM_LANES => Inst::PacketFromLanes {
                    kind: mwir::PacketKind::I32x4,
                    dst: *dst,
                    lanes: args[0],
                },
                F32_FROM_LANES => Inst::PacketFromLanes {
                    kind: mwir::PacketKind::F32x4,
                    dst: *dst,
                    lanes: args[0],
                },
                I32_SPLAT => Inst::PacketSplat {
                    kind: mwir::PacketKind::I32x4,
                    dst: *dst,
                    scalar: args[0],
                },
                F32_SPLAT => Inst::PacketSplat {
                    kind: mwir::PacketKind::F32x4,
                    dst: *dst,
                    scalar: args[0],
                },
                I32_ADD => binary(mwir::PacketKind::I32x4, mwir::PacketBinaryOp::Add),
                I32_SUB => binary(mwir::PacketKind::I32x4, mwir::PacketBinaryOp::Sub),
                I32_AND => binary(mwir::PacketKind::I32x4, mwir::PacketBinaryOp::And),
                I32_OR => binary(mwir::PacketKind::I32x4, mwir::PacketBinaryOp::Or),
                F32_ADD => binary(mwir::PacketKind::F32x4, mwir::PacketBinaryOp::Add),
                F32_SUB => binary(mwir::PacketKind::F32x4, mwir::PacketBinaryOp::Sub),
                F32_MUL => binary(mwir::PacketKind::F32x4, mwir::PacketBinaryOp::Mul),
                F32_MIN => binary(mwir::PacketKind::F32x4, mwir::PacketBinaryOp::Min),
                F32_MAX => binary(mwir::PacketKind::F32x4, mwir::PacketBinaryOp::Max),
                I32_SHR => {
                    let Some(value) = constant_ints.get(&args[1]) else {
                        return Err(LowerError::internal(
                            "Pixels i32x4 arithmetic shift requires a compile-time immediate",
                        ));
                    };
                    let immediate = u8::try_from(*value)
                        .ok()
                        .filter(|value| (1..=31).contains(value))
                        .ok_or_else(|| {
                            LowerError::internal(
                                "Pixels i32x4 arithmetic shift immediate must be in 1..=31",
                            )
                        })?;
                    Inst::PacketShiftRightArithmetic {
                        dst: *dst,
                        src: args[0],
                        immediate,
                    }
                }
                I32_SELECT_GT => Inst::PacketSelect {
                    kind: mwir::PacketKind::I32x4,
                    op: mwir::PacketSelectOp::Gt,
                    dst: *dst,
                    lhs: args[0],
                    rhs: args[1],
                    if_true: args[2],
                    if_false: args[3],
                },
                F32_SELECT_GE | F32_SELECT_GT => Inst::PacketSelect {
                    kind: mwir::PacketKind::F32x4,
                    op: if key == F32_SELECT_GE {
                        mwir::PacketSelectOp::Ge
                    } else {
                        mwir::PacketSelectOp::Gt
                    },
                    dst: *dst,
                    lhs: args[0],
                    rhs: args[1],
                    if_true: args[2],
                    if_false: args[3],
                },
                F32_FMA => Inst::PacketFma {
                    dst: *dst,
                    lhs: args[0],
                    rhs: args[1],
                    addend: args[2],
                },
                I32_TO_F32 => Inst::PacketConvert {
                    from: mwir::PacketKind::I32x4,
                    to: mwir::PacketKind::F32x4,
                    dst: *dst,
                    src: args[0],
                },
                F32_TO_I32 => Inst::PacketConvert {
                    from: mwir::PacketKind::F32x4,
                    to: mwir::PacketKind::I32x4,
                    dst: *dst,
                    src: args[0],
                },
                _ => unreachable!("signature matched an intrinsic"),
            };
        }
    }

    if fns.contains_key("__wrela_pixels_p8_raster_regular") {
        validate_pixels_i32x4_raster_mwir(fns)?;
    }
    for name in declared {
        fns.remove(name);
    }
    Ok(())
}

fn validate_pixels_i32x4_raster_mwir(fns: &BTreeMap<String, MwirFn>) -> Result<(), LowerError> {
    const RASTER: &str = "__wrela_pixels_p8_raster_regular";
    let Some(raster) = fns.get(RASTER) else {
        return Err(LowerError::internal(
            "Pixels i32x4 intrinsic is reachable without the production raster loop",
        ));
    };
    let additions = raster
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, inst)| {
            matches!(
                inst,
                Inst::I32x4Add { .. }
                    | Inst::PacketBinary {
                        kind: mwir::PacketKind::I32x4,
                        op: mwir::PacketBinaryOp::Add,
                        ..
                    }
            )
            .then_some(index)
        })
        .collect::<Vec<_>>();
    if additions.len() != 2 {
        return Err(LowerError::internal(format!(
            "Pixels production raster has {} i32x4 additions, expected 2",
            additions.len()
        )));
    }
    let backward_loops = raster
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, inst)| match inst {
            Inst::Jump { target } | Inst::JumpIfFalse { target, .. } if *target < index => {
                Some((*target, index))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let vector_loops = backward_loops
        .iter()
        .filter(|(start, end)| additions.iter().all(|index| start <= index && index < end))
        .count();
    if backward_loops.len() != 3 || vector_loops != 1 {
        return Err(LowerError::internal(format!(
            "Pixels production raster loop shape drifted: backward_loops={} vector_loops={vector_loops}",
            backward_loops.len()
        )));
    }
    Ok(())
}

fn lower_struct_members(
    key_prefix: &str,
    s: &TypedStruct,
    lw: &mut Lowerer,
    fns: &mut BTreeMap<String, MwirFn>,
    reachable: &BTreeSet<String>,
) -> Result<(), LowerError> {
    let owner = Some(key_prefix.to_string());
    for (member, f) in &s.methods {
        let key = format!("{key_prefix}.{member}");
        if f.is_async || !reachable.contains(&key) {
            continue;
        }
        fns.insert(key, lower_fn(f, owner.clone(), lw)?);
    }
    for (member, f) in &s.assoc_fns {
        let key = format!("{key_prefix}.{member}");
        if f.is_async || !reachable.contains(&key) {
            continue;
        }
        fns.insert(key, lower_fn(f, owner.clone(), lw)?);
    }
    if let Some(f) = &s.init {
        let key = format!("{key_prefix}.init");
        if !f.is_async && reachable.contains(&key) {
            fns.insert(key, lower_fn(f, owner, lw)?);
        }
    }
    Ok(())
}

fn lower_enum_members(
    key_prefix: &str,
    e: &crate::sema::typed::TypedEnum,
    lw: &mut Lowerer,
    fns: &mut BTreeMap<String, MwirFn>,
    reachable: &BTreeSet<String>,
) -> Result<(), LowerError> {
    let owner = Some(key_prefix.to_string());
    for (member, f) in &e.methods {
        let key = format!("{key_prefix}.{member}");
        if f.is_async || !reachable.contains(&key) {
            continue;
        }
        fns.insert(key, lower_fn(f, owner.clone(), lw)?);
    }
    for (member, f) in &e.assoc_fns {
        let key = format!("{key_prefix}.{member}");
        if f.is_async || !reachable.contains(&key) {
            continue;
        }
        fns.insert(key, lower_fn(f, owner.clone(), lw)?);
    }
    Ok(())
}

fn lower_fn(
    f: &TypedFn,
    owner_struct: Option<String>,
    lw: &mut Lowerer,
) -> Result<MwirFn, LowerError> {
    let mut b = FnBuilder {
        lw,
        temp_types: Vec::new(),
        body: Vec::new(),
        ret: f.ret.clone(),
        owner_struct,
    };
    let mut env: LEnv = vec![BTreeMap::new()];
    let receiver = match &f.receiver {
        Some((mode, ty)) => {
            let t = b.fresh(ty.clone());
            env_insert(&mut env, "self".to_string(), t);
            Some((t, *mode))
        }
        None => None,
    };
    let mut params = Vec::with_capacity(f.params.len());
    for p in &f.params {
        let t = b.fresh(p.ty.clone());
        env_insert(&mut env, p.name.clone(), t);
        params.push((t, p.mode));
    }
    let mut defers: Vec<&TypedDeferBody> = Vec::new();
    let mut loops: Vec<LoopCtx> = Vec::new();
    lower_block(&f.body, &mut b, &mut env, &mut defers, &mut loops)?;
    b.emit(Inst::Return { value: None });
    Ok(MwirFn {
        receiver,
        params,
        ret: f.ret.clone(),
        temp_types: b.temp_types,
        body: b.body,
    })
}

fn lower_block<'a>(
    stmts: &'a [TypedStmt],
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, LowerError> {
    let start = defers.len();
    let mut diverged = false;
    for s in stmts {
        if lower_stmt(s, b, env, defers, loops)? {
            diverged = true;
            break;
        }
    }
    if !diverged {
        let active: Vec<&TypedDeferBody> = defers[start..].to_vec();
        run_defers(&active, b, env)?;
    }
    defers.truncate(start);
    Ok(diverged)
}

fn run_defers(
    active: &[&TypedDeferBody],
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<(), LowerError> {
    for d in active.iter().rev() {
        let mut inner_defers: Vec<&TypedDeferBody> = Vec::new();
        let mut inner_loops: Vec<LoopCtx> = Vec::new();
        match d {
            TypedDeferBody::Expr(e) => {
                lower_expr(e, b, env)?;
            }
            TypedDeferBody::Suite(stmts) => {
                lower_block(stmts, b, env, &mut inner_defers, &mut inner_loops)?;
            }
        }
    }
    Ok(())
}

fn lower_stmt<'a>(
    stmt: &'a TypedStmt,
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, LowerError> {
    match &stmt.kind {
        TypedStmtKind::Let { name, ty, value } => {
            let v = lower_expr(value, b, env)?;
            let t = b.fresh(ty.clone());
            b.emit(Inst::Copy { dst: t, src: v });
            env_insert(env, name.clone(), t);
            Ok(false)
        }
        TypedStmtKind::Assign { target, value } => {
            let v = lower_expr(value, b, env)?;
            lower_place_write(target, v, b, env)?;
            Ok(false)
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => lower_if(cond, then_branch, elifs, else_branch, b, env, defers, loops),
        TypedStmtKind::Match { scrutinee, arms } => {
            lower_match(scrutinee, arms, b, env, defers, loops)
        }
        TypedStmtKind::While { cond, body, budget } => {
            lower_while(cond, body, *budget, b, env, defers, loops)?;
            Ok(false)
        }
        TypedStmtKind::For {
            name,
            elem_ty,
            iter,
            body,
            budget,
            ..
        } => {
            lower_for(name, elem_ty, iter, body, *budget, b, env, defers, loops)?;
            Ok(false)
        }
        TypedStmtKind::Break => {
            let marker = loops
                .last()
                .ok_or_else(|| LowerError::internal("`break` outside a loop"))?
                .defer_marker;
            let active: Vec<&TypedDeferBody> = defers[marker..].to_vec();
            run_defers(&active, b, env)?;
            let idx = b.emit(Inst::Jump { target: usize::MAX });
            loops
                .last_mut()
                .expect("checked above")
                .break_fixups
                .push(idx);
            Ok(true)
        }
        TypedStmtKind::Continue => {
            let marker = loops
                .last()
                .ok_or_else(|| LowerError::internal("`continue` outside a loop"))?
                .defer_marker;
            let active: Vec<&TypedDeferBody> = defers[marker..].to_vec();
            run_defers(&active, b, env)?;
            let idx = b.emit(Inst::Jump { target: usize::MAX });
            loops
                .last_mut()
                .expect("checked above")
                .continue_fixups
                .push(idx);
            Ok(true)
        }
        TypedStmtKind::Pass => Ok(false),
        TypedStmtKind::Return(value) => {
            let v = match value {
                Some(e) => Some(lower_expr(e, b, env)?),
                None => None,
            };
            let active: Vec<&TypedDeferBody> = defers[..].to_vec();
            run_defers(&active, b, env)?;
            b.emit(Inst::Return { value: v });
            Ok(true)
        }
        TypedStmtKind::Assert { cond, message } => {
            let c = lower_expr(cond, b, env)?;
            let fail_fixup = b.emit(Inst::JumpIfFalse {
                cond: c,
                target: usize::MAX,
            });
            let after_fixup = b.emit(Inst::Jump { target: usize::MAX });
            let fail_pos = b.here();
            b.patch_jump(fail_fixup, fail_pos);
            let msg = match message {
                Some(m) => Some(assert_message_text(m)?),
                None => None,
            };
            b.emit(Inst::AssertFail { message: msg });
            let after_pos = b.here();
            b.patch_jump(after_fixup, after_pos);
            Ok(false)
        }
        TypedStmtKind::ComptimeAssert { .. } => Ok(false),
        TypedStmtKind::Defer(body) => {
            defers.push(body);
            Ok(false)
        }
        TypedStmtKind::ExprStmt(e) => {
            lower_expr(e, b, env)?;
            Ok(false)
        }
        TypedStmtKind::WithGroup { .. } => Err(LowerError::unimplemented(
            "`with group` (FlowWir state machines, plans/M6.md item B) is",
        )),
        TypedStmtKind::BareSend { .. } => Err(LowerError::unimplemented(
            "a bare `send` statement (FlowWir state machines, plans/M6.md item B) is",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_if<'a>(
    cond: &'a TypedExpr,
    then_branch: &'a [TypedStmt],
    elifs: &'a [crate::sema::typed::TypedElif],
    else_branch: &'a Option<Vec<TypedStmt>>,
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, LowerError> {
    let mut end_fixups = Vec::new();
    let mut all_diverge = true;

    let c = lower_expr(cond, b, env)?;
    let mut next_fixup = b.emit(Inst::JumpIfFalse {
        cond: c,
        target: usize::MAX,
    });
    env.push(BTreeMap::new());
    let d = lower_block(then_branch, b, env, defers, loops)?;
    env.pop();
    if !d {
        all_diverge = false;
    }
    end_fixups.push(b.emit(Inst::Jump { target: usize::MAX }));
    let mut pos = b.here();
    b.patch_jump(next_fixup, pos);

    for elif in elifs {
        let c2 = lower_expr(&elif.cond, b, env)?;
        next_fixup = b.emit(Inst::JumpIfFalse {
            cond: c2,
            target: usize::MAX,
        });
        env.push(BTreeMap::new());
        let d2 = lower_block(&elif.body, b, env, defers, loops)?;
        env.pop();
        if !d2 {
            all_diverge = false;
        }
        end_fixups.push(b.emit(Inst::Jump { target: usize::MAX }));
        pos = b.here();
        b.patch_jump(next_fixup, pos);
    }

    match else_branch {
        Some(eb) => {
            env.push(BTreeMap::new());
            let de = lower_block(eb, b, env, defers, loops)?;
            env.pop();
            if !de {
                all_diverge = false;
            }
        }
        None => all_diverge = false,
    }

    let end_pos = b.here();
    for idx in end_fixups {
        b.patch_jump(idx, end_pos);
    }
    Ok(all_diverge)
}

fn lower_while<'a>(
    cond: &'a TypedExpr,
    body: &'a [TypedStmt],
    budget: Option<u64>,
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<(), LowerError> {
    loops.push(LoopCtx {
        break_fixups: Vec::new(),
        continue_fixups: Vec::new(),
        defer_marker: defers.len(),
    });
    let trips = budget.map(|n| {
        let t = b.fresh(Type::U64);
        b.emit(Inst::ConstInt {
            dst: t,
            ty: Type::U64,
            value: 0,
        });
        (t, n)
    });
    let cond_pos = b.here();
    let c = lower_expr(cond, b, env)?;
    let end_fixup = b.emit(Inst::JumpIfFalse {
        cond: c,
        target: usize::MAX,
    });
    if let Some((trips_t, n)) = trips {
        emit_trip_check(b, trips_t, n)?;
    }
    env.push(BTreeMap::new());
    lower_block(body, b, env, defers, loops)?;
    env.pop();
    b.emit(Inst::Jump { target: cond_pos });
    let end_pos = b.here();
    b.patch_jump(end_fixup, end_pos);
    let ctx = loops.pop().expect("pushed above");
    for idx in ctx.break_fixups {
        b.patch_jump(idx, end_pos);
    }
    for idx in ctx.continue_fixups {
        b.patch_jump(idx, cond_pos);
    }
    Ok(())
}

fn emit_trip_check(b: &mut FnBuilder, trips_t: Temp, bound: u64) -> Result<(), LowerError> {
    let one = b.fresh(Type::U64);
    b.emit(Inst::ConstInt {
        dst: one,
        ty: Type::U64,
        value: 1,
    });
    let next = b.fresh(Type::U64);
    b.emit(Inst::ArithWrapping {
        dst: next,
        op: BinOp::AddW,
        ty: Type::U64,
        lhs: trips_t,
        rhs: one,
    });
    b.emit(Inst::Copy {
        dst: trips_t,
        src: next,
    });
    let lim = b.fresh(Type::U64);
    b.emit(Inst::ConstInt {
        dst: lim,
        ty: Type::U64,
        value: i128::from(bound),
    });
    let ok = b.fresh(Type::Bool);
    b.emit(Inst::Compare {
        dst: ok,
        op: BinOp::Le,
        ty: Type::U64,
        lhs: trips_t,
        rhs: lim,
    });
    let fail_fixup = b.emit(Inst::JumpIfFalse {
        cond: ok,
        target: usize::MAX,
    });
    let after = b.emit(Inst::Jump { target: usize::MAX });
    let fail_pos = b.here();
    b.patch_jump(fail_fixup, fail_pos);
    b.emit(Inst::AssertFail {
        message: Some("loop budget exceeded".to_string()),
    });
    let after_pos = b.here();
    b.patch_jump(after, after_pos);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_for<'a>(
    name: &str,
    elem_ty: &Type,
    iter: &'a TypedForIter,
    body: &'a [TypedStmt],
    budget: Option<u64>,
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<(), LowerError> {
    loops.push(LoopCtx {
        break_fixups: Vec::new(),
        continue_fixups: Vec::new(),
        defer_marker: defers.len(),
    });
    let trips = budget.map(|n| {
        let t = b.fresh(Type::U64);
        b.emit(Inst::ConstInt {
            dst: t,
            ty: Type::U64,
            value: 0,
        });
        (t, n)
    });
    match iter {
        TypedForIter::Range(from, to, inclusive) => {
            let from_t = lower_expr(from, b, env)?;
            let to_t = lower_expr(to, b, env)?;
            let i_temp = b.fresh(elem_ty.clone());
            b.emit(Inst::Copy {
                dst: i_temp,
                src: from_t,
            });
            let cond_pos = b.here();
            let cmp_op = if *inclusive { BinOp::Le } else { BinOp::Lt };
            let cond_t = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: cond_t,
                op: cmp_op,
                ty: elem_ty.clone(),
                lhs: i_temp,
                rhs: to_t,
            });
            let end_fixup = b.emit(Inst::JumpIfFalse {
                cond: cond_t,
                target: usize::MAX,
            });
            if let Some((trips_t, n)) = trips {
                emit_trip_check(b, trips_t, n)?;
            }
            env.push(BTreeMap::new());
            env_insert(env, name.to_string(), i_temp);
            lower_block(body, b, env, defers, loops)?;
            env.pop();
            let incr_pos = b.here();
            let one_t = b.fresh(elem_ty.clone());
            b.emit(Inst::ConstInt {
                dst: one_t,
                ty: elem_ty.clone(),
                value: 1,
            });
            let next_t = b.fresh(elem_ty.clone());
            b.emit(Inst::ArithWrapping {
                dst: next_t,
                op: BinOp::AddW,
                ty: elem_ty.clone(),
                lhs: i_temp,
                rhs: one_t,
            });
            b.emit(Inst::Copy {
                dst: i_temp,
                src: next_t,
            });
            b.emit(Inst::Jump { target: cond_pos });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            let ctx = loops.pop().expect("pushed above");
            for idx in ctx.break_fixups {
                b.patch_jump(idx, end_pos);
            }
            for idx in ctx.continue_fixups {
                b.patch_jump(idx, incr_pos);
            }
        }
        TypedForIter::Expr(arr) => {
            let arr_t = lower_expr(arr, b, env)?;
            let len = eval_array_len(&arr.ty)?;
            let idx_t = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: idx_t,
                ty: Type::Usize,
                value: 0,
            });
            let len_t = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: len_t,
                ty: Type::Usize,
                value: len as i128,
            });
            let cond_pos = b.here();
            let cond_t = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: cond_t,
                op: BinOp::Lt,
                ty: Type::Usize,
                lhs: idx_t,
                rhs: len_t,
            });
            let end_fixup = b.emit(Inst::JumpIfFalse {
                cond: cond_t,
                target: usize::MAX,
            });
            if let Some((trips_t, n)) = trips {
                emit_trip_check(b, trips_t, n)?;
            }
            let elem_t = b.fresh(elem_ty.clone());
            b.emit(Inst::IndexGet {
                dst: elem_t,
                base: arr_t,
                index: idx_t,
                len,
            });
            env.push(BTreeMap::new());
            env_insert(env, name.to_string(), elem_t);
            lower_block(body, b, env, defers, loops)?;
            env.pop();
            let incr_pos = b.here();
            let one_t = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: one_t,
                ty: Type::Usize,
                value: 1,
            });
            let next_t = b.fresh(Type::Usize);
            b.emit(Inst::ArithWrapping {
                dst: next_t,
                op: BinOp::AddW,
                ty: Type::Usize,
                lhs: idx_t,
                rhs: one_t,
            });
            b.emit(Inst::Copy {
                dst: idx_t,
                src: next_t,
            });
            b.emit(Inst::Jump { target: cond_pos });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            let ctx = loops.pop().expect("pushed above");
            for idx in ctx.break_fixups {
                b.patch_jump(idx, end_pos);
            }
            for idx in ctx.continue_fixups {
                b.patch_jump(idx, incr_pos);
            }
        }
    }
    Ok(())
}

fn assert_message_text(e: &TypedExpr) -> Result<String, LowerError> {
    if let TypedExprKind::Str(text) = &e.kind {
        Ok(String::from_utf8_lossy(&value::decode_str(text)).into_owned())
    } else {
        Err(LowerError::unimplemented(
            "a non-literal `assert`/`panic` message is",
        ))
    }
}

fn lower_match<'a>(
    scrutinee: &'a TypedExpr,
    arms: &'a [crate::sema::typed::TypedMatchArm],
    b: &mut FnBuilder,
    env: &mut LEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, LowerError> {
    let sv = lower_expr(scrutinee, b, env)?;
    let mut end_fixups = Vec::new();
    let mut all_diverge = true;

    for arm in arms {
        let mut fail_fixups: Vec<usize> = Vec::new();
        let mut bindings = BTreeMap::new();
        collect_pattern_bindings(&arm.pattern, &mut bindings, b);
        let test = lower_pattern_test(&arm.pattern, sv, &bindings, b)?;
        fail_fixups.push(b.emit(Inst::JumpIfFalse {
            cond: test,
            target: usize::MAX,
        }));
        env.push(bindings);
        if let Some(guard) = &arm.guard {
            let g = lower_expr(guard, b, env)?;
            fail_fixups.push(b.emit(Inst::JumpIfFalse {
                cond: g,
                target: usize::MAX,
            }));
        }
        let d = lower_block(&arm.body, b, env, defers, loops)?;
        env.pop();
        if !d {
            all_diverge = false;
        }
        end_fixups.push(b.emit(Inst::Jump { target: usize::MAX }));
        let next_arm_pos = b.here();
        for idx in fail_fixups {
            b.patch_jump(idx, next_arm_pos);
        }
    }
    b.emit(Inst::AssertFail {
        message: Some(
            "match: no arm matched (exhaustiveness already proved this cannot happen)".to_string(),
        ),
    });
    let match_end = b.here();
    for idx in end_fixups {
        b.patch_jump(idx, match_end);
    }
    Ok(all_diverge)
}

fn collect_pattern_bindings(
    pat: &TypedPattern,
    out: &mut BTreeMap<String, Temp>,
    b: &mut FnBuilder,
) {
    match &pat.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        TypedPatternKind::Binding(name) => {
            let t = b.fresh(pat.ty.clone());
            out.insert(name.clone(), t);
        }
        TypedPatternKind::Take(inner) => collect_pattern_bindings(inner, out, b),
        TypedPatternKind::Variant { payload, .. } => {
            for p in payload {
                collect_pattern_bindings(p, out, b);
            }
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            for p in items {
                collect_pattern_bindings(p, out, b);
            }
        }
        TypedPatternKind::Or(_) => {}
    }
}

fn lower_pattern_test(
    pattern: &TypedPattern,
    value: Temp,
    bindings: &BTreeMap<String, Temp>,
    b: &mut FnBuilder,
) -> Result<Temp, LowerError> {
    match &pattern.kind {
        TypedPatternKind::Wildcard => {
            let t = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool {
                dst: t,
                value: true,
            });
            Ok(t)
        }
        TypedPatternKind::Binding(name) => {
            let dst = *bindings
                .get(name)
                .expect("collect_pattern_bindings pre-allocated every binding name");
            b.emit(Inst::Copy { dst, src: value });
            let t = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool {
                dst: t,
                value: true,
            });
            Ok(t)
        }
        TypedPatternKind::Take(inner) => lower_pattern_test(inner, value, bindings, b),
        TypedPatternKind::Literal(lit) => {
            let mut scratch: LEnv = vec![BTreeMap::new()];
            let lit_temp = lower_expr(lit, b, &mut scratch)?;
            let t = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: t,
                op: BinOp::Eq,
                ty: pattern.ty.clone(),
                lhs: value,
                rhs: lit_temp,
            });
            Ok(t)
        }
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } => {
            let want = variant_index(b.prog(), enum_name, variant)?;
            let tag_t = b.fresh(Type::U64);
            b.emit(Inst::EnumTag {
                dst: tag_t,
                src: value,
            });
            let want_t = b.fresh(Type::U64);
            b.emit(Inst::ConstInt {
                dst: want_t,
                ty: Type::U64,
                value: want as i128,
            });
            let mut result = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: result,
                op: BinOp::Eq,
                ty: Type::U64,
                lhs: tag_t,
                rhs: want_t,
            });
            for (i, subpat) in payload.iter().enumerate() {
                let payload_t = b.fresh(subpat.ty.clone());
                b.emit(Inst::EnumPayload {
                    dst: payload_t,
                    src: value,
                    index: i,
                });
                let sub_ok = lower_pattern_test(subpat, payload_t, bindings, b)?;
                let merged = b.fresh(Type::Bool);
                b.emit(Inst::BoolAnd {
                    dst: merged,
                    lhs: result,
                    rhs: sub_ok,
                });
                result = merged;
            }
            Ok(result)
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            let result = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool {
                dst: result,
                value: true,
            });
            let mut result = result;
            for (i, subpat) in items.iter().enumerate() {
                let elem_t = b.fresh(subpat.ty.clone());
                b.emit(Inst::Project {
                    dst: elem_t,
                    base: value,
                    index: i,
                });
                let sub_ok = lower_pattern_test(subpat, elem_t, bindings, b)?;
                let merged = b.fresh(Type::Bool);
                b.emit(Inst::BoolAnd {
                    dst: merged,
                    lhs: result,
                    rhs: sub_ok,
                });
                result = merged;
            }
            Ok(result)
        }
        TypedPatternKind::Or(_) => Err(LowerError::unimplemented("an `|` (or) pattern is")),
    }
}

fn materialize_place_mut(
    place: &TypedExpr,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<(Temp, bool), LowerError> {
    match &place.kind {
        TypedExprKind::Local(name) => {
            let t = env_lookup(env, name).ok_or_else(|| {
                LowerError::internal(format!("unbound local `{name}` in place position"))
            })?;
            Ok((t, false))
        }
        TypedExprKind::Field(..) | TypedExprKind::Index(..) => {
            let t = lower_expr(place, b, env)?;
            Ok((t, true))
        }
        _ => Err(LowerError::internal(
            "expression is not an assignable place",
        )),
    }
}

fn lower_place_write(
    target: &TypedExpr,
    value: Temp,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<(), LowerError> {
    match &target.kind {
        TypedExprKind::Local(name) => {
            let t = env_lookup(env, name).ok_or_else(|| {
                LowerError::internal(format!("unbound local `{name}` in place position"))
            })?;
            b.emit(Inst::Copy { dst: t, src: value });
            Ok(())
        }
        TypedExprKind::Field(base, fname) => {
            if let TypedExprKind::Static(sname) = &base.kind {
                let layout_name = match bodies::unwrap_own(base.ty.clone()) {
                    Type::Named(n, _) => n,
                    other => {
                        return Err(LowerError::internal(format!(
                            "placed static `{sname}` has non-named type {other:?}"
                        )));
                    }
                };
                let offset = runtime_layout_field_offset(&layout_name, fname, b.prog())?;
                let base_temp = lower_expr(base, b, env)?;
                b.emit(Inst::MmioWrite {
                    base: base_temp,
                    offset,
                    ty: target.ty.clone(),
                    value,
                });
                return Ok(());
            }
            if let Some((static_expr, idx_expr, field_offset, elem_stride, len)) =
                placed_struct_array_scalar_field(base, fname, b.prog())?
            {
                let base_temp = lower_expr(&static_expr, b, env)?;
                let idx_temp = lower_expr(&idx_expr, b, env)?;
                b.emit(Inst::PlacedIndexSet {
                    base: base_temp,
                    field_offset,
                    index: idx_temp,
                    value,
                    len,
                    elem_stride,
                    ty: target.ty.clone(),
                });
                return Ok(());
            }
            if bodies::is_interrupt_cell_type(&target.ty) {
                let TypedExprKind::Local(base_name) = &base.kind else {
                    return Err(LowerError::unimplemented(
                        "assigning an `InterruptCell` through a nested field/index chain is",
                    ));
                };
                if base_name != "self" {
                    return Err(LowerError::unimplemented(
                        "assigning an `InterruptCell` on a non-`self` place is",
                    ));
                }
                let base_temp = env_lookup(env, base_name)
                    .ok_or_else(|| LowerError::internal(format!("unbound local `{base_name}`")))?;
                let base_ty = bodies::unwrap_own(base.ty.clone());
                let idx = field_index(b.prog(), &base_ty, fname)?;
                let field_off = interrupt_cell_field_off(b, &base_ty, idx)?;
                b.emit(Inst::InterruptCellStoreRelease {
                    field_off,
                    width: 4,
                    value,
                });
                b.emit(Inst::SetField {
                    base: base_temp,
                    index: idx,
                    value,
                });
                return Ok(());
            }
            let (base_temp, needs_writeback) = materialize_place_mut(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            let idx = field_index(b.prog(), &base_ty, fname)?;
            b.emit(Inst::SetField {
                base: base_temp,
                index: idx,
                value,
            });
            if needs_writeback {
                lower_place_write(base, base_temp, b, env)?;
            }
            Ok(())
        }
        TypedExprKind::Index(base, idx_expr) => {
            if let Some((static_expr, field_offset, elem_stride, len)) =
                placed_array_field_index(base, b.prog())?
            {
                let base_temp = lower_expr(&static_expr, b, env)?;
                let idx_temp = lower_expr(idx_expr, b, env)?;
                b.emit(Inst::PlacedIndexSet {
                    base: base_temp,
                    field_offset,
                    index: idx_temp,
                    value,
                    len,
                    elem_stride,
                    ty: target.ty.clone(),
                });
                return Ok(());
            }
            let (base_temp, needs_writeback) = materialize_place_mut(base, b, env)?;
            let len = eval_array_len(&base.ty)?;
            if let Some(i) = literal_array_index_elide(idx_expr, len)? {
                b.emit(Inst::SetField {
                    base: base_temp,
                    index: i,
                    value,
                });
            } else {
                let idx_temp = lower_expr(idx_expr, b, env)?;
                b.emit(Inst::IndexSet {
                    base: base_temp,
                    index: idx_temp,
                    value,
                    len,
                });
            }
            if needs_writeback {
                lower_place_write(base, base_temp, b, env)?;
            }
            Ok(())
        }
        _ => Err(LowerError::internal(
            "expression is not an assignable place",
        )),
    }
}

fn lower_mut_arg_place<'a>(
    expr: &'a TypedExpr,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<(Temp, Option<&'a TypedExpr>), LowerError> {
    match &expr.kind {
        TypedExprKind::Local(name) => {
            let t = env_lookup(env, name).ok_or_else(|| {
                LowerError::internal(format!("unbound local `{name}` as `mut` argument"))
            })?;
            Ok((t, None))
        }
        TypedExprKind::Field(..) | TypedExprKind::Index(..) => {
            let t = lower_expr(expr, b, env)?;
            Ok((t, Some(expr)))
        }
        _ => Err(LowerError::internal(
            "expression is not an assignable `mut` place",
        )),
    }
}

fn bind_args<'a>(
    f: &TypedFn,
    args: &'a [TypedCallArg],
    self_temp: Option<Temp>,
    b: &mut FnBuilder,
    caller_env: &mut LEnv,
    nested_mut_writebacks: &mut Vec<(&'a TypedExpr, Temp)>,
) -> Result<Vec<Temp>, LowerError> {
    let mut callee_env: LEnv = vec![BTreeMap::new()];
    if let Some(st) = self_temp {
        env_insert(&mut callee_env, "self".to_string(), st);
    }
    let mut out = Vec::with_capacity(args.len());
    for (param, slot) in f.params.iter().zip(args.iter()) {
        let t = match &slot.value {
            Some(e) if param.mode == AccessMode::Mut => {
                let (t, wb) = lower_mut_arg_place(e, b, caller_env)?;
                if let Some(place) = wb {
                    nested_mut_writebacks.push((place, t));
                }
                t
            }
            Some(e) => lower_expr(e, b, caller_env)?,
            None if param.mode == AccessMode::Mut => {
                return Err(LowerError::unimplemented(
                    "writing back a `mut` parameter through a defaulted argument is",
                ));
            }
            None => {
                let default = param
                    .default
                    .as_ref()
                    .expect("producer guarantees a default when a call slot is None");
                lower_expr(default, b, &mut callee_env)?
            }
        };
        env_insert(&mut callee_env, param.name.clone(), t);
        out.push(t);
    }
    Ok(out)
}

fn call_write_backs(
    f: &TypedFn,
    receiver_temp: Option<Temp>,
    arg_temps: &[Temp],
) -> Vec<(usize, Temp)> {
    let mut write_backs = Vec::new();
    let arg0_is_receiver = receiver_temp.is_some();
    if let Some(st) = receiver_temp {
        if matches!(f.receiver.as_ref().map(|(m, _)| *m), Some(AccessMode::Mut)) {
            write_backs.push((0, st));
        }
    }
    for (i, param) in f.params.iter().enumerate() {
        if param.mode == AccessMode::Mut {
            let args_idx = if arg0_is_receiver { i + 1 } else { i };
            write_backs.push((args_idx, arg_temps[i]));
        }
    }
    write_backs
}

fn lower_call(
    callee: &CalleeKey,
    receiver: &Option<Box<TypedExpr>>,
    args: &[TypedCallArg],
    result_ty: &Type,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    if let CalleeKey::Method(_, m) = callee {
        if m == "format" {
            if let Some(recv) = receiver {
                if let Some(_k) = crate::sema::types::scalar_format_bound(&recv.ty) {
                    let src = lower_expr(recv, b, env)?;
                    let Type::String(n_expr) = result_ty else {
                        return Err(LowerError::internal(
                            "scalar `.format()` result is not `String[..N]`".to_string(),
                        ));
                    };
                    let capacity = eval_len_expr(n_expr)?;
                    let dst = b.fresh(result_ty.clone());
                    b.emit(Inst::FormatScalar {
                        dst,
                        src,
                        src_ty: recv.ty.clone(),
                        capacity,
                    });
                    return Ok(dst);
                }
            }
        }
    }
    let member_is_init =
        matches!(callee, CalleeKey::Method(_, m) | CalleeKey::MethodInstance(_, m) if m == "init");
    let f = resolve_fn(b.prog(), callee).ok_or_else(|| missing_callee(b.prog(), callee))?;
    let key = callee.spelling();

    if member_is_init {
        return lower_init_call(f, &key, args, result_ty, b, env);
    }

    let mode = f.receiver.as_ref().map(|(m, _)| *m);
    match (receiver, mode) {
        (Some(recv_expr), Some(AccessMode::Mut)) => {
            let (self_temp, recv_wb) = lower_mut_arg_place(recv_expr, b, env)?;
            let mut nested_mut_writebacks = Vec::new();
            if let Some(place) = recv_wb {
                nested_mut_writebacks.push((place, self_temp));
            }
            let arg_temps =
                bind_args(f, args, Some(self_temp), b, env, &mut nested_mut_writebacks)?;
            let write_backs = call_write_backs(f, Some(self_temp), &arg_temps);
            let mut call_args = vec![self_temp];
            call_args.extend(arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit(Inst::Call {
                dst,
                write_backs,
                key,
                args: call_args,
            });
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
        (Some(recv_expr), Some(AccessMode::Read | AccessMode::Take)) => {
            let recv_temp = lower_expr(recv_expr, b, env)?;
            let mut nested_mut_writebacks = Vec::new();
            let arg_temps =
                bind_args(f, args, Some(recv_temp), b, env, &mut nested_mut_writebacks)?;
            let write_backs = call_write_backs(f, Some(recv_temp), &arg_temps);
            let mut call_args = vec![recv_temp];
            call_args.extend(arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit(Inst::Call {
                dst,
                write_backs,
                key,
                args: call_args,
            });
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
        _ => {
            let mut nested_mut_writebacks = Vec::new();
            let arg_temps = bind_args(f, args, None, b, env, &mut nested_mut_writebacks)?;
            let write_backs = call_write_backs(f, None, &arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit(Inst::Call {
                dst,
                write_backs,
                key,
                args: arg_temps,
            });
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
    }
}

fn lower_init_call(
    f: &TypedFn,
    key: &str,
    args: &[TypedCallArg],
    result_ty: &Type,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    let self_ty = f
        .receiver
        .as_ref()
        .map(|(_, t)| t.clone())
        .ok_or_else(|| LowerError::internal("`init` has no receiver type"))?;
    let self_temp = b.fresh(self_ty.clone());
    let mut nested_mut_writebacks = Vec::new();
    let arg_temps = bind_args(f, args, Some(self_temp), b, env, &mut nested_mut_writebacks)?;
    let write_backs = call_write_backs(f, Some(self_temp), &arg_temps);
    let mut call_args = vec![self_temp];
    call_args.extend(arg_temps);
    let body_dst = b.fresh(f.ret.clone());
    b.emit(Inst::Call {
        dst: body_dst,
        write_backs,
        key: key.to_string(),
        args: call_args,
    });
    if mwir::is_slotmap_type(&self_ty) {
        b.emit(Inst::SlotMapMint { map: self_temp });
    }
    for (place, t) in nested_mut_writebacks {
        lower_place_write(place, t, b, env)?;
    }
    match &f.ret {
        Type::Unit => Ok(self_temp),
        Type::Result(_, _) => {
            let Type::Result(_, err_ty) = result_ty else {
                return Err(LowerError::internal(
                    "`init`'s own call-result type is not `Result` even though its body's own return type is",
                ));
            };
            let tag_t = b.fresh(Type::U64);
            b.emit(Inst::EnumTag {
                dst: tag_t,
                src: body_dst,
            });
            let ok_const = b.fresh(Type::U64);
            b.emit(Inst::ConstInt {
                dst: ok_const,
                ty: Type::U64,
                value: value::RESULT_OK as i128,
            });
            let is_ok = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst: is_ok,
                op: BinOp::Eq,
                ty: Type::U64,
                lhs: tag_t,
                rhs: ok_const,
            });
            let result = b.fresh(result_ty.clone());
            let else_fixup = b.emit(Inst::JumpIfFalse {
                cond: is_ok,
                target: usize::MAX,
            });
            b.emit(Inst::MakeEnum {
                dst: result,
                tag: value::RESULT_OK,
                payload: vec![self_temp],
            });
            let end_fixup = b.emit(Inst::Jump { target: usize::MAX });
            let else_pos = b.here();
            b.patch_jump(else_fixup, else_pos);
            let err_payload = b.fresh((**err_ty).clone());
            b.emit(Inst::EnumPayload {
                dst: err_payload,
                src: body_dst,
                index: 0,
            });
            b.emit(Inst::MakeEnum {
                dst: result,
                tag: value::RESULT_ERR,
                payload: vec![err_payload],
            });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            Ok(result)
        }
        other => Err(LowerError::internal(format!(
            "`init` with a non-standard return type ({other:?}) — unreachable per sema"
        ))),
    }
}

fn lower_try_sync(
    value_temp: Temp,
    value_ty: &Type,
    conv: &Option<CalleeKey>,
    b: &mut FnBuilder,
) -> Result<Temp, LowerError> {
    let (ok_ty, err_ty) = match value_ty {
        Type::Result(o, e) => ((**o).clone(), (**e).clone()),
        _ => {
            return Err(LowerError::unimplemented(
                "`?` on a non-`Result` (e.g. `Option`) value in a synchronous body is",
            ));
        }
    };
    let tag_t = b.fresh(Type::U64);
    b.emit(Inst::EnumTag {
        dst: tag_t,
        src: value_temp,
    });
    let ok_const = b.fresh(Type::U64);
    b.emit(Inst::ConstInt {
        dst: ok_const,
        ty: Type::U64,
        value: value::RESULT_OK as i128,
    });
    let is_ok = b.fresh(Type::Bool);
    b.emit(Inst::Compare {
        dst: is_ok,
        op: BinOp::Eq,
        ty: Type::U64,
        lhs: tag_t,
        rhs: ok_const,
    });
    let err_fixup = b.emit(Inst::JumpIfFalse {
        cond: is_ok,
        target: usize::MAX,
    });
    let ok_payload = b.fresh(ok_ty);
    b.emit(Inst::EnumPayload {
        dst: ok_payload,
        src: value_temp,
        index: 0,
    });
    let after_fixup = b.emit(Inst::Jump { target: usize::MAX });
    let err_pos = b.here();
    b.patch_jump(err_fixup, err_pos);
    let err_payload = b.fresh(err_ty);
    b.emit(Inst::EnumPayload {
        dst: err_payload,
        src: value_temp,
        index: 0,
    });
    let Type::Result(_, ret_err) = &b.ret else {
        return Err(LowerError::internal(
            "`?` used inside a fn whose own declared return type is not `Result`".to_string(),
        ));
    };
    let target_ty = (**ret_err).clone();
    let converted = lower_from_conversion(err_payload, conv, target_ty, b)?;
    let ret_enum = b.fresh(b.ret.clone());
    b.emit(Inst::MakeEnum {
        dst: ret_enum,
        tag: value::RESULT_ERR,
        payload: vec![converted],
    });
    b.emit(Inst::Return {
        value: Some(ret_enum),
    });
    let after_pos = b.here();
    b.patch_jump(after_fixup, after_pos);
    Ok(ok_payload)
}

fn lower_from_conversion(
    err_payload: Temp,
    conv: &Option<CalleeKey>,
    target_ty: Type,
    b: &mut FnBuilder,
) -> Result<Temp, LowerError> {
    let Some(key) = conv else {
        return Ok(err_payload);
    };
    if resolve_fn(b.prog(), key).is_none() {
        return Err(LowerError::internal(format!(
            "`?` conversion `{}` has no TypedFn (deriving(From) must generate one)",
            key.spelling()
        )));
    }
    let dst = b.fresh(target_ty);
    b.emit(Inst::Call {
        dst,
        write_backs: Vec::new(),
        key: key.spelling(),
        args: vec![err_payload],
    });
    Ok(dst)
}

fn collapse_reserve_permit_if_needed(
    expr_ty: &Type,
    src: Temp,
    b: &mut FnBuilder<'_, '_>,
) -> Result<Temp, LowerError> {
    if !lower_shared::needs_collapse_reserve_permit(expr_ty, &b.temp_types[src.0]) {
        return Ok(src);
    }
    let dst = b.fresh(expr_ty.clone());
    lower_shared::emit_collapse_reserve_permit(dst, src, |inst| {
        b.emit(inst);
    });
    Ok(dst)
}

fn lower_expr(expr: &TypedExpr, b: &mut FnBuilder, env: &mut LEnv) -> Result<Temp, LowerError> {
    match &expr.kind {
        TypedExprKind::Int(text) => {
            let raw = value::parse_int_literal(text)
                .ok_or_else(|| LowerError::internal("invalid integer literal text"))?;
            match &expr.ty {
                Type::F32 => {
                    let dst = b.fresh(Type::F32);
                    b.emit(Inst::ConstFloat {
                        dst,
                        ty: Type::F32,
                        bits: (raw as f32).to_bits() as u64,
                    });
                    Ok(dst)
                }
                Type::F64 => {
                    let dst = b.fresh(Type::F64);
                    b.emit(Inst::ConstFloat {
                        dst,
                        ty: Type::F64,
                        bits: (raw as f64).to_bits(),
                    });
                    Ok(dst)
                }
                t => {
                    let dst = b.fresh(t.clone());
                    b.emit(Inst::ConstInt {
                        dst,
                        ty: t.clone(),
                        value: raw,
                    });
                    Ok(dst)
                }
            }
        }
        TypedExprKind::Float(text) => {
            let f: f64 = text
                .parse()
                .map_err(|_| LowerError::internal("invalid float literal text"))?;
            match &expr.ty {
                Type::F32 => {
                    let dst = b.fresh(Type::F32);
                    b.emit(Inst::ConstFloat {
                        dst,
                        ty: Type::F32,
                        bits: (f as f32).to_bits() as u64,
                    });
                    Ok(dst)
                }
                _ => {
                    let dst = b.fresh(Type::F64);
                    b.emit(Inst::ConstFloat {
                        dst,
                        ty: Type::F64,
                        bits: f.to_bits(),
                    });
                    Ok(dst)
                }
            }
        }
        TypedExprKind::Str(text) => {
            let bytes = value::decode_str(text);
            if let Type::String(n_expr) = &expr.ty {
                return emit_string_aggregate(&bytes, n_expr, &expr.ty, b);
            }
            let data = b.intern(bytes);
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstText { dst, data });
            Ok(dst)
        }
        TypedExprKind::BStr(text) => {
            let bytes = value::decode_bstr(text);
            let data = b.intern(bytes);
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstText { dst, data });
            Ok(dst)
        }
        TypedExprKind::Char(text) => {
            let c = value::decode_char(text);
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstChar { dst, value: c });
            Ok(dst)
        }
        TypedExprKind::Bool(v) => {
            let dst = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool { dst, value: *v });
            Ok(dst)
        }
        TypedExprKind::Unit => {
            let dst = b.fresh(Type::Unit);
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        TypedExprKind::Local(name) => {
            let t = env_lookup(env, name)
                .ok_or_else(|| LowerError::internal(format!("unbound local `{name}`")))?;
            collapse_reserve_permit_if_needed(&expr.ty, t, b)
        }
        TypedExprKind::Const(name) => {
            let v = crate::eval::interp::eval_const(b.prog(), name).map_err(|e| {
                LowerError::internal(format!(
                    "const `{name}` failed to evaluate during lowering \
                     (already proven to succeed by check_typed): {}",
                    e.message
                ))
            })?;
            emit_const_value(&v, &expr.ty, b)
        }
        TypedExprKind::Static(name) => {
            let addr = placed_static_addr(b.prog(), name)?;
            let dst = b.fresh(Type::U64);
            b.emit(Inst::ConstInt {
                dst,
                ty: Type::U64,
                value: addr as i128,
            });
            Ok(dst)
        }
        TypedExprKind::FnRef(_) => Err(LowerError::unimplemented(
            "a bare fn/method value reference is",
        )),
        TypedExprKind::Field(base, name) => {
            if let TypedExprKind::Static(sname) = &base.kind {
                let layout_name = match bodies::unwrap_own(base.ty.clone()) {
                    Type::Named(n, _) => n,
                    other => {
                        return Err(LowerError::internal(format!(
                            "placed static `{sname}` has non-named type {other:?}"
                        )));
                    }
                };
                let offset = runtime_layout_field_offset(&layout_name, name, b.prog())?;
                let base_temp = lower_expr(base, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::MmioRead {
                    dst,
                    base: base_temp,
                    offset,
                    ty: expr.ty.clone(),
                });
                return Ok(dst);
            }
            if let Some((static_expr, idx_expr, field_offset, elem_stride, len)) =
                placed_struct_array_scalar_field(base, name, b.prog())?
            {
                let base_temp = lower_expr(&static_expr, b, env)?;
                let idx_temp = lower_expr(&idx_expr, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::PlacedIndexGet {
                    dst,
                    base: base_temp,
                    field_offset,
                    index: idx_temp,
                    len,
                    elem_stride,
                    ty: expr.ty.clone(),
                });
                return Ok(dst);
            }
            let base_temp = lower_expr(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            if let Type::Named(sname, _) = &base_ty {
                if matches!(sname.as_str(), "Duration" | "Instant") && name == "nanos" {
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::Copy {
                        dst,
                        src: base_temp,
                    });
                    return Ok(dst);
                }
            }
            let idx = field_index(b.prog(), &base_ty, name)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::Project {
                dst,
                base: base_temp,
                index: idx,
            });
            Ok(dst)
        }
        TypedExprKind::Index(base, idx_expr) => {
            if let Some((static_expr, field_offset, elem_stride, len)) =
                placed_array_field_index(base, b.prog())?
            {
                let base_temp = lower_expr(&static_expr, b, env)?;
                let idx_temp = lower_expr(idx_expr, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::PlacedIndexGet {
                    dst,
                    base: base_temp,
                    field_offset,
                    index: idx_temp,
                    len,
                    elem_stride,
                    ty: expr.ty.clone(),
                });
                return Ok(dst);
            }
            let base_temp = lower_expr(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            if matches!(base_ty, Type::Bytes(None)) {
                let idx_temp = lower_expr(idx_expr, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::BytesIndexGet {
                    dst,
                    base: base_temp,
                    index: idx_temp,
                });
                return Ok(dst);
            }
            if let Type::Bytes(Some(n_expr)) = &base_ty {
                let cap = eval_len_expr(n_expr)?;
                let i = match &idx_expr.kind {
                    TypedExprKind::Int(text) => {
                        let raw = value::parse_int_literal(text)
                            .ok_or_else(|| LowerError::internal("invalid integer literal text"))?;
                        usize::try_from(raw)
                            .map_err(|_| LowerError::internal("Bytes index out of range"))?
                    }
                    _ => {
                        return Err(LowerError::unimplemented(
                            "indexing `Bytes[N]` with a non-literal index is",
                        ));
                    }
                };
                if i >= cap {
                    return Err(LowerError::internal(format!(
                        "Bytes index {i} out of length {cap}"
                    )));
                }
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Project {
                    dst,
                    base: base_temp,
                    index: i,
                });
                return Ok(dst);
            }
            if let Type::String(n_expr) = &base_ty {
                let cap = eval_len_expr(n_expr)?;
                let i = match &idx_expr.kind {
                    TypedExprKind::Int(text) => {
                        let raw = value::parse_int_literal(text)
                            .ok_or_else(|| LowerError::internal("invalid integer literal text"))?;
                        usize::try_from(raw)
                            .map_err(|_| LowerError::internal("String index out of range"))?
                    }
                    _ => {
                        return Err(LowerError::unimplemented(
                            "indexing `String[..N]` with a non-literal index is",
                        ));
                    }
                };
                if i >= cap {
                    return Err(LowerError::internal(format!(
                        "String index {i} out of capacity {cap}"
                    )));
                }
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Project {
                    dst,
                    base: base_temp,
                    index: 1 + i,
                });
                return Ok(dst);
            }
            let len = eval_array_len(&base.ty)?;
            if let Some(i) = literal_array_index_elide(idx_expr, len)? {
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Project {
                    dst,
                    base: base_temp,
                    index: i,
                });
                return Ok(dst);
            }
            let idx_temp = lower_expr(idx_expr, b, env)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::IndexGet {
                dst,
                base: base_temp,
                index: idx_temp,
                len,
            });
            Ok(dst)
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            if receiver.is_none()
                && callee.spelling().as_str() == "__wrela_pixels_f64_bits_to_f32"
                && args.len() == 1
            {
                let value = args[0]
                    .value
                    .as_ref()
                    .ok_or_else(|| LowerError::internal("P7 f64 bitcast argument is missing"))?;
                let src = lower_expr(value, b, env)?;
                let as_f64 = b.fresh(Type::F64);
                b.emit(Inst::Copy { dst: as_f64, src });
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Convert {
                    dst,
                    ty: Type::F32,
                    src: as_f64,
                    abort: mwir::convert_abort_message(&Type::F32),
                });
                Ok(dst)
            } else if receiver.is_none()
                && (matches!(
                    callee.spelling().as_str(),
                    "__wrela_pixels_f32_to_bits" | "__wrela_pixels_f32_from_bits"
                ) || matches!(callee, CalleeKey::Fn(name)
                    if matches!(name.as_str(), "pixels_f32_to_bits" | "pixels_f32_from_bits")
                        && b.prog().fn_decl_modules.get(name).map(String::as_str)
                            == Some("core.render_raster")
                        && b.prog().fn_decl_names.get(name).map(String::as_str)
                            == Some(name.as_str())))
                && args.len() == 1
            {
                let value = args[0]
                    .value
                    .as_ref()
                    .ok_or_else(|| LowerError::internal("P7 f32 bitcast argument is missing"))?;
                let src = lower_expr(value, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Copy { dst, src });
                Ok(dst)
            } else {
                lower_call(callee, receiver, args, &expr.ty, b, env)
            }
        }
        TypedExprKind::CallValue(..) => Err(LowerError::unimplemented(
            "calling a closure/fn value indirectly is",
        )),
        TypedExprKind::ToScalar(inner) => {
            let src = lower_expr(inner, b, env)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::Convert {
                dst,
                ty: expr.ty.clone(),
                src,
                abort: mwir::convert_abort_message(&expr.ty),
            });
            Ok(dst)
        }
        TypedExprKind::Neg(inner) => {
            if let TypedExprKind::Int(text) = &inner.kind {
                let raw = value::parse_int_literal(text)
                    .ok_or_else(|| LowerError::internal("invalid integer literal text"))?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::ConstInt {
                    dst,
                    ty: expr.ty.clone(),
                    value: -raw,
                });
                Ok(dst)
            } else {
                let src = lower_expr(inner, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Neg {
                    dst,
                    ty: expr.ty.clone(),
                    src,
                    abort: mwir::neg_abort_message(),
                });
                Ok(dst)
            }
        }
        TypedExprKind::BitNot(inner) => {
            let src = lower_expr(inner, b, env)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::BitNot {
                dst,
                ty: expr.ty.clone(),
                src,
            });
            Ok(dst)
        }
        TypedExprKind::Take(inner) => {
            let t = lower_expr(inner, b, env)?;
            collapse_reserve_permit_if_needed(&expr.ty, t, b)
        }
        TypedExprKind::Try(inner, conv) => {
            let v = lower_expr(inner, b, env)?;
            lower_try_sync(v, &inner.ty, conv, b)
        }
        TypedExprKind::Binary(op, l, r) => lower_binary(*op, l, r, expr, b, env),
        TypedExprKind::OpCall(key, l, r) => {
            let lv = lower_expr(l, b, env)?;
            let rv = lower_expr(r, b, env)?;
            let f = resolve_fn(b.prog(), key).ok_or_else(|| missing_callee(b.prog(), key))?;
            let dst = b.fresh(f.ret.clone());
            b.emit(Inst::Call {
                dst,
                write_backs: Vec::new(),
                key: key.spelling(),
                args: vec![lv, rv],
            });
            Ok(dst)
        }
        TypedExprKind::Is(inner, pattern) => {
            let v = lower_expr(inner, b, env)?;
            let mut bindings = BTreeMap::new();
            collect_pattern_bindings(pattern, &mut bindings, b);
            let test = lower_pattern_test(pattern, v, &bindings, b)?;
            for (n, t) in bindings {
                env_insert(env, n, t);
            }
            Ok(test)
        }
        TypedExprKind::Not(inner) => {
            let v = lower_expr(inner, b, env)?;
            let dst = b.fresh(Type::Bool);
            b.emit(Inst::Not { dst, src: v });
            Ok(dst)
        }
        TypedExprKind::And(l, r) => {
            let lv = lower_expr(l, b, env)?;
            let result = b.fresh(Type::Bool);
            let false_fixup = b.emit(Inst::JumpIfFalse {
                cond: lv,
                target: usize::MAX,
            });
            let rv = lower_expr(r, b, env)?;
            b.emit(Inst::Copy {
                dst: result,
                src: rv,
            });
            let end_fixup = b.emit(Inst::Jump { target: usize::MAX });
            let false_pos = b.here();
            b.patch_jump(false_fixup, false_pos);
            b.emit(Inst::ConstBool {
                dst: result,
                value: false,
            });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            Ok(result)
        }
        TypedExprKind::Or(l, r) => {
            let lv = lower_expr(l, b, env)?;
            let result = b.fresh(Type::Bool);
            let eval_r_fixup = b.emit(Inst::JumpIfFalse {
                cond: lv,
                target: usize::MAX,
            });
            b.emit(Inst::ConstBool {
                dst: result,
                value: true,
            });
            let end_fixup = b.emit(Inst::Jump { target: usize::MAX });
            let eval_r_pos = b.here();
            b.patch_jump(eval_r_fixup, eval_r_pos);
            let rv = lower_expr(r, b, env)?;
            b.emit(Inst::Copy {
                dst: result,
                src: rv,
            });
            let end_pos = b.here();
            b.patch_jump(end_fixup, end_pos);
            Ok(result)
        }
        TypedExprKind::EnumConstruct {
            enum_name,
            variant,
            args,
        } => {
            let idx = variant_index(b.prog(), enum_name, variant)?;
            let mut arg_temps = Vec::with_capacity(args.len());
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                arg_temps.push(lower_expr(a, b, env)?);
            }
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::MakeEnum {
                dst,
                tag: idx,
                payload: arg_temps,
            });
            Ok(dst)
        }
        TypedExprKind::Closure { .. } => Err(LowerError::unimplemented("a closure literal is")),
        TypedExprKind::Tuple(items) => {
            let mut elems = Vec::with_capacity(items.len());
            for i in items {
                elems.push(lower_expr(i, b, env)?);
            }
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        TypedExprKind::List(items) => {
            let mut elems = Vec::with_capacity(items.len());
            for i in items {
                elems.push(lower_expr(i, b, env)?);
            }
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        TypedExprKind::StructLiteral { name, fields } => {
            let Type::Named(sname, targs) = &expr.ty else {
                return Err(LowerError::internal("struct literal type is not `Named`"));
            };
            debug_assert_eq!(name, sname);
            if matches!(sname.as_str(), "Duration" | "Instant") {
                if fields.len() != 1 {
                    return Err(LowerError::internal(format!(
                        "`{sname}` construction must supply exactly one field"
                    )));
                }
                let nanos = lower_expr(&fields[0].1, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Copy { dst, src: nanos });
                return Ok(dst);
            }
            let s = resolve_struct(b.prog(), sname, targs)
                .ok_or_else(|| missing_struct(b.prog(), sname))?;
            let mut slots: Vec<Option<Temp>> = vec![None; s.fields.len()];
            for (fname, fval) in fields {
                let idx = s
                    .fields
                    .iter()
                    .position(|f| f == fname)
                    .ok_or_else(|| LowerError::internal(format!("unknown field `{fname}`")))?;
                slots[idx] = Some(lower_expr(fval, b, env)?);
            }
            for (i, fname) in s.fields.iter().enumerate() {
                if slots[i].is_none() {
                    let default = s.field_defaults.get(fname).ok_or_else(|| {
                        LowerError::internal(format!(
                            "field `{fname}` has neither a supplied value nor a default"
                        ))
                    })?;
                    let mut fresh_env: LEnv = vec![BTreeMap::new()];
                    slots[i] = Some(lower_expr(default, b, &mut fresh_env)?);
                }
            }
            let elems: Vec<Temp> = slots
                .into_iter()
                .map(|s| s.expect("every slot filled above"))
                .collect();
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        TypedExprKind::Panic(msg) => {
            let text = assert_message_text(msg)?;
            b.emit(Inst::AssertFail {
                message: Some(format!("panic: {text}")),
            });
            Ok(b.fresh(expr.ty.clone()))
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            args,
            ..
        } if crate::sema::bodies::is_device_transport_intrinsic(key) => match key.as_str() {
            "Device.take_irq" => {
                let Some(driver) = b.owner_struct.clone() else {
                    return Err(LowerError::internal(
                        "`Device.take_irq` reached lowering outside a `@driver` member".to_string(),
                    ));
                };
                let _ = receiver;
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::LoadIrqVector { dst, driver });
                Ok(dst)
            }
            "Device.read_capacity_sectors" => {
                let capacity = b.blk_capacity_sectors().ok_or_else(|| {
                    LowerError::unimplemented(
                        "`read_capacity_sectors`: this image declares no \
                             `capacity_sectors=` on its `img.device` (plans/M7.md item E1: \
                             capacity is an image-declared build constant, not a register)",
                    )
                })?;
                let ok_payload = b.fresh(Type::U64);
                b.emit(Inst::ConstInt {
                    dst: ok_payload,
                    ty: Type::U64,
                    value: capacity as i128,
                });
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::MakeEnum {
                    dst,
                    tag: value::RESULT_OK,
                    payload: vec![ok_payload],
                });
                Ok(dst)
            }
            "Device.negotiate" | "VirtQueue.configure" => {
                let src = match (key.as_str(), receiver, args.as_slice()) {
                    ("Device.negotiate", Some(state), _) => lower_expr(state, b, env)?,
                    ("VirtQueue.configure", _, args) => {
                        let (_, pool) =
                            args.iter().find(|(l, _)| l == "pool").ok_or_else(|| {
                                LowerError::internal(
                                    "`VirtQueue.configure` reached lowering without `pool=`"
                                        .to_string(),
                                )
                            })?;
                        lower_expr(pool, b, env)?
                    }
                    _ => {
                        return Err(LowerError::internal(format!(
                            "sealed-transport intrinsic `{key}` reached lowering without \
                                 its operand"
                        )));
                    }
                };
                let Type::Result(ok_ty, _) = &expr.ty else {
                    return Err(LowerError::internal(format!(
                        "`{key}`'s typed result is not a `Result`"
                    )));
                };
                let payload = b.fresh((**ok_ty).clone());
                b.emit(Inst::Copy { dst: payload, src });
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::MakeEnum {
                    dst,
                    tag: value::RESULT_OK,
                    payload: vec![payload],
                });
                Ok(dst)
            }
            "Device.start" | "Device.claim" | "Device.map_partition" => {
                let src = match (key.as_str(), receiver, args.first()) {
                    ("Device.claim", _, Some((_, cap))) => lower_expr(cap, b, env)?,
                    ("Device.map_partition" | "Device.start", Some(state), _) => {
                        lower_expr(state, b, env)?
                    }
                    _ => {
                        return Err(LowerError::internal(format!(
                            "sealed-transport intrinsic `{key}` reached lowering without \
                                 its operand"
                        )));
                    }
                };
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::Copy { dst, src });
                Ok(dst)
            }
            "Device.reset" => {
                let device = match receiver {
                    Some(state) => lower_expr(state, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`Device.reset` reached lowering without a RunningDevice receiver"
                                .to_string(),
                        ));
                    }
                };
                let queue = match args.first() {
                    Some((_, q)) => lower_expr(q, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`Device.reset` reached lowering without a queue argument".to_string(),
                        ));
                    }
                };
                let dst = b.fresh(expr.ty.clone());
                let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                    .map_err(LowerError::internal)?;
                lower_queue::expand_device_reset(dst, device, queue, depth, &mut LowerQueueSink(b))
                    .map_err(LowerError::internal)?;
                Ok(dst)
            }
            other => Err(LowerError::internal(format!(
                "unknown sealed-transport intrinsic `{other}`"
            ))),
        },
        TypedExprKind::Intrinsic { key, .. } if crate::sema::bodies::is_irq_cap_intrinsic(key) => {
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            args,
            ..
        } if crate::sema::bodies::is_interrupt_cell_intrinsic(key) => {
            lower_interrupt_cell_intrinsic(key, receiver.as_deref(), args, &expr.ty, b, env)
        }
        TypedExprKind::Intrinsic { key, args, .. }
            if crate::sema::bodies::is_wake_intrinsic(key) =>
        {
            let Some((_, task)) = args.iter().find(|(l, _)| l == "task") else {
                return Err(LowerError::internal(
                    "`wake` with no task argument".to_string(),
                ));
            };
            let TypedExprKind::FnRef(callee) = &task.kind else {
                return Err(LowerError::internal(
                    "`wake` task is not a FnRef".to_string(),
                ));
            };
            let driver = match callee {
                crate::sema::typed::CalleeKey::Method(s, _) => s.clone(),
                crate::sema::typed::CalleeKey::MethodInstance(ikey, _) => ikey
                    .strip_prefix("struct:")
                    .unwrap_or(ikey.as_str())
                    .split('[')
                    .next()
                    .unwrap_or(ikey.as_str())
                    .to_string(),
                _ => {
                    return Err(LowerError::internal(
                        "`wake` task is not a driver method".to_string(),
                    ));
                }
            };
            b.emit(Inst::Wake { driver });
            let dst = b.fresh(Type::Unit);
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
            ..
        } if crate::sema::bodies::is_mmio_access_intrinsic(key) => {
            let Some(mmio) = receiver else {
                return Err(LowerError::internal(
                    "an MMIO access with no receiver".to_string(),
                ));
            };
            let (layout_name, register) = mmio_access_names(&mmio.ty, args)?;
            let offset = mmio_register_offset(&layout_name, &register, b.prog())?;
            let Some(ty) = type_arg.clone() else {
                return Err(LowerError::internal(
                    "an MMIO access with no register type".to_string(),
                ));
            };
            let base = lower_expr(mmio, b, env)?;
            if key == "Mmio.read" {
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::MmioRead {
                    dst,
                    base,
                    offset,
                    ty,
                });
                Ok(dst)
            } else {
                let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
                    return Err(LowerError::internal(
                        "an `Mmio.write` with no value argument".to_string(),
                    ));
                };
                let value = lower_expr(v, b, env)?;
                b.emit(Inst::MmioWrite {
                    base,
                    offset,
                    ty,
                    value,
                });
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::ConstUnit { dst });
                Ok(dst)
            }
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
            ..
        } if crate::sema::bodies::is_mmio_atomic_intrinsic(key) => {
            let Some(mmio) = receiver else {
                return Err(LowerError::internal("atomic MMIO access has no receiver"));
            };
            let (layout_name, register) = mmio_access_names(&mmio.ty, args)?;
            let field_offset = mmio_register_offset(&layout_name, &register, b.prog())?;
            let width: u8 = match type_arg {
                Some(Type::U32) => 4,
                Some(Type::U64) => 8,
                other => {
                    return Err(LowerError::internal(format!(
                        "atomic MMIO has unsupported scalar type {other:?}"
                    )));
                }
            };
            let base = lower_expr(mmio, b, env)?;
            let index = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: index,
                ty: Type::Usize,
                value: 0,
            });
            let mut value = || -> Result<Temp, LowerError> {
                let (_, value) = args
                    .iter()
                    .find(|(label, _)| label == "value")
                    .ok_or_else(|| LowerError::internal("atomic MMIO RMW has no value"))?;
                lower_expr(value, b, env)
            };
            match key.as_str() {
                "MmioAtomic.load_acquire" => {
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::PlacedInterruptCellLoadAcquire {
                        dst,
                        base,
                        field_offset,
                        index,
                        len: 1,
                        elem_stride: u64::from(width),
                        width,
                    });
                    Ok(dst)
                }
                "MmioAtomic.swap_acquire" | "MmioAtomic.fetch_or_release" => {
                    let value = value()?;
                    let dst = b.fresh(expr.ty.clone());
                    if key == "MmioAtomic.swap_acquire" {
                        b.emit(Inst::PlacedInterruptCellSwapAcquire {
                            dst,
                            base,
                            field_offset,
                            index,
                            len: 1,
                            elem_stride: u64::from(width),
                            width,
                            value,
                        });
                    } else {
                        b.emit(Inst::PlacedInterruptCellFetchOrRelease {
                            dst,
                            base,
                            field_offset,
                            index,
                            len: 1,
                            elem_stride: u64::from(width),
                            width,
                            value,
                        });
                    }
                    Ok(dst)
                }
                other => Err(LowerError::internal(format!(
                    "unknown atomic MMIO intrinsic `{other}`"
                ))),
            }
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
            ..
        } if crate::sema::bodies::is_untrusted_narrowing_intrinsic(key) => {
            lower_untrusted_checked_le(expr, receiver, type_arg, args, b, env)
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
            ..
        } if crate::sema::bodies::is_queue_op_intrinsic(key) => match key.as_str() {
            "VirtQueue.prepare_block" => {
                let parts = lower_shared::unpack_prepare_block_args(args).map_err(|e| match e {
                    lower_shared::PrepareBlockUnpackError::Missing(label) => LowerError::internal(
                        format!("`prepare_block` reached lowering without `{label}`"),
                    ),
                    lower_shared::PrepareBlockUnpackError::NonLiteralDeviceWrites => {
                        LowerError::unimplemented(
                            "`prepare_block`'s `device_writes_payload=` as a non-literal bool \
                                     (revision 0.1 requires a literal `true`/`false`)",
                        )
                    }
                })?;
                let queue = match receiver {
                    Some(q) => lower_expr(q, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`prepare_block` reached lowering without a queue receiver".to_string(),
                        ));
                    }
                };
                let permit_t = lower_expr(parts.permit, b, env)?;
                let header_t = lower_expr(parts.header, b, env)?;
                let payload_t = lower_expr(parts.payload, b, env)?;
                let status_t = lower_expr(parts.status, b, env)?;
                let payload_len = lower_shared::prepare_block_payload_len(
                    &parts.payload.ty,
                    b.prog(),
                )
                .map_err(|e| match e {
                    lower_shared::PreparePayloadLenError::NoDmaSize => LowerError::internal(
                        "`prepare_block`'s payload type has no `@layout(dma)` size \
                                         in this program"
                            .to_string(),
                    ),
                    lower_shared::PreparePayloadLenError::BadSectorMultiple(n) => {
                        LowerError::unimplemented(&format!(
                            "`prepare_block` with payload layout size {n}: the \
                                         virtio-blk model requires a positive multiple of 512 \
                                         (SECTOR_SIZE)"
                        ))
                    }
                })?;
                let dst = b.fresh(expr.ty.clone());
                let _ = permit_t;
                let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                    .map_err(LowerError::internal)?;
                lower_queue::expand_prepare(
                    dst,
                    queue,
                    header_t,
                    payload_t,
                    status_t,
                    parts.device_writes,
                    payload_len as u32,
                    depth,
                    &mut LowerQueueSink(b),
                )
                .map_err(LowerError::internal)?;
                Ok(dst)
            }
            "VirtQueue.reserve" => {
                let _ = args
                    .iter()
                    .find(|(l, _)| l == "descriptors")
                    .ok_or_else(|| {
                        LowerError::internal(
                            "`reserve` reached lowering without `descriptors=`".to_string(),
                        )
                    })?;
                let _ = receiver;
                let permit = b.fresh(Type::Named("QueuePermit".to_string(), vec![]));
                b.emit(Inst::ConstInt {
                    dst: permit,
                    ty: Type::U64,
                    value: 0,
                });
                if matches!(&expr.ty, Type::Result(_, _)) {
                    let dst = b.fresh(expr.ty.clone());
                    b.emit(Inst::MakeEnum {
                        dst,
                        tag: value::RESULT_OK,
                        payload: vec![permit],
                    });
                    Ok(dst)
                } else {
                    Ok(permit)
                }
            }
            "VirtQueue.publish" => {
                let op = args.iter().find(|(l, _)| l == "operation").ok_or_else(|| {
                    LowerError::internal(
                        "`publish` reached lowering without `operation=`".to_string(),
                    )
                })?;
                let queue = match receiver {
                    Some(q) => lower_expr(q, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`publish` reached lowering without a queue receiver".to_string(),
                        ));
                    }
                };
                let operation = lower_expr(&op.1, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                    .map_err(LowerError::internal)?;
                lower_queue::expand_publish(dst, queue, operation, depth, &mut LowerQueueSink(b))
                    .map_err(LowerError::internal)?;
                Ok(dst)
            }
            "VirtQueue.reject" => {
                for (_, a) in args {
                    let _ = lower_expr(a, b, env)?;
                }
                let dst = b.fresh(expr.ty.clone());
                b.emit(Inst::ConstInt {
                    dst,
                    ty: Type::U64,
                    value: 0,
                });
                let _ = receiver;
                Ok(dst)
            }
            "VirtQueue.drain" => {
                let queue = match receiver {
                    Some(q) => lower_expr(q, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`drain` reached lowering without a queue receiver".to_string(),
                        ));
                    }
                };
                let max_val = match type_arg {
                    Some(Type::Named(_, targs)) => match targs.first() {
                        Some(crate::sema::types::TypeArg::Bound(
                            crate::syntax::ast::Expr::Int(_, text),
                        )) => text
                            .parse::<u16>()
                            .map_err(|_| LowerError::internal(format!("drain max `{text}`")))?,
                        _ => {
                            return Err(LowerError::internal(
                                "`drain` type_arg Bound is not an integer literal".to_string(),
                            ));
                        }
                    },
                    _ => {
                        return Err(LowerError::internal(
                            "`drain` reached lowering without a folded max Bound".to_string(),
                        ));
                    }
                };
                let _ = args;
                let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                    .map_err(LowerError::internal)?;
                lower_queue::expand_drain(queue, max_val, depth, &mut LowerQueueSink(b))
                    .map_err(LowerError::internal)?;
                let dst = b.fresh(Type::Unit);
                b.emit(Inst::ConstUnit { dst });
                Ok(dst)
            }
            "VirtQueue.suppress_interrupts" => {
                let queue = match receiver {
                    Some(q) => lower_expr(q, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`suppress_interrupts` reached lowering without a queue receiver"
                                .to_string(),
                        ));
                    }
                };
                let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                    .map_err(LowerError::internal)?;
                lower_queue::expand_suppress(queue, depth, &mut LowerQueueSink(b))
                    .map_err(LowerError::internal)?;
                let dst = b.fresh(Type::Unit);
                b.emit(Inst::ConstUnit { dst });
                Ok(dst)
            }
            "VirtQueue.claim" => {
                let receipt_arg = args.iter().find(|(l, _)| l == "receipt").ok_or_else(|| {
                    LowerError::internal("`claim` reached lowering without `receipt=`".to_string())
                })?;
                let queue = match receiver {
                    Some(q) => lower_expr(q, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`claim` reached lowering without a queue receiver".to_string(),
                        ));
                    }
                };
                let receipt = lower_expr(&receipt_arg.1, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                lower_queue::expand_claim(dst, queue, receipt, &mut LowerQueueSink(b))
                    .map_err(LowerError::internal)?;
                Ok(dst)
            }
            "VirtQueue.recover" => {
                let receipt_arg = args.iter().find(|(l, _)| l == "receipt").ok_or_else(|| {
                    LowerError::internal(
                        "`recover` reached lowering without `receipt=`".to_string(),
                    )
                })?;
                let queue = match receiver {
                    Some(q) => lower_expr(q, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`recover` reached lowering without a queue receiver".to_string(),
                        ));
                    }
                };
                let receipt = lower_expr(&receipt_arg.1, b, env)?;
                let dst = b.fresh(expr.ty.clone());
                let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                    .map_err(LowerError::internal)?;
                lower_queue::expand_recover(dst, queue, receipt, depth, &mut LowerQueueSink(b))
                    .map_err(LowerError::internal)?;
                Ok(dst)
            }
            "VirtQueue.reclaim" => {
                let queue = match receiver {
                    Some(q) => lower_expr(q, b, env)?,
                    None => {
                        return Err(LowerError::internal(
                            "`reclaim` reached lowering without a queue receiver".to_string(),
                        ));
                    }
                };
                let _ = args;
                let dst = b.fresh(expr.ty.clone());
                let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                    .map_err(LowerError::internal)?;
                lower_queue::expand_reclaim(dst, queue, depth, &mut LowerQueueSink(b))
                    .map_err(LowerError::internal)?;
                Ok(dst)
            }
            other => Err(LowerError::internal(format!(
                "unknown queue-op intrinsic `{other}`"
            ))),
        },
        TypedExprKind::Intrinsic { key, .. }
            if let Some(owner) = crate::sema::bodies::is_queue_op_deferred(key) =>
        {
            Err(LowerError::unimplemented(&format!("`{key}` ({owner}) is")))
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            args,
            ..
        } if key == "Array.map_take" || key == "Array.try_map_take" => {
            lower_array_map_take(key, receiver.as_deref(), args, &expr.ty, b, env)
        }
        TypedExprKind::Intrinsic { key, .. } if key == "dmb.ishst" || key == "dmb.ishld" => {
            let option = key.strip_prefix("dmb.").unwrap_or(key).to_string();
            b.emit(Inst::Dmb { option });
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        TypedExprKind::Intrinsic { key, .. } if key.as_str() == "now" => {
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::Now { dst });
            Ok(dst)
        }
        TypedExprKind::Intrinsic { key, const_arg, .. } if key.as_str() == "entropy" => {
            let n = const_arg.ok_or_else(|| {
                LowerError::internal("`entropy` Intrinsic missing const_arg (sema bug)".to_string())
            })?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::Entropy { dst, n });
            Ok(dst)
        }
        TypedExprKind::Intrinsic { .. } => Err(LowerError::unimplemented(
            "an `@image` builder intrinsic (reachable only inside the one `@image` fn, which is never lowered) is",
        )),
        TypedExprKind::PoolName(_) => Err(LowerError::unimplemented(
            "a bare pool name (the `@image` builder surface) is",
        )),
        TypedExprKind::Await(_) => Err(LowerError::unimplemented(
            "an `await` expression (FlowWir, plans/M6.md item B) is",
        )),
        TypedExprKind::Send(_) => Err(LowerError::unimplemented(
            "a `send` expression (FlowWir, plans/M6.md item B) is",
        )),
        TypedExprKind::GroupChild(_) => Err(LowerError::unimplemented(
            "a group-child reference (FlowWir, plans/M6.md item B) is",
        )),
    }
}

fn lower_binary(
    op: BinOp,
    l: &TypedExpr,
    r: &TypedExpr,
    expr: &TypedExpr,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    if op == BinOp::Add {
        if let (Type::String(ln), Type::String(rn), Type::String(_)) = (&l.ty, &r.ty, &expr.ty) {
            let lhs_cap = eval_len_expr(ln)?;
            let rhs_cap = eval_len_expr(rn)?;
            let lv = lower_expr(l, b, env)?;
            let rv = lower_expr(r, b, env)?;
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::StringConcat {
                dst,
                lhs: lv,
                rhs: rv,
                lhs_cap,
                rhs_cap,
            });
            return Ok(dst);
        }
    }
    let lv = lower_expr(l, b, env)?;
    let rv = lower_expr(r, b, env)?;
    let ty = l.ty.clone();
    let is_float = matches!(ty, Type::F32 | Type::F64);
    use BinOp::*;
    match op {
        Add | Sub | Mul => {
            let dst = b.fresh(expr.ty.clone());
            if is_float {
                b.emit(Inst::ArithWrapping {
                    dst,
                    op,
                    ty,
                    lhs: lv,
                    rhs: rv,
                });
            } else {
                b.emit(Inst::ArithChecked {
                    dst,
                    op,
                    ty,
                    lhs: lv,
                    rhs: rv,
                    abort: mwir::abort_message(op),
                });
            }
            Ok(dst)
        }
        AddW | SubW | MulW => {
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::ArithWrapping {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
            });
            Ok(dst)
        }
        Div | Rem => {
            let dst = b.fresh(expr.ty.clone());
            if is_float {
                b.emit(Inst::ArithWrapping {
                    dst,
                    op,
                    ty,
                    lhs: lv,
                    rhs: rv,
                });
            } else {
                b.emit(Inst::DivRem {
                    dst,
                    op,
                    ty,
                    lhs: lv,
                    rhs: rv,
                    abort_zero: mwir::div_zero_message(op),
                    abort_overflow: mwir::abort_message(op),
                });
            }
            Ok(dst)
        }
        Shl | Shr => {
            let dst = b.fresh(expr.ty.clone());
            let bits = int_bits(&ty)?;
            let lost = if op == Shl {
                Some(mwir::shift_lost_message())
            } else {
                None
            };
            b.emit(Inst::Shift {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
                bits,
                lost,
            });
            Ok(dst)
        }
        BitAnd | BitOr | BitXor => {
            let dst = b.fresh(expr.ty.clone());
            b.emit(Inst::Bitwise {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
            });
            Ok(dst)
        }
        Lt | Le | Gt | Ge | Eq | Ne => {
            let dst = b.fresh(Type::Bool);
            b.emit(Inst::Compare {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
            });
            Ok(dst)
        }
    }
}

fn emit_const_value(v: &Value, ty: &Type, b: &mut FnBuilder) -> Result<Temp, LowerError> {
    match v {
        Value::U8(_)
        | Value::U16(_)
        | Value::U32(_)
        | Value::U64(_)
        | Value::Usize(_)
        | Value::I8(_)
        | Value::I16(_)
        | Value::I32(_)
        | Value::I64(_)
        | Value::Isize(_) => {
            let raw = value::as_i128(v).expect("checked above");
            let dst = b.fresh(ty.clone());
            b.emit(Inst::ConstInt {
                dst,
                ty: ty.clone(),
                value: raw,
            });
            Ok(dst)
        }
        Value::F32(x) => {
            let dst = b.fresh(Type::F32);
            b.emit(Inst::ConstFloat {
                dst,
                ty: Type::F32,
                bits: x.to_bits() as u64,
            });
            Ok(dst)
        }
        Value::F64(x) => {
            let dst = b.fresh(Type::F64);
            b.emit(Inst::ConstFloat {
                dst,
                ty: Type::F64,
                bits: x.to_bits(),
            });
            Ok(dst)
        }
        Value::Bool(x) => {
            let dst = b.fresh(Type::Bool);
            b.emit(Inst::ConstBool { dst, value: *x });
            Ok(dst)
        }
        Value::Char(c) => {
            let dst = b.fresh(Type::Char);
            b.emit(Inst::ConstChar { dst, value: *c });
            Ok(dst)
        }
        Value::Unit => {
            let dst = b.fresh(Type::Unit);
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        Value::Str(bytes) => {
            if let Type::String(n_expr) = ty {
                return emit_string_aggregate(bytes, n_expr, ty, b);
            }
            let data = b.intern(bytes.clone());
            let dst = b.fresh(ty.clone());
            b.emit(Inst::ConstText { dst, data });
            Ok(dst)
        }
        Value::Bytes(bytes) => {
            let data = b.intern(bytes.clone());
            let dst = b.fresh(ty.clone());
            b.emit(Inst::ConstText { dst, data });
            Ok(dst)
        }
        Value::Tuple(items) | Value::Array(items) => {
            let elem_tys: Vec<Type> = match ty {
                Type::Tuple(ts) => ts.clone(),
                Type::Array(elem, _) => vec![(**elem).clone(); items.len()],
                other => {
                    return Err(LowerError::internal(format!(
                        "tuple/array const value with an unexpected type {other:?}"
                    )));
                }
            };
            let mut elems = Vec::with_capacity(items.len());
            for (item, ety) in items.iter().zip(elem_tys.iter()) {
                elems.push(emit_const_value(item, ety, b)?);
            }
            let dst = b.fresh(ty.clone());
            b.emit(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        Value::Struct(_) => Err(LowerError::unimplemented(
            "a struct-valued module `const` is",
        )),
        Value::Enum(_, _) => Err(LowerError::unimplemented(
            "an enum-valued module `const` is",
        )),
        Value::Fn(_) => Err(LowerError::unimplemented("a fn-valued module `const` is")),
        Value::Closure { .. } => Err(LowerError::unimplemented(
            "a closure-valued module `const` is",
        )),
        Value::ImageDecl(_) => Err(LowerError::unimplemented(
            "an `@image`-builder-valued module `const` is",
        )),
    }
}

fn struct_by_name<'p>(prog: &'p TypedProgram, name: &str) -> Option<&'p TypedStruct> {
    prog.structs
        .get(name)
        .or_else(|| prog.imported.structs.get(name).map(|value| value.as_ref()))
}

fn instantiation_by_key<'p>(prog: &'p TypedProgram, key: &str) -> Option<&'p TypedInstantiation> {
    prog.instantiations
        .get(key)
        .or_else(|| prog.imported.instantiations.get(key))
}

fn resolve_fn<'p>(prog: &'p TypedProgram, key: &CalleeKey) -> Option<&'p TypedFn> {
    match key {
        CalleeKey::Fn(name) => prog.fns.get(name).or_else(|| {
            prog.imported
                .fns
                .get(name)
                .map(|function| function.as_ref())
        }),
        CalleeKey::FnInstance(ikey) => match instantiation_by_key(prog, ikey) {
            Some(TypedInstantiation::Fn(f)) => Some(f),
            _ => None,
        },
        CalleeKey::Method(sname, member) => {
            if let Some(s) = struct_by_name(prog, sname) {
                return resolve_struct_member(s, member);
            }
            resolve_enum_member(enum_by_name(prog, sname)?, member)
        }
        CalleeKey::MethodInstance(ikey, member) => match instantiation_by_key(prog, ikey) {
            Some(TypedInstantiation::Struct(s)) => resolve_struct_member(s, member),
            _ => None,
        },
    }
}

fn resolve_struct_member<'p>(s: &'p TypedStruct, member: &str) -> Option<&'p TypedFn> {
    if member == "init" {
        s.init.as_ref()
    } else {
        s.methods.get(member).or_else(|| s.assoc_fns.get(member))
    }
}

fn resolve_enum_member<'p>(
    e: &'p crate::sema::typed::TypedEnum,
    member: &str,
) -> Option<&'p TypedFn> {
    e.methods.get(member).or_else(|| e.assoc_fns.get(member))
}

fn enum_by_name<'p>(
    prog: &'p TypedProgram,
    name: &str,
) -> Option<&'p crate::sema::typed::TypedEnum> {
    prog.enums
        .get(name)
        .or_else(|| prog.imported.enums.get(name).map(|value| value.as_ref()))
}

fn resolve_struct<'p>(
    prog: &'p TypedProgram,
    name: &str,
    targs: &[TypeArg],
) -> Option<&'p TypedStruct> {
    if targs.is_empty() {
        struct_by_name(prog, name)
    } else {
        let key = generics::canonical_key(InstKind::Struct, name, targs);
        match instantiation_by_key(prog, &key) {
            Some(TypedInstantiation::Struct(s)) => Some(s),
            _ => None,
        }
    }
}

fn missing_callee(prog: &TypedProgram, key: &CalleeKey) -> LowerError {
    let name = crate::eval::interp::callee_decl_name(key);
    if let Some(note) = prog.unresolvable_import_note(&name) {
        return LowerError::named(format!("`{name}` {note}"));
    }
    match key {
        CalleeKey::FnInstance(_) | CalleeKey::MethodInstance(_, _) => LowerError::unimplemented(
            "calling a callee not resolvable at lowering time (an unresolved generic instantiation)",
        ),
        _ => LowerError::unimplemented(format!(
            "calling `{}` — not declared in this module and not present in its import closure",
            key.spelling()
        )),
    }
}

fn missing_struct(prog: &TypedProgram, name: &str) -> LowerError {
    if let Some(note) = prog.unresolvable_import_note(name) {
        return LowerError::named(format!("`{name}` {note}"));
    }
    LowerError::unimplemented(format!(
        "struct `{name}` is not declared in this module and not present in its import closure"
    ))
}

fn field_index(prog: &TypedProgram, base_ty: &Type, field_name: &str) -> Result<usize, LowerError> {
    if matches!(base_ty, Type::Bytes(None)) {
        return match field_name {
            "len" => Ok(1),
            other => Err(LowerError::internal(format!(
                "unknown Bytes field `{other}`"
            ))),
        };
    }
    if matches!(base_ty, Type::String(_)) {
        return match field_name {
            "len" => Ok(0),
            other => Err(LowerError::internal(format!(
                "unknown String field `{other}`"
            ))),
        };
    }
    let Type::Named(sname, targs) = base_ty else {
        return Err(LowerError::internal("field base is not a `Named` type"));
    };
    if sname == "IoCompletion" {
        let fields = crate::mwir::io_completion_fields(targs).map_err(LowerError::internal)?;
        return fields
            .iter()
            .position(|(f, _)| *f == field_name)
            .ok_or_else(|| {
                LowerError::internal(format!("unknown IoCompletion field `{field_name}`"))
            });
    }
    let s = resolve_struct(prog, sname, targs).ok_or_else(|| missing_struct(prog, sname))?;
    s.fields
        .iter()
        .position(|f| f == field_name)
        .ok_or_else(|| LowerError::internal(format!("unknown field `{field_name}`")))
}

fn emit_string_aggregate(
    bytes: &[u8],
    n_expr: &ast::Expr,
    ty: &Type,
    b: &mut FnBuilder,
) -> Result<Temp, LowerError> {
    let n = eval_len_expr(n_expr)?;
    if bytes.len() > n {
        return Err(LowerError::internal(format!(
            "String literal of {} bytes exceeds capacity {n}",
            bytes.len()
        )));
    }
    let mut elems = Vec::with_capacity(1 + n);
    let len_t = b.fresh(Type::Usize);
    b.emit(Inst::ConstInt {
        dst: len_t,
        ty: Type::Usize,
        value: bytes.len() as i128,
    });
    elems.push(len_t);
    for i in 0..n {
        let byte_t = b.fresh(Type::U8);
        let v = i128::from(bytes.get(i).copied().unwrap_or(0));
        b.emit(Inst::ConstInt {
            dst: byte_t,
            ty: Type::U8,
            value: v,
        });
        elems.push(byte_t);
    }
    let dst = b.fresh(ty.clone());
    b.emit(Inst::MakeAggregate { dst, elems });
    Ok(dst)
}

fn interrupt_cell_field_off(
    b: &FnBuilder,
    base_ty: &Type,
    index: usize,
) -> Result<usize, LowerError> {
    let Type::Named(sname, targs) = base_ty else {
        return Err(LowerError::internal(
            "InterruptCell field base is not a `Named` type",
        ));
    };
    let s =
        resolve_struct(b.prog(), sname, targs).ok_or_else(|| missing_struct(b.prog(), sname))?;
    let layout = mwir::LayoutCtx::default();
    let mut off = 0usize;
    for (i, fname) in s.fields.iter().enumerate() {
        if i == index {
            return Ok(off);
        }
        let fty = s
            .field_types
            .get(fname)
            .cloned()
            .ok_or_else(|| LowerError::internal(format!("no type for field `{fname}`")))?;
        off += mwir::size_of(&fty, &layout).map_err(|e| {
            LowerError::unimplemented(&format!(
                "sizing field `{fname}` of `{sname}` for an `InterruptCell` live offset ({e}) is"
            ))
        })?;
    }
    Err(LowerError::internal(format!(
        "InterruptCell field index {index} out of range for `{sname}`"
    )))
}

fn lower_interrupt_cell_intrinsic(
    key: &str,
    receiver: Option<&TypedExpr>,
    args: &[(String, TypedExpr)],
    ret_ty: &Type,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    if key == "InterruptCell.new" {
        let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
            return Err(LowerError::internal(
                "`InterruptCell.new` with no value argument".to_string(),
            ));
        };
        let src = lower_expr(v, b, env)?;
        let dst = b.fresh(ret_ty.clone());
        b.emit(Inst::Copy { dst, src });
        return Ok(dst);
    }
    let Some(recv) = receiver else {
        return Err(LowerError::internal(format!(
            "`{key}` reached lowering with no receiver"
        )));
    };
    let width = match bodies::interrupt_cell_element_type(&recv.ty) {
        Some(Type::U32) => 4,
        Some(Type::U64) => 8,
        _ => {
            return Err(LowerError::internal(format!(
                "unsupported InterruptCell receiver type {:?}",
                recv.ty
            )));
        }
    };
    if let TypedExprKind::Index(base, index_expr) = &recv.kind {
        let Some((static_expr, field_offset, elem_stride, len)) =
            placed_array_field_index(base, b.prog())?
        else {
            return Err(LowerError::unimplemented(
                "an `InterruptCell` indexed outside a placed runtime array is",
            ));
        };
        let base = lower_expr(&static_expr, b, env)?;
        let index = lower_expr(index_expr, b, env)?;
        let mut value = || -> Result<Temp, LowerError> {
            let Some((_, value)) = args.iter().find(|(label, _)| label == "value") else {
                return Err(LowerError::internal(format!(
                    "`{key}` has no value argument"
                )));
            };
            lower_expr(value, b, env)
        };
        return match key {
            "InterruptCell.load_acquire" => {
                let dst = b.fresh(ret_ty.clone());
                b.emit(Inst::PlacedInterruptCellLoadAcquire {
                    dst,
                    base,
                    field_offset,
                    index,
                    len,
                    elem_stride,
                    width,
                });
                Ok(dst)
            }
            "InterruptCell.store_release" => {
                let value = value()?;
                b.emit(Inst::PlacedInterruptCellStoreRelease {
                    base,
                    field_offset,
                    index,
                    len,
                    elem_stride,
                    width,
                    value,
                });
                let dst = b.fresh(Type::Unit);
                b.emit(Inst::ConstUnit { dst });
                Ok(dst)
            }
            "InterruptCell.swap_acquire" => {
                let value = value()?;
                let dst = b.fresh(ret_ty.clone());
                b.emit(Inst::PlacedInterruptCellSwapAcquire {
                    dst,
                    base,
                    field_offset,
                    index,
                    len,
                    elem_stride,
                    width,
                    value,
                });
                Ok(dst)
            }
            "InterruptCell.fetch_or_release" => {
                let value = value()?;
                let dst = b.fresh(ret_ty.clone());
                b.emit(Inst::PlacedInterruptCellFetchOrRelease {
                    dst,
                    base,
                    field_offset,
                    index,
                    len,
                    elem_stride,
                    width,
                    value,
                });
                Ok(dst)
            }
            other => Err(LowerError::internal(format!(
                "unknown InterruptCell intrinsic `{other}`"
            ))),
        };
    }
    let TypedExprKind::Field(base, fname) = &recv.kind else {
        return Err(LowerError::unimplemented(
            "an `InterruptCell` op on a non-field place (only `self.<cell>` is supported) is",
        ));
    };
    let TypedExprKind::Local(base_name) = &base.kind else {
        return Err(LowerError::unimplemented(
            "an `InterruptCell` op through a nested field chain is",
        ));
    };
    if base_name != "self" {
        return Err(LowerError::unimplemented(
            "an `InterruptCell` op on a local cell (only a `@driver` field of `self` is live) is",
        ));
    }
    let base_ty = bodies::unwrap_own(base.ty.clone());
    let idx = field_index(b.prog(), &base_ty, fname)?;
    let field_off = interrupt_cell_field_off(b, &base_ty, idx)?;
    match key {
        "InterruptCell.load_acquire" => {
            let dst = b.fresh(ret_ty.clone());
            b.emit(Inst::InterruptCellLoadAcquire {
                dst,
                field_off,
                width,
            });
            Ok(dst)
        }
        "InterruptCell.store_release" => {
            let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
                return Err(LowerError::internal(
                    "`InterruptCell.store_release` with no value".to_string(),
                ));
            };
            let value = lower_expr(v, b, env)?;
            b.emit(Inst::InterruptCellStoreRelease {
                field_off,
                width,
                value,
            });
            let dst = b.fresh(Type::Unit);
            b.emit(Inst::ConstUnit { dst });
            Ok(dst)
        }
        "InterruptCell.swap_acquire" => {
            let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
                return Err(LowerError::internal(
                    "`InterruptCell.swap_acquire` with no value".to_string(),
                ));
            };
            let value = lower_expr(v, b, env)?;
            let dst = b.fresh(ret_ty.clone());
            b.emit(Inst::InterruptCellSwapAcquire {
                dst,
                field_off,
                width,
                value,
            });
            Ok(dst)
        }
        "InterruptCell.fetch_or_release" => {
            let Some((_, v)) = args.iter().find(|(l, _)| l == "value") else {
                return Err(LowerError::internal(
                    "`InterruptCell.fetch_or_release` with no value".to_string(),
                ));
            };
            let value = lower_expr(v, b, env)?;
            let dst = b.fresh(ret_ty.clone());
            b.emit(Inst::InterruptCellFetchOrRelease {
                dst,
                field_off,
                width,
                value,
            });
            Ok(dst)
        }
        other => Err(LowerError::internal(format!(
            "unknown InterruptCell intrinsic `{other}`"
        ))),
    }
}

fn variant_index(prog: &TypedProgram, enum_name: &str, variant: &str) -> Result<usize, LowerError> {
    match enum_name {
        "Option" => match variant {
            "None" => Ok(value::OPTION_NONE),
            "Some" => Ok(value::OPTION_SOME),
            other => Err(LowerError::internal(format!(
                "unknown Option variant `{other}`"
            ))),
        },
        "Result" => match variant {
            "Ok" => Ok(value::RESULT_OK),
            "Err" => Ok(value::RESULT_ERR),
            other => Err(LowerError::internal(format!(
                "unknown Result variant `{other}`"
            ))),
        },
        "CallError" => crate::sema::bodies::call_error_variant_index(variant)
            .ok_or_else(|| LowerError::internal(format!("unknown CallError variant `{variant}`"))),
        _ => {
            let en = prog
                .enums
                .get(enum_name)
                .or_else(|| prog.imported.enums.get(enum_name).map(|value| value.as_ref()))
                .ok_or_else(|| {
                    LowerError::unimplemented(format!(
                        "constructing/matching a generic enum instantiation's variant (`{enum_name}.{variant}`) is",
                    ))
                })?;
            en.variants
                .iter()
                .position(|v| v == variant)
                .ok_or_else(|| {
                    LowerError::internal(format!("unknown variant `{enum_name}.{variant}`"))
                })
        }
    }
}

fn int_bits(ty: &Type) -> Result<u32, LowerError> {
    match ty {
        Type::U8 | Type::I8 => Ok(8),
        Type::U16 | Type::I16 => Ok(16),
        Type::U32 | Type::I32 => Ok(32),
        Type::U64 | Type::I64 | Type::Usize | Type::Isize => Ok(64),
        other => Err(LowerError::internal(format!(
            "shift on a non-integer type ({other:?})"
        ))),
    }
}

fn lower_array_map_take(
    key: &str,
    receiver: Option<&TypedExpr>,
    args: &[(String, TypedExpr)],
    result_ty: &Type,
    b: &mut FnBuilder,
    env: &mut LEnv,
) -> Result<Temp, LowerError> {
    let recv = receiver.ok_or_else(|| {
        LowerError::internal(format!(
            "`{key}` reached lowering without an array receiver"
        ))
    })?;
    let Type::Array(elem_ty, _) = bodies::unwrap_own(recv.ty.clone()) else {
        return Err(LowerError::internal(format!(
            "`{key}` receiver is not an array"
        )));
    };
    let len = eval_array_len(&recv.ty)?;
    let arr = lower_expr(recv, b, env)?;
    let (_, mapper_expr) = args.first().ok_or_else(|| {
        LowerError::internal(format!("`{key}` reached lowering without a mapper"))
    })?;
    let TypedExprKind::FnRef(mapper_key) = &mapper_expr.kind else {
        return Err(LowerError::unimplemented(
            "`Array.map_take` / `Array.try_map_take` with a non-named-fn mapper is",
        ));
    };
    let mapper_spelling = mapper_key.spelling();
    let is_try = key == "Array.try_map_take";

    if !is_try {
        let Type::Array(out_elem, _) = result_ty else {
            return Err(LowerError::internal(
                "`Array.map_take` result is not an array",
            ));
        };
        let mut outs = Vec::with_capacity(len);
        for i in 0..len {
            let idx_t = b.fresh(Type::Usize);
            b.emit(Inst::ConstInt {
                dst: idx_t,
                ty: Type::Usize,
                value: i as i128,
            });
            let elem_t = b.fresh((*elem_ty).clone());
            b.emit(Inst::IndexGet {
                dst: elem_t,
                base: arr,
                index: idx_t,
                len,
            });
            let out_t = b.fresh((**out_elem).clone());
            b.emit(Inst::Call {
                dst: out_t,
                write_backs: vec![],
                key: mapper_spelling.clone(),
                args: vec![elem_t],
            });
            outs.push(out_t);
        }
        let dst = b.fresh(result_ty.clone());
        b.emit(Inst::MakeAggregate { dst, elems: outs });
        return Ok(dst);
    }

    let Type::Result(ok_arr_ty, err_ty) = result_ty else {
        return Err(LowerError::internal(
            "`Array.try_map_take` result is not a Result",
        ));
    };
    let Type::Array(out_elem, _) = ok_arr_ty.as_ref() else {
        return Err(LowerError::internal(
            "`Array.try_map_take` Ok payload is not an array",
        ));
    };
    let result = b.fresh(result_ty.clone());
    let mut ok_elems = Vec::with_capacity(len);
    let mut err_entry_fixups: Vec<(usize, Temp)> = Vec::new();
    for i in 0..len {
        let idx_t = b.fresh(Type::Usize);
        b.emit(Inst::ConstInt {
            dst: idx_t,
            ty: Type::Usize,
            value: i as i128,
        });
        let elem_t = b.fresh((*elem_ty).clone());
        b.emit(Inst::IndexGet {
            dst: elem_t,
            base: arr,
            index: idx_t,
            len,
        });
        let mapped_ty = Type::Result(Box::new((**out_elem).clone()), Box::new((**err_ty).clone()));
        let mapped = b.fresh(mapped_ty);
        b.emit(Inst::Call {
            dst: mapped,
            write_backs: vec![],
            key: mapper_spelling.clone(),
            args: vec![elem_t],
        });
        let tag_t = b.fresh(Type::U64);
        b.emit(Inst::EnumTag {
            dst: tag_t,
            src: mapped,
        });
        let ok_const = b.fresh(Type::U64);
        b.emit(Inst::ConstInt {
            dst: ok_const,
            ty: Type::U64,
            value: value::RESULT_OK as i128,
        });
        let is_ok = b.fresh(Type::Bool);
        b.emit(Inst::Compare {
            dst: is_ok,
            op: BinOp::Eq,
            ty: Type::U64,
            lhs: tag_t,
            rhs: ok_const,
        });
        let err_fixup = b.emit(Inst::JumpIfFalse {
            cond: is_ok,
            target: usize::MAX,
        });
        err_entry_fixups.push((err_fixup, mapped));
        let payload = b.fresh((**out_elem).clone());
        b.emit(Inst::EnumPayload {
            dst: payload,
            src: mapped,
            index: 0,
        });
        ok_elems.push(payload);
    }
    let arr_out = b.fresh((**ok_arr_ty).clone());
    b.emit(Inst::MakeAggregate {
        dst: arr_out,
        elems: ok_elems,
    });
    b.emit(Inst::MakeEnum {
        dst: result,
        tag: value::RESULT_OK,
        payload: vec![arr_out],
    });
    let success_end_fixup = b.emit(Inst::Jump { target: usize::MAX });

    let mut err_end_fixups = Vec::new();
    for (entry_fixup, mapped) in err_entry_fixups {
        let err_pos = b.here();
        b.patch_jump(entry_fixup, err_pos);
        let err_payload = b.fresh((**err_ty).clone());
        b.emit(Inst::EnumPayload {
            dst: err_payload,
            src: mapped,
            index: 0,
        });
        b.emit(Inst::MakeEnum {
            dst: result,
            tag: value::RESULT_ERR,
            payload: vec![err_payload],
        });
        let j = b.emit(Inst::Jump { target: usize::MAX });
        err_end_fixups.push(j);
    }
    let end_pos = b.here();
    b.patch_jump(success_end_fixup, end_pos);
    for j in err_end_fixups {
        b.patch_jump(j, end_pos);
    }
    Ok(result)
}

fn literal_array_index_elide(
    _idx_expr: &TypedExpr,
    _len: usize,
) -> Result<Option<usize>, LowerError> {
    // Bounds elimination is proof-carrying in optimized MWIR.  Lowering must
    // not bypass that representation by turning a literal index directly into
    // a field operation; the range pass handles literals through the same
    // `Index*Proven` variants as every other proof.
    Ok(None)
}

fn eval_array_len(ty: &Type) -> Result<usize, LowerError> {
    match ty {
        Type::Array(_, len_expr) => eval_len_expr(len_expr),
        Type::Own(_, inner) => eval_array_len(inner),
        _ => Err(LowerError::unimplemented(
            "indexing a non-array (e.g. `Bytes`) value is",
        )),
    }
}

fn eval_len_expr(e: &ast::Expr) -> Result<usize, LowerError> {
    if let Some(n) = bodies::literal_array_len(e) {
        return usize::try_from(n).map_err(|_| LowerError::internal("array length out of range"));
    }
    Err(LowerError::unimplemented(
        "an array length expression that is not a literal is",
    ))
}

fn eval_array_len_with_prog(ty: &Type, prog: &TypedProgram) -> Result<usize, LowerError> {
    match ty {
        Type::Array(_, len_expr) => eval_len_expr_with_prog(len_expr, prog),
        Type::Own(_, inner) => eval_array_len_with_prog(inner, prog),
        _ => Err(LowerError::unimplemented(
            "indexing a non-array (e.g. `Bytes`) value is",
        )),
    }
}

fn eval_len_expr_with_prog(e: &ast::Expr, prog: &TypedProgram) -> Result<usize, LowerError> {
    if let Some(n) = bodies::literal_array_len(e) {
        return usize::try_from(n).map_err(|_| LowerError::internal("array length out of range"));
    }
    if let ast::Expr::Name(_, name) = e {
        let v = crate::eval::interp::eval_const(prog, name).map_err(|err| {
            LowerError::internal(format!(
                "const `{name}` failed to evaluate during array-length lowering: {}",
                err.message
            ))
        })?;
        let n = value::as_i128(&v).ok_or_else(|| {
            LowerError::internal(format!("array length const `{name}` is not an integer"))
        })?;
        return usize::try_from(n).map_err(|_| LowerError::internal("array length out of range"));
    }
    Err(LowerError::unimplemented(
        "an array length expression that is not a literal is",
    ))
}

#[cfg(test)]
mod builder_tests {
    use super::*;

    fn new_lowerer(prog: &TypedProgram) -> Lowerer<'_> {
        Lowerer {
            prog,
            blk_capacity_sectors: prog.blk_capacity_sectors,
            rodata: Vec::new(),
            rodata_index: BTreeMap::new(),
        }
    }

    #[test]
    fn patch_jump_resolves_a_forward_jump_to_the_patched_index() {
        let prog = TypedProgram::default();
        let mut lw = new_lowerer(&prog);
        let mut b = FnBuilder {
            lw: &mut lw,
            temp_types: Vec::new(),
            body: Vec::new(),
            ret: Type::Unit,
            owner_struct: None,
        };
        let cond = b.fresh(Type::Bool);
        b.emit(Inst::ConstBool {
            dst: cond,
            value: true,
        });
        let fixup = b.emit(Inst::JumpIfFalse {
            cond,
            target: usize::MAX,
        });
        b.emit(Inst::ConstUnit { dst: cond });
        b.emit(Inst::ConstUnit { dst: cond });
        let target = b.here();
        b.patch_jump(fixup, target);
        match &b.body[fixup] {
            Inst::JumpIfFalse { target: t, .. } => assert_eq!(*t, target),
            other => panic!("expected JumpIfFalse, got {other:?}"),
        }
        assert_eq!(target, 4);
    }

    #[test]
    fn rodata_interning_dedupes_identical_bytes() {
        let prog = TypedProgram::default();
        let mut lw = new_lowerer(&prog);
        let mut b = FnBuilder {
            lw: &mut lw,
            temp_types: Vec::new(),
            body: Vec::new(),
            ret: Type::Unit,
            owner_struct: None,
        };
        let a = b.intern(b"hello".to_vec());
        let c = b.intern(b"world".to_vec());
        let d = b.intern(b"hello".to_vec());
        assert_eq!(a, d);
        assert_ne!(a, c);
        assert_eq!(b.lw.rodata.len(), 2);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::opts::{CompileMode, OptId, apply_mode, apply_opts};
    use crate::sema;
    use crate::syntax::ast::Span;
    use crate::syntax::{lexer, parser};

    fn typed_program(src: &str) -> TypedProgram {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        sema::check_typed(&module, "<test>").expect("test source must check")
    }

    #[test]
    fn pixels_raster_mwir_audit_requires_one_live_packet_loop_and_scalar_edges() {
        let mut fns = BTreeMap::new();
        let body = vec![
            Inst::ConstUnit { dst: Temp(0) },
            Inst::Jump { target: 0 },
            Inst::I32x4Add {
                dst: Temp(0),
                lhs: Temp(0),
                rhs: Temp(0),
            },
            Inst::I32x4Add {
                dst: Temp(0),
                lhs: Temp(0),
                rhs: Temp(0),
            },
            Inst::Jump { target: 2 },
            Inst::ConstUnit { dst: Temp(0) },
            Inst::Jump { target: 5 },
        ];
        fns.insert(
            "__wrela_pixels_p8_raster_regular".to_string(),
            MwirFn {
                receiver: None,
                params: Vec::new(),
                ret: Type::Unit,
                temp_types: vec![Type::Named("I32x4".to_string(), Vec::new())],
                body,
            },
        );
        validate_pixels_i32x4_raster_mwir(&fns).expect("canonical packet loop");

        fns.get_mut("__wrela_pixels_p8_raster_regular")
            .expect("fixture raster")
            .body
            .remove(2);
        let error = validate_pixels_i32x4_raster_mwir(&fns)
            .expect_err("scalarized packet recurrence must fail closed");
        assert!(error.message.contains("expected 2"));
    }

    fn packet_fn(param_types: Vec<Type>, ret: Type) -> MwirFn {
        MwirFn {
            receiver: None,
            params: param_types
                .iter()
                .enumerate()
                .map(|(index, _)| (Temp(index), AccessMode::Read))
                .collect(),
            ret,
            temp_types: param_types,
            body: Vec::new(),
        }
    }

    fn mark_renderer_fn(program: &mut TypedProgram, name: &str) {
        program
            .fn_decl_modules
            .insert(name.to_string(), "core.render_raster".to_string());
        program
            .fn_decl_names
            .insert(name.to_string(), name.to_string());
    }

    #[test]
    fn every_sealed_packet_intrinsic_maps_to_its_exact_mwir_operation() {
        use mwir::{PacketBinaryOp as Binary, PacketKind as Kind, PacketSelectOp as Select};

        #[derive(Clone, Copy)]
        enum Expected {
            From(Kind),
            Splat(Kind),
            Binary(Kind, Binary),
            Shift,
            Select(Kind, Select),
            Fma,
            Convert(Kind, Kind),
        }

        let i32x4 = Type::Named("I32x4".to_string(), Vec::new());
        let f32x4 = Type::Named("F32x4".to_string(), Vec::new());
        let four = || Box::new(ast::Expr::Int(Span::default(), "4".to_string()));
        let i32_lanes = Type::Array(Box::new(Type::I32), four());
        let f32_lanes = Type::Array(Box::new(Type::F32), four());
        let cases = vec![
            (
                "I32x4.from_lanes",
                vec![i32_lanes],
                i32x4.clone(),
                Expected::From(Kind::I32x4),
            ),
            (
                "F32x4.from_lanes",
                vec![f32_lanes],
                f32x4.clone(),
                Expected::From(Kind::F32x4),
            ),
            (
                "pixels_i32x4_backend_splat",
                vec![Type::I32],
                i32x4.clone(),
                Expected::Splat(Kind::I32x4),
            ),
            (
                "pixels_i32x4_backend_add",
                vec![i32x4.clone(); 2],
                i32x4.clone(),
                Expected::Binary(Kind::I32x4, Binary::Add),
            ),
            (
                "pixels_i32x4_backend_sub",
                vec![i32x4.clone(); 2],
                i32x4.clone(),
                Expected::Binary(Kind::I32x4, Binary::Sub),
            ),
            (
                "pixels_i32x4_backend_shr_arith_imm",
                vec![i32x4.clone(), Type::U8],
                i32x4.clone(),
                Expected::Shift,
            ),
            (
                "pixels_i32x4_backend_and",
                vec![i32x4.clone(); 2],
                i32x4.clone(),
                Expected::Binary(Kind::I32x4, Binary::And),
            ),
            (
                "pixels_i32x4_backend_or",
                vec![i32x4.clone(); 2],
                i32x4.clone(),
                Expected::Binary(Kind::I32x4, Binary::Or),
            ),
            (
                "pixels_i32x4_backend_select_gt",
                vec![i32x4.clone(); 4],
                i32x4.clone(),
                Expected::Select(Kind::I32x4, Select::Gt),
            ),
            (
                "pixels_i32x4_backend_to_f32x4",
                vec![i32x4.clone()],
                f32x4.clone(),
                Expected::Convert(Kind::I32x4, Kind::F32x4),
            ),
            (
                "pixels_f32x4_backend_splat",
                vec![Type::F32],
                f32x4.clone(),
                Expected::Splat(Kind::F32x4),
            ),
            (
                "pixels_f32x4_backend_add",
                vec![f32x4.clone(); 2],
                f32x4.clone(),
                Expected::Binary(Kind::F32x4, Binary::Add),
            ),
            (
                "pixels_f32x4_backend_sub",
                vec![f32x4.clone(); 2],
                f32x4.clone(),
                Expected::Binary(Kind::F32x4, Binary::Sub),
            ),
            (
                "pixels_f32x4_backend_mul",
                vec![f32x4.clone(); 2],
                f32x4.clone(),
                Expected::Binary(Kind::F32x4, Binary::Mul),
            ),
            (
                "pixels_f32x4_backend_min",
                vec![f32x4.clone(); 2],
                f32x4.clone(),
                Expected::Binary(Kind::F32x4, Binary::Min),
            ),
            (
                "pixels_f32x4_backend_max",
                vec![f32x4.clone(); 2],
                f32x4.clone(),
                Expected::Binary(Kind::F32x4, Binary::Max),
            ),
            (
                "pixels_f32x4_backend_select_ge",
                vec![f32x4.clone(); 4],
                f32x4.clone(),
                Expected::Select(Kind::F32x4, Select::Ge),
            ),
            (
                "pixels_f32x4_backend_select_gt",
                vec![f32x4.clone(); 4],
                f32x4.clone(),
                Expected::Select(Kind::F32x4, Select::Gt),
            ),
            (
                "pixels_f32x4_backend_fma",
                vec![f32x4.clone(); 3],
                f32x4.clone(),
                Expected::Fma,
            ),
            (
                "pixels_f32x4_backend_to_i32x4",
                vec![f32x4.clone()],
                i32x4.clone(),
                Expected::Convert(Kind::F32x4, Kind::I32x4),
            ),
        ];

        let mut program = TypedProgram::default();
        for packet_type in ["I32x4", "F32x4"] {
            program
                .type_decl_modules
                .insert(packet_type.to_string(), "core.render_raster".to_string());
            program
                .type_decl_names
                .insert(packet_type.to_string(), packet_type.to_string());
        }
        let mut fns = BTreeMap::new();
        for (index, (name, params, ret, _)) in cases.iter().enumerate() {
            mark_renderer_fn(&mut program, name);
            fns.insert((*name).to_string(), packet_fn(params.clone(), ret.clone()));

            let mut temp_types = params.clone();
            temp_types.push(ret.clone());
            let dst = Temp(params.len());
            let mut body = Vec::new();
            if *name == "pixels_i32x4_backend_shr_arith_imm" {
                body.push(Inst::ConstInt {
                    dst: Temp(1),
                    ty: Type::U8,
                    value: 7,
                });
            }
            body.push(Inst::Call {
                dst,
                key: (*name).to_string(),
                args: (0..params.len()).map(Temp).collect(),
                write_backs: Vec::new(),
            });
            let consumer = format!("consumer_{index}");
            mark_renderer_fn(&mut program, &consumer);
            fns.insert(
                consumer,
                MwirFn {
                    receiver: None,
                    params: params
                        .iter()
                        .enumerate()
                        .map(|(temp, _)| (Temp(temp), AccessMode::Read))
                        .collect(),
                    ret: ret.clone(),
                    temp_types,
                    body,
                },
            );
        }

        install_pixels_packet_intrinsics(&program, &mut fns)
            .expect("the complete canonical intrinsic table must lower");
        for (index, (name, _, _, expected)) in cases.iter().enumerate() {
            assert!(!fns.contains_key(*name), "intrinsic body `{name}` survived");
            let inst = fns[&format!("consumer_{index}")]
                .body
                .last()
                .expect("consumer operation");
            let matches = match (expected, inst) {
                (Expected::From(want), Inst::PacketFromLanes { kind, .. }) => want == kind,
                (Expected::Splat(want), Inst::PacketSplat { kind, .. }) => want == kind,
                (Expected::Binary(want_kind, want_op), Inst::PacketBinary { kind, op, .. }) => {
                    want_kind == kind && want_op == op
                }
                (Expected::Shift, Inst::PacketShiftRightArithmetic { immediate: 7, .. }) => true,
                (Expected::Select(want_kind, want_op), Inst::PacketSelect { kind, op, .. }) => {
                    want_kind == kind && want_op == op
                }
                (Expected::Fma, Inst::PacketFma { .. }) => true,
                (Expected::Convert(want_from, want_to), Inst::PacketConvert { from, to, .. }) => {
                    want_from == from && want_to == to
                }
                _ => false,
            };
            assert!(matches, "`{name}` lowered to {inst:?}");
        }
    }

    #[test]
    fn packet_shift_lowering_requires_a_declared_constant_in_one_through_31() {
        const SHIFT: &str = "pixels_i32x4_backend_shr_arith_imm";
        let packet = Type::Named("I32x4".to_string(), Vec::new());
        for (immediate, accepted) in [(0_i128, false), (1, true), (31, true), (32, false)] {
            let mut program = TypedProgram::default();
            program
                .fn_decl_modules
                .insert(SHIFT.to_string(), "core.render_raster".to_string());
            program
                .fn_decl_names
                .insert(SHIFT.to_string(), SHIFT.to_string());
            mark_renderer_fn(&mut program, "consumer");
            program
                .type_decl_modules
                .insert("I32x4".to_string(), "core.render_raster".to_string());
            program
                .type_decl_names
                .insert("I32x4".to_string(), "I32x4".to_string());
            let mut fns = BTreeMap::new();
            fns.insert(
                SHIFT.to_string(),
                packet_fn(vec![packet.clone(), Type::U8], packet.clone()),
            );
            fns.insert(
                "consumer".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![(Temp(0), AccessMode::Read)],
                    ret: packet.clone(),
                    temp_types: vec![packet.clone(), Type::U8, packet.clone()],
                    body: vec![
                        Inst::ConstInt {
                            dst: Temp(1),
                            ty: Type::U8,
                            value: immediate,
                        },
                        Inst::Call {
                            dst: Temp(2),
                            key: SHIFT.to_string(),
                            args: vec![Temp(0), Temp(1)],
                            write_backs: Vec::new(),
                        },
                    ],
                },
            );
            let result = install_pixels_packet_intrinsics(&program, &mut fns);
            if accepted {
                result.expect("sealed shift immediate must lower");
                assert!(matches!(
                    fns["consumer"].body[1],
                    Inst::PacketShiftRightArithmetic {
                        immediate: value,
                        ..
                    } if i128::from(value) == immediate
                ));
                assert!(!fns.contains_key(SHIFT));
            } else {
                let error = result.expect_err("out-of-range shift must fail closed");
                assert!(error.message.contains("must be in 1..=31"));
            }
        }
    }

    #[test]
    fn packet_call_without_its_canonical_declaration_fails_closed() {
        const ADD: &str = "pixels_f32x4_backend_add";
        let packet = Type::Named("F32x4".to_string(), Vec::new());
        let mut program = TypedProgram::default();
        program
            .fn_decl_modules
            .insert(ADD.to_string(), "core.render_raster".to_string());
        program
            .fn_decl_names
            .insert(ADD.to_string(), ADD.to_string());
        mark_renderer_fn(&mut program, "consumer");
        program
            .type_decl_modules
            .insert("F32x4".to_string(), "core.render_raster".to_string());
        program
            .type_decl_names
            .insert("F32x4".to_string(), "F32x4".to_string());
        let mut fns = BTreeMap::from([(
            "consumer".to_string(),
            MwirFn {
                receiver: None,
                params: vec![(Temp(0), AccessMode::Read), (Temp(1), AccessMode::Read)],
                ret: packet.clone(),
                temp_types: vec![packet, Type::Named("F32x4".to_string(), Vec::new())],
                body: vec![Inst::Call {
                    dst: Temp(0),
                    key: ADD.to_string(),
                    args: vec![Temp(0), Temp(1)],
                    write_backs: Vec::new(),
                }],
            },
        )]);
        let error = install_pixels_packet_intrinsics(&program, &mut fns)
            .expect_err("missing intrinsic declaration must fail closed");
        assert!(error.message.contains("without its canonical declaration"));
    }

    #[test]
    fn canonical_packet_call_from_non_renderer_function_fails_closed() {
        const ADD: &str = "pixels_f32x4_backend_add";
        let packet = Type::Named("F32x4".to_string(), Vec::new());
        let mut program = TypedProgram::default();
        mark_renderer_fn(&mut program, ADD);
        program
            .type_decl_modules
            .insert("F32x4".to_string(), "core.render_raster".to_string());
        program
            .type_decl_names
            .insert("F32x4".to_string(), "F32x4".to_string());
        program
            .fn_decl_modules
            .insert("consumer".to_string(), "examples.consumer".to_string());
        program
            .fn_decl_names
            .insert("consumer".to_string(), "consumer".to_string());
        let mut fns = BTreeMap::from([
            (
                ADD.to_string(),
                packet_fn(vec![packet.clone(), packet.clone()], packet.clone()),
            ),
            (
                "consumer".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![(Temp(0), AccessMode::Read), (Temp(1), AccessMode::Read)],
                    ret: packet.clone(),
                    temp_types: vec![packet.clone(), packet.clone(), packet],
                    body: vec![Inst::Call {
                        dst: Temp(2),
                        key: ADD.to_string(),
                        args: vec![Temp(0), Temp(1)],
                        write_backs: Vec::new(),
                    }],
                },
            ),
        ]);

        let error = install_pixels_packet_intrinsics(&program, &mut fns)
            .expect_err("ordinary sync code must not acquire packet instructions");
        assert_eq!(
            error.message,
            "internal error: Pixels packet intrinsic pixels_f32x4_backend_add is called from non-renderer function consumer"
        );
    }

    #[test]
    fn user_packet_spelling_is_not_rewritten_or_removed() {
        const ADD: &str = "pixels_f32x4_backend_add";
        let packet = Type::Named("F32x4".to_string(), Vec::new());
        let mut program = TypedProgram::default();
        program
            .fn_decl_modules
            .insert(ADD.to_string(), "examples.packet_spoof".to_string());
        program
            .fn_decl_names
            .insert(ADD.to_string(), ADD.to_string());
        program
            .type_decl_modules
            .insert("F32x4".to_string(), "examples.packet_spoof".to_string());
        program
            .type_decl_names
            .insert("F32x4".to_string(), "F32x4".to_string());
        let ordinary = packet_fn(vec![packet.clone(), packet.clone()], packet.clone());
        let mut fns = BTreeMap::from([
            (ADD.to_string(), ordinary.clone()),
            (
                "consumer".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![(Temp(0), AccessMode::Read), (Temp(1), AccessMode::Read)],
                    ret: packet.clone(),
                    temp_types: vec![packet.clone(), packet.clone(), packet],
                    body: vec![Inst::Call {
                        dst: Temp(2),
                        key: ADD.to_string(),
                        args: vec![Temp(0), Temp(1)],
                        write_backs: Vec::new(),
                    }],
                },
            ),
        ]);

        install_pixels_packet_intrinsics(&program, &mut fns)
            .expect("ordinary user functions are outside the sealed intrinsic set");
        assert_eq!(fns.get(ADD), Some(&ordinary));
        assert!(matches!(fns["consumer"].body[0], Inst::Call { .. }));
    }

    #[test]
    fn pixels_submission_bridge_writes_the_sealed_dynamic_doorbell() {
        let mut program = TypedProgram::default();
        program.fn_decl_modules.insert(
            PIXELS_DISPLAY_RAW_FN.to_string(),
            "core.__image_pixels".to_string(),
        );
        program.fn_decl_names.insert(
            PIXELS_DISPLAY_RAW_FN.to_string(),
            PIXELS_DISPLAY_RAW_FN.to_string(),
        );
        program
            .fn_decl_modules
            .insert(PIXELS_DISPLAY_DRIVER_FN.to_string(), "display".to_string());
        program.fn_decl_names.insert(
            PIXELS_DISPLAY_DRIVER_FN.to_string(),
            PIXELS_DISPLAY_DRIVER_FN.to_string(),
        );
        let canonical_fn = MwirFn {
            receiver: None,
            params: vec![(Temp(0), AccessMode::Read), (Temp(1), AccessMode::Read)],
            ret: Type::Unit,
            temp_types: vec![Type::U64, Type::U64],
            body: Vec::new(),
        };
        let mut functions = BTreeMap::from([
            (PIXELS_DISPLAY_RAW_FN.to_string(), canonical_fn.clone()),
            (PIXELS_DISPLAY_DRIVER_FN.to_string(), canonical_fn),
        ]);
        install_pixels_display_driver_bridge(&program, &mut functions).unwrap();
        validate_pixels_display_driver_bridge_target(&MwirProgram {
            fns: functions.clone(),
            rodata: Vec::new(),
            direct_fp_fns: BTreeSet::new(),
        })
        .unwrap();
        let bridge = &functions[PIXELS_DISPLAY_RAW_FN];
        assert!(matches!(
            bridge.body.as_slice(),
            [
                Inst::Call { key, args, write_backs, .. },
                Inst::Return { .. },
            ] if key == PIXELS_DISPLAY_DRIVER_FN
                && args == &[Temp(0), Temp(1)]
                && write_backs.is_empty()
        ));
        let driver = &functions[PIXELS_DISPLAY_DRIVER_FN];
        assert!(matches!(
            driver.body.as_slice(),
            [
                Inst::Dmb { option: before },
                Inst::MmioWrite { base: Temp(0), offset: 0, ty: Type::U64, value: Temp(1) },
                Inst::Dmb { option: after },
                Inst::ConstUnit { .. },
                Inst::Return { .. },
            ] if before == "ishst" && after == "ishld"
        ));
    }

    #[test]
    fn user_display_bridge_spelling_is_not_rewritten() {
        let mut program = TypedProgram::default();
        for name in [PIXELS_DISPLAY_RAW_FN, PIXELS_DISPLAY_DRIVER_FN] {
            program
                .fn_decl_modules
                .insert(name.to_string(), "examples.display_spoof".to_string());
            program
                .fn_decl_names
                .insert(name.to_string(), name.to_string());
        }
        let ordinary = MwirFn {
            receiver: None,
            params: vec![(Temp(0), AccessMode::Read), (Temp(1), AccessMode::Read)],
            ret: Type::Unit,
            temp_types: vec![Type::U64, Type::U64],
            body: vec![Inst::ConstUnit { dst: Temp(0) }],
        };
        let mut functions = BTreeMap::from([
            (PIXELS_DISPLAY_RAW_FN.to_string(), ordinary.clone()),
            (PIXELS_DISPLAY_DRIVER_FN.to_string(), ordinary.clone()),
        ]);

        install_pixels_display_driver_bridge(&program, &mut functions).unwrap();
        assert_eq!(functions[PIXELS_DISPLAY_RAW_FN], ordinary);
        assert_eq!(functions[PIXELS_DISPLAY_DRIVER_FN], ordinary);
    }

    #[test]
    fn quiesce_before_halt_is_a_bounded_wait() {
        let (runtime_key, runtime_loaded) = match crate::loader::load_runtime_module() {
            Ok(v) => v,
            Err(_) => panic!("runtime.wr must load"),
        };
        let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let gen_module = crate::rtconfig::parse_generated(&crate::rtconfig::stub_text())
            .expect("stub must parse");
        let mut modules = BTreeMap::new();
        modules.insert(runtime_key.clone(), runtime_loaded.module);
        modules.insert(gen_key.clone(), gen_module);
        let mut paths = BTreeMap::new();
        paths.insert(
            runtime_key.clone(),
            runtime_loaded.file.display().to_string(),
        );
        paths.insert(
            gen_key.clone(),
            crate::rtconfig::GENERATED_INPUT_PATH.to_string(),
        );
        let internal_sources = BTreeSet::from([gen_key]);
        let programs =
            sema::check_program_typed_with_internal_sources(&modules, &paths, &internal_sources)
                .expect("check");
        let typed = programs
            .get(&runtime_key)
            .expect("core.runtime must be checked");
        let mut only = BTreeSet::new();
        only.insert("__wrela_quiesce_before_halt".to_string());
        let opts = LowerOpts {
            emit_comptime_tests: false,
            only: Some(only),
        };
        let mwir = lower_program_with(typed, &opts).expect("core.runtime must lower");
        let f = mwir
            .fns
            .get("__wrela_quiesce_before_halt")
            .expect("quiesce fn must lower");
        let bounded = f.body.iter().any(
            |i| matches!(i, Inst::AssertFail { message: Some(m) } if m == "loop budget exceeded"),
        );
        assert!(
            bounded,
            "the quiesce wait must be `@budget`-bounded; body:\n{:?}",
            f.body
        );
        let bound = f
            .body
            .iter()
            .filter_map(|i| match i {
                Inst::ConstInt {
                    ty: Type::U64,
                    value,
                    ..
                } if *value > 1 => Some(*value),
                _ => None,
            })
            .max()
            .expect("a budget bound literal must be present");
        assert_eq!(
            bound, 262_144,
            "QUIESCE_POLL_BOUND changed; update this oracle deliberately"
        );
    }

    #[test]
    fn runtime_force_root_seeds_probe_when_runtime_loaded() {
        let src = "\
module examples.force_root_probe

@test(runtime)
pub fn t():
    return
";
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let (runtime_key, runtime_loaded) = match crate::loader::load_runtime_module() {
            Ok(v) => v,
            Err(_) => panic!("runtime.wr must load"),
        };
        let root_key = module.path.clone();
        let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let gen_module = crate::rtconfig::parse_generated(&crate::rtconfig::stub_text())
            .expect("stub must parse");
        let mut modules = BTreeMap::new();
        modules.insert(root_key.clone(), module);
        modules.insert(runtime_key.clone(), runtime_loaded.module);
        modules.insert(gen_key.clone(), gen_module);
        let mut paths = BTreeMap::new();
        paths.insert(root_key.clone(), "<test>".to_string());
        paths.insert(
            runtime_key.clone(),
            runtime_loaded.file.display().to_string(),
        );
        paths.insert(
            gen_key.clone(),
            crate::rtconfig::GENERATED_INPUT_PATH.to_string(),
        );
        let internal_sources = BTreeSet::from([gen_key]);
        let programs =
            sema::check_program_typed_with_internal_sources(&modules, &paths, &internal_sources)
                .expect("check");
        let by_dot: BTreeMap<String, TypedProgram> = programs
            .into_iter()
            .map(|(k, p)| (k.join("."), p))
            .collect();
        let reachable = guest_reachable_keys_closure(&by_dot, &LowerOpts::default());
        assert!(
            reachable.contains("__wrela_runtime_probe"),
            "force-root must seed the probe: {reachable:?}"
        );
        assert!(
            reachable.contains("__wrela_line_begin"),
            "force-root must seed line_begin: {reachable:?}"
        );
        assert!(
            reachable.contains("__wrela_line_commit"),
            "force-root must seed line_commit: {reachable:?}"
        );
        assert!(
            reachable.contains("__wrela_fmt_dec"),
            "force-root must seed fmt_dec: {reachable:?}"
        );
        assert!(
            reachable.contains("__wrela_abort"),
            "force-root must seed abort: {reachable:?}"
        );
        assert!(
            reachable.contains("__wrela_abort_val"),
            "force-root must seed abort_val: {reachable:?}"
        );
        let lower_opts = LowerOpts {
            emit_comptime_tests: false,
            only: Some(reachable),
        };
        let mut saw_probe = false;
        for typed in by_dot.values() {
            let mwir = lower_program_with(typed, &lower_opts).expect("lower");
            if mwir.fns.contains_key("__wrela_runtime_probe") {
                saw_probe = true;
            }
        }
        assert!(saw_probe, "probe must lower from core.runtime");
    }

    #[test]
    fn lowers_a_plain_arithmetic_fn() {
        let program = typed_program(
            "module examples.lower_arith

pub fn add_one(x: u64) -> u64:
    return x + 1
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        let f = mwir.fns.get("add_one").expect("fn lowered");
        assert!(
            f.body
                .iter()
                .any(|i| matches!(i, Inst::ArithChecked { .. }))
        );
        assert!(matches!(f.body.last(), Some(Inst::Return { .. })));
    }

    #[test]
    fn literal_fixed_array_index_keeps_its_bounds_check_in_both_modes() {
        let get = typed_program(
            "module examples.lower_array_index_checked

pub fn at_zero(a: [u64; 4]) -> u64:
    return a[0]
",
        );
        let set = typed_program(
            "module examples.lower_array_index_set_checked

pub fn write_zero(mut a: [u64; 4], v: u64):
    a[0] = v
",
        );

        for mode in [CompileMode::Release, CompileMode::Dev] {
            apply_mode(mode);

            let mwir = lower_program(&get).expect("must lower cleanly");
            let f = mwir.fns.get("at_zero").expect("fn lowered");
            assert!(
                f.body
                    .iter()
                    .any(|i| matches!(i, Inst::IndexGet { len: 4, .. })),
                "{mode:?}: literal read must keep IndexGet len=4, got {:?}",
                f.body
            );
            assert!(
                f.body.iter().all(|i| !matches!(i, Inst::Project { .. })),
                "{mode:?}: the elision to Project is deleted, got {:?}",
                f.body
            );

            let mwir = lower_program(&set).expect("must lower cleanly");
            let f = mwir.fns.get("write_zero").expect("fn lowered");
            assert!(
                f.body
                    .iter()
                    .any(|i| matches!(i, Inst::IndexSet { len: 4, .. })),
                "{mode:?}: literal write must keep IndexSet len=4, got {:?}",
                f.body
            );
            assert!(
                f.body.iter().all(|i| !matches!(i, Inst::SetField { .. })),
                "{mode:?}: no product mode may elide to SetField, got {:?}",
                f.body
            );
        }
        apply_mode(CompileMode::Release);
    }

    #[test]
    fn parked_bounds_elide_uses_the_shared_proof_marker_when_named() {
        let get = typed_program(
            "module examples.lower_bounds_elide_parked

pub fn at_zero(a: [u64; 4]) -> u64:
    return a[0]
",
        );
        let set = typed_program(
            "module examples.lower_bounds_elide_parked_set

pub fn write_zero(mut a: [u64; 4], v: u64):
    a[0] = v
",
        );

        apply_opts(&[OptId::BoundsElide]);

        let mwir = lower_program(&get).expect("must lower cleanly");
        let f = mwir.fns.get("at_zero").expect("fn lowered");
        assert!(f.body.iter().any(|i| matches!(i, Inst::IndexGet { .. })));
        let proven = crate::range::apply_program_proofs(&mwir).expect("literal proof");
        assert!(
            proven.fns["at_zero"]
                .body
                .iter()
                .any(|i| matches!(i, Inst::IndexGetProven { len: 4, .. }))
        );

        let mwir = lower_program(&set).expect("must lower cleanly");
        let f = mwir.fns.get("write_zero").expect("fn lowered");
        assert!(f.body.iter().any(|i| matches!(i, Inst::IndexSet { .. })));
        let proven = crate::range::apply_program_proofs(&mwir).expect("literal proof");
        assert!(
            proven.fns["write_zero"]
                .body
                .iter()
                .any(|i| matches!(i, Inst::IndexSetProven { len: 4, .. }))
        );

        let oob = typed_program(
            "module examples.lower_bounds_elide_parked_oob

pub fn at_nine(a: [u64; 4]) -> u64:
    return a[9]
",
        );
        let mwir = lower_program(&oob).expect("must lower cleanly");
        let f = mwir.fns.get("at_nine").expect("fn lowered");
        assert!(
            f.body
                .iter()
                .any(|i| matches!(i, Inst::IndexGet { len: 4, .. })),
            "parked opt on: an out-of-range literal keeps IndexGet, got {:?}",
            f.body
        );

        apply_mode(CompileMode::Release);
    }

    #[test]
    fn lowers_now_and_entropy_intrinsics() {
        let program = typed_program(
            "module examples.lower_sealed_runtime

pub fn tick() -> Instant:
    return now()

pub fn bits() -> Bytes[8]:
    return entropy[8]()
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        let tick = mwir.fns.get("tick").expect("tick lowered");
        assert!(
            tick.body.iter().any(|i| matches!(i, Inst::Now { .. })),
            "tick body must emit Inst::Now: {:?}",
            tick.body
        );
        let bits = mwir.fns.get("bits").expect("bits lowered");
        assert!(
            bits.body
                .iter()
                .any(|i| matches!(i, Inst::Entropy { n: 8, .. })),
            "bits body must emit Inst::Entropy n=8: {:?}",
            bits.body
        );
    }

    #[test]
    fn lowers_a_struct_method_and_init() {
        let program = typed_program(
            "module examples.lower_struct

pub struct Counter:
    value: u64

    init(mut self, start: u64):
        self.value = start

    pub fn bump(mut self, by: u64):
        self.value = self.value + by

pub fn use_counter() -> u64:
    c = Counter(start=10)
    c.bump(5)
    return c.value
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        assert!(mwir.fns.contains_key("Counter.init"));
        assert!(mwir.fns.contains_key("Counter.bump"));
        let use_fn = mwir.fns.get("use_counter").expect("fn lowered");
        assert!(use_fn.body.iter().any(|i| matches!(
            i,
            Inst::Call { key, .. } if key == "Counter.init"
        )));
        assert!(use_fn.body.iter().any(|i| matches!(
            i,
            Inst::Call { key, write_backs, .. } if key == "Counter.bump" && !write_backs.is_empty()
        )));
    }

    #[test]
    fn lowers_an_enum_match() {
        let program = typed_program(
            "module examples.lower_match

pub enum Shape:
    Circle(u64)
    Square(u64)

pub fn area(s: Shape) -> u64:
    match s:
        case .Circle(r):
            return r * r
        case .Square(side):
            return side * side
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        let f = mwir.fns.get("area").expect("fn lowered");
        assert!(f.body.iter().any(|i| matches!(i, Inst::EnumTag { .. })));
        assert!(f.body.iter().any(|i| matches!(i, Inst::EnumPayload { .. })));
        assert!(f.body.iter().any(|i| matches!(i, Inst::AssertFail { .. })));
    }

    #[test]
    fn lowers_a_loop_with_an_accumulator() {
        let program = typed_program(
            "module examples.lower_loop

pub fn sum_to(n: u64) -> u64:
    total: u64 = 0
    i: u64 = 0
    @budget(bound=1000000)
    while i < n:
        total = total + i
        i = i + 1
    return total
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        let f = mwir.fns.get("sum_to").expect("fn lowered");
        assert!(f.body.iter().any(|i| matches!(i, Inst::Jump { .. })));
        assert!(f.body.iter().any(|i| matches!(i, Inst::JumpIfFalse { .. })));
        assert!(
            f.body.iter().any(|i| matches!(
                i,
                Inst::AssertFail {
                    message: Some(m)
                } if m == "loop budget exceeded"
            )),
            "sync loop must lower a trip-counter AssertFail"
        );
    }

    #[test]
    fn a_generic_fn_instantiation_lowers_under_its_own_key() {
        let program = typed_program(
            "module examples.lower_generic

pub fn identity[T](x: T) -> T:
    return x

pub fn use_identity() -> i64:
    return identity(42)
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        assert!(
            mwir.fns.keys().any(|k| k.starts_with("fn:identity")),
            "expected an instantiated `identity` key, got {:?}",
            mwir.fns.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn take_lowers_transparently_with_no_extra_instruction() {
        let program = typed_program(
            "module examples.lower_take

pub fn consume(take x: u64) -> u64:
    return x

pub fn use_it() -> u64:
    a: u64 = 7
    return consume(take a)
",
        );
        let mwir = lower_program(&program).expect("must lower cleanly");
        assert!(mwir.fns.contains_key("consume"));
        assert!(mwir.fns.contains_key("use_it"));
    }

    #[test]
    fn closures_fail_closed() {
        let program = typed_program(
            "module examples.lower_closure

pub fn apply_twice(f: fn(u64) -> u64, x: u64) -> u64:
    return f(f(x))

const RESULT: u64 = apply_twice(|v: u64| v * 2, 3)
",
        );
        let err = lower_program(&program).expect_err("closures must fail closed");
        assert!(err.message.contains("closure"), "message: {}", err.message);
    }

    #[test]
    fn a_non_literal_panic_message_fails_closed() {
        let program = typed_program(
            "module examples.lower_dynamic_panic

pub fn label() -> Static[Str]:
    return \"nope\"

pub fn check():
    panic(label())
",
        );
        let err =
            lower_program(&program).expect_err("a non-literal panic message must fail closed");
        assert!(
            err.message.contains("non-literal"),
            "message: {}",
            err.message
        );
    }

    #[test]
    fn mmio_register_offset_fail_closed_arms() {
        let empty = TypedProgram::default();
        let cross =
            mmio_register_offset("ForeignRegs", "status", &empty).expect_err("cross-module");
        assert!(
            cross.message.contains("different module") && cross.message.contains("ForeignRegs"),
            "{}",
            cross.message
        );

        let mut prog = TypedProgram::default();
        prog.layouts.push(crate::sema::types::LayoutType {
            name: "Regs".to_string(),
            kind: crate::sema::types::LayoutKind::Mmio,
            endian: crate::sema::types::LayoutEndian::Little,
            size: Some(4),
            padding: 0,
            entries: vec![],
        });
        let missing = mmio_register_offset("Regs", "nope", &prog).expect_err("missing reg");
        assert!(
            missing.message.contains("internal error:")
                && missing.message.contains("declares no register `nope`"),
            "{}",
            missing.message
        );
    }

    #[test]
    fn mmio_access_names_internal_guards() {
        let not_mmio = mmio_access_names(&Type::U32, &[]).expect_err("not Mmio");
        assert!(
            not_mmio.message.contains("not an `Mmio[L]`"),
            "{}",
            not_mmio.message
        );

        let wrong_cap = mmio_access_names(&Type::Named("DeviceCap".to_string(), vec![]), &[])
            .expect_err("DeviceCap");
        assert!(
            wrong_cap.message.contains("not an `Mmio[L]`"),
            "{}",
            wrong_cap.message
        );

        let bad_targ = mmio_access_names(
            &Type::Named(
                "Mmio".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::U32)],
            ),
            &[],
        )
        .expect_err("non-named layout arg");
        assert!(
            bad_targ
                .message
                .contains("layout argument is not a named type"),
            "{}",
            bad_targ.message
        );

        let no_reg = mmio_access_names(
            &Type::Named(
                "Mmio".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::Named(
                    "Regs".to_string(),
                    vec![],
                ))],
            ),
            &[],
        )
        .expect_err("no register");
        assert!(
            no_reg.message.contains("no register name"),
            "{}",
            no_reg.message
        );

        let non_lit = mmio_access_names(
            &Type::Named(
                "Mmio".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::Named(
                    "Regs".to_string(),
                    vec![],
                ))],
            ),
            &[(
                "register".to_string(),
                TypedExpr {
                    span: Span::default(),
                    ty: Type::Named(
                        "Static".to_string(),
                        vec![crate::sema::types::TypeArg::Type(Type::Named(
                            "Str".to_string(),
                            vec![],
                        ))],
                    ),
                    kind: TypedExprKind::Local("r".to_string()),
                },
            )],
        )
        .expect_err("non-literal register");
        assert!(
            non_lit.message.contains("register name is not a literal"),
            "{}",
            non_lit.message
        );
    }
}

#[cfg(test)]
mod census_region_tests {
    use super::*;

    fn marker_fn(region_value: i128) -> MwirFn {
        // `t0` is a live parameter, `t1` the marker's constant argument,
        // `t2` the marker call's unit result, `t3` a live temp after it.
        MwirFn {
            receiver: None,
            params: vec![(Temp(0), AccessMode::Read)],
            ret: Type::U64,
            temp_types: vec![Type::U64, Type::U32, Type::Unit, Type::U64],
            body: vec![
                Inst::ConstInt {
                    dst: Temp(1),
                    ty: Type::U32,
                    value: region_value,
                },
                Inst::Call {
                    dst: Temp(2),
                    write_backs: Vec::new(),
                    key: CENSUS_REGION_FN.to_string(),
                    args: vec![Temp(1)],
                },
                Inst::Copy {
                    dst: Temp(3),
                    src: Temp(0),
                },
                Inst::Return {
                    value: Some(Temp(3)),
                },
            ],
        }
    }

    #[test]
    fn a_marker_leaves_no_temp_and_no_emitting_instruction() {
        let mut f = marker_fn(2);
        install_census_regions_in_fn("raster", &mut f).expect("install");
        assert_eq!(
            f.body,
            vec![
                Inst::RegionMarker { region: 2 },
                Inst::RegionMarker { region: 2 },
                Inst::Copy {
                    dst: Temp(1),
                    src: Temp(0)
                },
                Inst::Return {
                    value: Some(Temp(1))
                },
            ],
            "the constant and the call both become markers, and the live \
             temps renumber down over the two erased ones"
        );
        assert_eq!(
            f.temp_types,
            vec![Type::U64, Type::U64],
            "the marker's constant and unit result leave no frame slot"
        );
        assert_eq!(f.params, vec![(Temp(0), AccessMode::Read)]);
    }

    #[test]
    fn an_unsealed_region_id_is_a_lowering_error() {
        let mut f = marker_fn(4242);
        let error = install_census_regions_in_fn("raster", &mut f).expect_err("unsealed");
        assert!(error.message.contains("sealed vocabulary"), "{error:?}");
    }

    #[test]
    fn a_runtime_region_id_is_a_lowering_error() {
        let mut f = marker_fn(2);
        // Replace the constant with a copy, so the id is no longer static.
        f.body[0] = Inst::Copy {
            dst: Temp(1),
            src: Temp(0),
        };
        let error = install_census_regions_in_fn("raster", &mut f).expect_err("runtime id");
        assert!(error.message.contains("compile-time constant"), "{error:?}");
    }

    #[test]
    fn a_shared_region_constant_is_a_lowering_error() {
        let mut f = marker_fn(2);
        // A live consumer of the marker's constant makes the erasure unsafe.
        f.body.insert(
            2,
            Inst::Copy {
                dst: Temp(3),
                src: Temp(1),
            },
        );
        let error = install_census_regions_in_fn("raster", &mut f).expect_err("shared constant");
        assert!(
            error.message.contains("shares census-region temp"),
            "{error:?}"
        );
    }

    #[test]
    fn a_function_without_markers_is_untouched() {
        let mut f = marker_fn(2);
        f.body.remove(1);
        f.body.remove(0);
        let before = f.clone();
        install_census_regions_in_fn("plain", &mut f).expect("no markers");
        assert_eq!(f, before);
    }

    #[test]
    fn user_census_annotation_spelling_is_not_rewritten_or_removed() {
        let mut program = TypedProgram::default();
        program.fn_decl_modules.insert(
            CENSUS_REGION_FN.to_string(),
            "examples.census_spoof".to_string(),
        );
        program
            .fn_decl_names
            .insert(CENSUS_REGION_FN.to_string(), CENSUS_REGION_FN.to_string());
        let ordinary = marker_fn(2);
        let mut functions = BTreeMap::from([(CENSUS_REGION_FN.to_string(), ordinary.clone())]);

        install_pixels_census_regions(&program, &mut functions).unwrap();
        assert_eq!(functions[CENSUS_REGION_FN], ordinary);
    }
}
