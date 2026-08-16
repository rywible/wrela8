use std::collections::BTreeMap;

use wrela_machine::report as machine_report;

use crate::codegen::CodegenProgram;
use crate::cost::dump as cost_dump;
use crate::cost::{self, CostReport};
use crate::eval::image::{self, ImageDeclRef, ImageGraph, TypedProgramEnums};
use crate::eval::quota;
use crate::eval::value::Value;
use crate::placement::{self, PlacementTable};
use crate::sema::types;

#[derive(Debug, Clone)]
pub struct ImageReportDoc<'a> {
    pub inputs: &'a [BuildInput],
    pub enums: &'a BTreeMap<String, Vec<String>>,
    pub graph: &'a ImageGraph,
    pub placement: &'a PlacementTable,
}

const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInput {
    pub path: String,
    pub digest: String,
}

pub fn address_to_relative_path(address: &str) -> String {
    format!("{}.wr", address.replace('.', "/"))
}

enum DeclFact {
    Arg { label: String, rendered: String },
    Mailbox { rendered: String },
    Edge { to: ImageDeclRef },
}

fn decl_facts(program: &TypedProgramEnums, args: &[image::DeclArg]) -> Vec<DeclFact> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        if a.label == "core" {
            continue;
        }
        if let Value::ImageDecl(r) = &a.value {
            out.push(DeclFact::Edge { to: r.clone() });
            continue;
        }
        let mut nested_refs = Vec::new();
        crate::eval::image_checks::decl_refs_in_value(&a.value, &mut nested_refs);
        let rendered = image::render_value(program, &a.ty, &a.value);
        if a.label == "mailbox" {
            out.push(DeclFact::Mailbox { rendered });
        } else {
            out.push(DeclFact::Arg {
                label: a.label.clone(),
                rendered,
            });
        }
        out.extend(nested_refs.into_iter().map(|to| DeclFact::Edge { to }));
    }
    out
}

fn render_decl_block(
    program: &TypedProgramEnums,
    owner: &ImageDeclRef,
    args: &[image::DeclArg],
    out: &mut String,
    edges: &mut Vec<(ImageDeclRef, ImageDeclRef)>,
) {
    for fact in decl_facts(program, args) {
        match fact {
            DeclFact::Arg { label, rendered } => {
                image::push_line(out, 2, &format!("Arg label={label} value={rendered}"));
            }
            DeclFact::Mailbox { rendered } => {
                image::push_line(out, 2, &format!("Mailbox value={rendered}"));
            }
            DeclFact::Edge { to } => edges.push((owner.clone(), to)),
        }
    }
}

fn render_pool_args(program: &TypedProgramEnums, args: &[image::DeclArg], out: &mut String) {
    for a in args {
        let rendered = match &a.value {
            Value::ImageDecl(r) => r.render(),
            v => image::render_value(program, &a.ty, v),
        };
        image::push_line(out, 2, &format!("Arg label={} value={rendered}", a.label));
    }
}

pub fn render(
    inputs: &[BuildInput],
    enums: &BTreeMap<String, Vec<String>>,
    graph: &ImageGraph,
    placement: &PlacementTable,
) -> Result<String, String> {
    Ok(render_doc(&ImageReportDoc {
        inputs,
        enums,
        graph,
        placement,
    }))
}

pub fn render_doc(doc: &ImageReportDoc<'_>) -> String {
    let program = TypedProgramEnums { enums: doc.enums };
    let mut out = String::new();
    out.push_str("ImageReport v0\n");

    image::push_line(&mut out, 1, &format!("Compiler version={COMPILER_VERSION}"));
    image::push_line(
        &mut out,
        1,
        &machine_report::line_machine_revision(wrela_machine::MACHINE_REVISION_STR),
    );
    if let Some(target) = &doc.graph.target {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Target value={}",
                image::render_value(&program, &target.ty, &target.value)
            ),
        );
    }
    image::push_line(
        &mut out,
        1,
        &format!("Quota max_steps={}", quota::MAX_STEPS),
    );
    image::push_line(
        &mut out,
        1,
        &format!("Quota max_memory={}", quota::MAX_MEMORY),
    );
    image::push_line(
        &mut out,
        1,
        &format!("Quota max_call_depth={}", quota::MAX_CALL_DEPTH),
    );
    image::push_line(
        &mut out,
        1,
        &format!("Quota max_exhaustive_cases={}", quota::MAX_EXHAUSTIVE_CASES),
    );
    for inp in doc.inputs {
        image::push_line(
            &mut out,
            1,
            &machine_report::line_input(&inp.path, &inp.digest),
        );
    }

    if let Some(name) = &doc.graph.name {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Name value={}",
                image::render_value(&program, &name.ty, &name.value)
            ),
        );
    }
    if let Some(target) = &doc.graph.target {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Target value={}",
                image::render_value(&program, &target.ty, &target.value)
            ),
        );
    }

    let mut edges: Vec<(ImageDeclRef, ImageDeclRef)> = Vec::new();

    for (i, d) in doc.graph.devices.iter().enumerate() {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Device index={i} type={}",
                types::render_type(&d.device_type)
            ),
        );
        render_decl_block(
            &program,
            &ImageDeclRef::Device(i),
            &d.args,
            &mut out,
            &mut edges,
        );
    }
    for (i, d) in doc.graph.drivers.iter().enumerate() {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Driver index={i} type={}",
                types::render_type(&d.actor_type)
            ),
        );
        render_decl_block(
            &program,
            &ImageDeclRef::Driver(i),
            &d.args,
            &mut out,
            &mut edges,
        );
    }
    for (i, d) in doc.graph.actors.iter().enumerate() {
        image::push_line(
            &mut out,
            1,
            &format!("Actor index={i} type={}", types::render_type(&d.actor_type)),
        );
        render_decl_block(
            &program,
            &ImageDeclRef::Actor(i),
            &d.args,
            &mut out,
            &mut edges,
        );
    }
    for (i, d) in doc.graph.renderers.iter().enumerate() {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Renderer index={i} params={} actor={}",
                types::render_type(&d.params_type),
                types::render_type(&d.actor_type)
            ),
        );
        render_decl_block(
            &program,
            &ImageDeclRef::Renderer(i),
            &d.args,
            &mut out,
            &mut edges,
        );
    }

    let mut seen_edges = std::collections::BTreeSet::new();
    edges.retain(|edge| seen_edges.insert(edge.clone()));
    for (from, to) in &edges {
        image::push_line(
            &mut out,
            1,
            &format!("Edge from={} to={}", from.render(), to.render()),
        );
    }

    for (name, d) in &doc.graph.pools {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Pool name={name} type={}",
                types::render_type(&d.payload_type)
            ),
        );
        render_pool_args(&program, &d.args, &mut out);
    }
    for (name, d) in &doc.graph.dma_pools {
        image::push_line(
            &mut out,
            1,
            &format!(
                "DmaPool name={name} type={}",
                types::render_type(&d.payload_type)
            ),
        );
        render_pool_args(&program, &d.args, &mut out);
    }

    for (i, s) in doc.graph.on_failures.iter().enumerate() {
        let policy = s
            .args
            .iter()
            .find(|a| a.label == "policy")
            .map(|a| image::render_value(&program, &a.ty, &a.value));
        let mut header = format!("OnFailure index={i}");
        if let Some(v) = &policy {
            header.push_str(&format!(" policy={v}"));
        }
        image::push_line(&mut out, 1, &header);
        for a in &s.args {
            if a.label == "policy" {
                continue;
            }
            image::push_line(
                &mut out,
                2,
                &format!(
                    "Arg label={} value={}",
                    a.label,
                    image::render_value(&program, &a.ty, &a.value)
                ),
            );
        }
    }

    placement::render_placement_section(&mut out, doc.placement);

    out
}

pub fn render_exact_bytes_section(
    out: &mut String,
    layouts: &[types::LayoutType],
) -> Result<(), crate::sema::SemaError> {
    for l in layouts {
        types::push_layout_lines(out, 1, l)?;
    }
    Ok(())
}

pub fn append_linked_cost_summary(
    out: &mut String,
    linked: &crate::linked::LinkedProgram,
    placement: &PlacementTable,
    ghz: f64,
) -> Result<(), String> {
    let table = cost::load_default()?;
    let report = cost::score_linked_program(linked, &table, placement)?;
    out.push_str(&format_cost_summary_scoped(
        &report,
        placement,
        ghz,
        "linked-image",
        linked.executable_words(),
        linked.executable_code_bytes(),
        linked.rodata_bytes(),
        linked.image_bytes,
        None,
    )?);
    out.push_str(&format!(
        "    Scope name=linked-image executable_words={} executable_code_bytes={} fetched_text_bytes={} image_bytes={} rodata_bytes={} sync_frame_max_bytes={} async_frame_total_bytes={}\n",
        linked.executable_words(),
        linked.executable_code_bytes(),
        report.footprint.iter().map(|b| b.fetched_text_bytes).sum::<u64>(),
        linked.image_bytes,
        linked.rodata_bytes(),
        linked.sync_frame_max_bytes,
        linked.async_frame_total_bytes,
    ));
    Ok(())
}

pub fn append_cost_summary(
    out: &mut String,
    program: &CodegenProgram,
    placement: &PlacementTable,
    ghz: f64,
    source: Option<&std::path::Path>,
) -> Result<(), String> {
    let table = cost::load_default()?;
    let mut report = cost::score_program(program, &table, placement)?;
    let attach = cost::WorkloadAttach::load_default_for(source, program, &table, placement)?;
    cost::attach_workloads(&mut report, &attach)?;
    out.push_str(&format_cost_summary_scoped(
        &report,
        placement,
        ghz,
        "closure",
        report.total_words,
        report.total_words.saturating_mul(4),
        0,
        0,
        Some(&attach),
    )?);
    Ok(())
}

pub fn format_cost_summary(
    report: &CostReport,
    placement: &PlacementTable,
    ghz: f64,
    attach: Option<&cost::WorkloadAttach>,
) -> Result<String, String> {
    format_cost_summary_scoped(
        report,
        placement,
        ghz,
        "closure",
        report.total_words,
        report.total_words.saturating_mul(4),
        0,
        0,
        attach,
    )
}

fn format_cost_summary_scoped(
    report: &CostReport,
    placement: &PlacementTable,
    ghz: f64,
    scope: &str,
    executable_words: u64,
    executable_code_bytes: u64,
    rodata_bytes: u64,
    image_bytes: u64,
    attach: Option<&cost::WorkloadAttach>,
) -> Result<String, String> {
    let app = report.owner_totals.get("app").copied().unwrap_or(0);
    let runtime = report.owner_totals.get("runtime").copied().unwrap_or(0);
    let driver = report.owner_totals.get("driver").copied().unwrap_or(0);
    let fetched_text_bytes: u64 = report.footprint.iter().map(|b| b.fetched_text_bytes).sum();
    let mut header = format!(
        "  Cost version={} scope={} digest={} total={} rank_cycles={} schedule_cycles={} footprint_cycles={} executable_words={} executable_code_bytes={} fetched_text_bytes={} rodata_bytes={} image_bytes={} sync_frame_max_bytes={} async_frame_total_bytes={} ghz={}",
        report.version,
        scope,
        report.digest,
        report.total_proxy_cycles,
        report.rank_cycles,
        report.schedule_cycles,
        report.footprint_cycles,
        executable_words,
        executable_code_bytes,
        fetched_text_bytes,
        rodata_bytes,
        if scope == "closure" {
            "n/a".to_string()
        } else {
            image_bytes.to_string()
        },
        report.sync_frame_max_bytes,
        report.async_frame_total_bytes,
        cost::fmt_compact(ghz),
    );
    if let Some(wd) = &report.workloads_digest {
        header.push_str(&format!(" workloads_digest={wd}"));
    }
    header.push('\n');
    let mut out = header;
    cost_dump::append_workload_rows(&mut out, 2, report, attach);
    out.push_str(&format!(
        "    Owner name=app proxy_cycles={app}\n\
         \x20   Owner name=runtime proxy_cycles={runtime}\n\
         \x20   Owner name=driver proxy_cycles={driver}\n"
    ));
    cost_dump::append_core_block(&mut out, 2, report, placement, ghz, false, attach)?;
    Ok(out)
}

pub fn append_convention_section(out: &mut String, program: &CodegenProgram) {
    if program.conventions.is_empty() {
        return;
    }
    let mut rows: Vec<String> = Vec::new();
    let mut frameless = 0usize;
    for (key, conv) in &program.conventions {
        let frame = program.fns.get(key).map(|f| f.frame_size);
        let residents = conv.assignment.resident_count();
        let interesting = residents > 0 || frame == Some(0);
        if !interesting {
            continue;
        }
        if frame == Some(0) {
            frameless += 1;
        }
        let regs: u32 = conv
            .assignment
            .residents()
            .iter()
            .fold(0u32, |m, &(_, r)| m | crate::regalloc::reg_bit(r));
        rows.push(format!(
            "    Fn key={key} frame={} residents={residents} regs={} clobbers={} pool={}\n",
            frame.map_or_else(|| "?".to_string(), |f| f.to_string()),
            crate::regalloc::render_reg_set(regs),
            crate::regalloc::render_reg_set(conv.clobbers),
            conv.pool.len(),
        ));
    }
    if rows.is_empty() {
        return;
    }
    let tail_calls: usize = program
        .fns
        .values()
        .flat_map(|f| f.code.iter())
        .filter(|w| w.text.ends_with("; tail call"))
        .count();
    out.push_str(&format!(
        "  Convention fns={} frameless={frameless} tail_calls={tail_calls}\n",
        rows.len()
    ));
    for r in rows {
        out.push_str(&r);
    }
}

pub use wrela_machine::sha256::sha256_hex;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::image::{DeclArg, TypedValue};
    use crate::sema::types::Type;

    #[test]
    fn sha256_of_empty_string() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_of_abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn address_to_relative_path_joins_dots_as_slashes() {
        assert_eq!(address_to_relative_path("image"), "image.wr");
        assert_eq!(address_to_relative_path("core.bytes"), "core/bytes.wr");
        assert_eq!(
            address_to_relative_path("examples.image_basic"),
            "examples/image_basic.wr"
        );
    }

    fn tv(ty: Type, value: Value) -> TypedValue {
        TypedValue { ty, value }
    }

    fn decl_arg(label: &str, ty: Type, value: Value) -> DeclArg {
        DeclArg {
            label: label.to_string(),
            ty,
            value,
            span: Default::default(),
        }
    }

    fn image_decl_ty() -> Type {
        Type::Named("ImageDecl".to_string(), vec![])
    }

    fn sample_graph() -> (ImageGraph, BTreeMap<String, Vec<String>>) {
        let mut enums = BTreeMap::new();
        enums.insert("Target".to_string(), vec!["wrela_machine_v1".to_string()]);
        enums.insert(
            "Failure".to_string(),
            vec!["Reboot".to_string(), "Halt".to_string()],
        );

        let mut g = ImageGraph::new(
            tv(
                Type::Static(Box::new(Type::Str)),
                Value::Str(b"sample".to_vec()),
            ),
            tv(
                Type::Named("Target".to_string(), vec![]),
                Value::Enum(0, vec![]),
            ),
        );
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![decl_arg("queue_depth", Type::U32, Value::U32(8))],
        });
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![
                decl_arg(
                    "disk",
                    image_decl_ty(),
                    Value::ImageDecl(ImageDeclRef::Driver(0)),
                ),
                decl_arg("mailbox", Type::U32, Value::U32(16)),
            ],
        });
        g.pools.insert(
            "Buffers".to_string(),
            crate::eval::image::PoolDecl {
                payload_type: Type::U32,
                args: vec![decl_arg("slots", Type::U32, Value::U32(4))],
            },
        );
        g.declare_on_failure(vec![decl_arg(
            "policy",
            Type::Named("Failure".to_string(), vec![]),
            Value::Enum(1, vec![]),
        )]);
        g.sealed = true;
        (g, enums)
    }

    #[test]
    fn render_produces_every_section_in_fixed_order() {
        let (g, enums) = sample_graph();
        let inputs = vec![BuildInput {
            path: "image.wr".to_string(),
            digest: sha256_hex(b"placeholder"),
        }];
        let text = render(&inputs, &enums, &g, &PlacementTable::default())
            .expect("no layout asserts registered");

        let expected = format!(
            "ImageReport v0\n\
             \x20 Compiler version={COMPILER_VERSION}\n\
             \x20 Machine revision={}\n\
             \x20 Target value=Target.wrela_machine_v1\n\
             \x20 Quota max_steps={}\n\
             \x20 Quota max_memory={}\n\
             \x20 Quota max_call_depth={}\n\
             \x20 Quota max_exhaustive_cases={}\n\
             \x20 Input path=image.wr sha256={}\n\
             \x20 Name value=sample\n\
             \x20 Target value=Target.wrela_machine_v1\n\
             \x20 Driver index=0 type=Blk\n\
             \x20   Arg label=queue_depth value=8\n\
             \x20 Actor index=0 type=Store\n\
             \x20   Mailbox value=16\n\
             \x20 Edge from=actor#0 to=driver#0\n\
             \x20 Pool name=Buffers type=u32\n\
             \x20   Arg label=slots value=4\n\
             \x20 OnFailure index=0 policy=Failure.Halt\n",
            wrela_machine::MACHINE_REVISION_STR,
            quota::MAX_STEPS,
            quota::MAX_MEMORY,
            quota::MAX_CALL_DEPTH,
            quota::MAX_EXHAUSTIVE_CASES,
            sha256_hex(b"placeholder"),
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn a_registered_layout_assert_no_longer_blocks_render() {
        let (mut g, enums) = sample_graph();
        g.declare_check_layout("check_limits".to_string());
        let text = render(&[], &enums, &g, &PlacementTable::default())
            .expect("registered layout asserts must not block render");
        assert!(text.starts_with("ImageReport v0\n"));
    }

    #[test]
    fn the_convention_section_publishes_what_the_whole_program_pass_chose() {
        use crate::opts::{CompileMode, apply_mode};
        const SRC: &str = r#"
module examples.report_convention

fn leaf(a: u64) -> u64:
    x: u64 = a +% 1
    return x +% x

pub fn caller(a: u64) -> u64:
    keep: u64 = a *% 3
    p: u64 = leaf(a)
    return (keep +% p) +% keep
"#;
        let build = |mode: CompileMode| -> String {
            apply_mode(mode);
            let tokens = crate::syntax::lexer::lex(SRC).expect("lex");
            let module = crate::syntax::parser::parse(tokens).expect("parse");
            let typed = crate::sema::check_typed(&module, "<t>").expect("check");
            let mwir = crate::lower::lower_program(&typed).expect("lower");
            let ctx = crate::mwir::build_layout_ctx(&module, &Default::default()).expect("ctx");
            let prog = crate::codegen::codegen_program(&mwir, &ctx).expect("codegen");
            let mut out = String::new();
            append_convention_section(&mut out, &prog);
            out
        };

        assert_eq!(build(CompileMode::Dev), "", "dev must add no section");

        let text = build(CompileMode::Release);
        apply_mode(CompileMode::Release);
        assert!(
            text.starts_with("  Convention fns="),
            "the section must lead with its own counts:\n{text}"
        );
        assert!(
            text.contains("    Fn key=leaf frame="),
            "every function with a convention of its own must be listed:\n{text}"
        );
        assert!(
            text.contains("frameless=") && text.contains("tail_calls="),
            "the header must carry F3's and F5's counts:\n{text}"
        );
        for want in ["residents=", "regs=x", "clobbers=", "pool="] {
            assert!(text.contains(want), "missing `{want}`:\n{text}");
        }
        let leaf_line = text
            .lines()
            .find(|l| l.contains("Fn key=leaf "))
            .expect("leaf line");
        assert!(
            !leaf_line.contains("clobbers=all"),
            "a leaf's clobber set must be measured: {leaf_line}"
        );
        assert_eq!(build(CompileMode::Release), text);
        apply_mode(CompileMode::Release);
    }

    #[test]
    fn the_exact_bytes_section_is_a_pure_appending_function() {
        let layout = types::LayoutType {
            name: "VirtioIrqMmio".to_string(),
            kind: types::LayoutKind::Mmio,
            endian: types::LayoutEndian::Little,
            size: Some(0x68),
            padding: 0x60,
            entries: vec![
                types::LayoutEntry::Padding {
                    offset: 0,
                    size: 0x60,
                },
                types::LayoutEntry::Field(types::LayoutField {
                    name: "interrupt_status".to_string(),
                    ty: "ReadOnly[u32]".to_string(),
                    offset: 0x60,
                    size: 4,
                }),
            ],
        };
        let mut a = String::from("ImageReport v0\n");
        let mut b = a.clone();
        render_exact_bytes_section(&mut a, std::slice::from_ref(&layout)).expect("complete");
        render_exact_bytes_section(&mut b, std::slice::from_ref(&layout)).expect("complete");
        assert_eq!(a, b);
        assert_eq!(
            a,
            "ImageReport v0\n\
             \x20 Layout name=VirtioIrqMmio kind=mmio endian=little size=104 padding=96\n\
             \x20   Padding offset=0x0 size=96\n\
             \x20   Field name=interrupt_status type=ReadOnly[u32] offset=0x60 size=4\n"
        );
        let mut empty = String::from("ImageReport v0\n");
        render_exact_bytes_section(&mut empty, &[]).expect("nothing to render");
        assert_eq!(empty, "ImageReport v0\n");
    }

    #[test]
    fn render_is_a_pure_function_of_its_arguments() {
        let (g, enums) = sample_graph();
        let inputs = vec![BuildInput {
            path: "image.wr".to_string(),
            digest: sha256_hex(b"x"),
        }];
        let a = render(&inputs, &enums, &g, &PlacementTable::default()).unwrap();
        let b = render(&inputs, &enums, &g, &PlacementTable::default()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn cost_summary_contains_version_and_owners() {
        use crate::codegen::CodegenFn;
        use crate::cost::rule::{CostRule, EmittedWord};
        use std::collections::BTreeMap;

        let mut fns = BTreeMap::new();
        fns.insert(
            "checked_add".to_string(),
            CodegenFn {
                frame_size: 0,
                code: vec![EmittedWord::gpr(
                    0,
                    String::new(),
                    CostRule::Alu,
                    Some(1),
                    &[0, 0],
                )],
                relocs: Vec::new(),
                regions: Vec::new(),
            },
        );
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
        };
        let mut out = String::from("ImageReport v0\n");
        append_cost_summary(
            &mut out,
            &program,
            &PlacementTable::default(),
            cost::DEFAULT_GHZ,
            None,
        )
        .expect("default cost table");
        assert!(
            out.contains("Cost version=3"),
            "missing Cost version line:\n{out}"
        );
        assert!(out.contains("ghz=2.4"), "missing ghz:\n{out}");
        assert!(out.contains("Workload name=flat proxy_cycles="));
        assert!(out.contains("Owner name=app proxy_cycles="));
        assert!(out.contains("Owner name=runtime proxy_cycles="));
        assert!(out.contains("Owner name=driver proxy_cycles="));
        assert!(!out.contains("Term rule="));
        assert!(!out.contains("Fn key="));
        assert!(!out.contains("Placeable "));
    }

    #[test]
    fn format_cost_summary_aggregates_owners() {
        use std::collections::BTreeMap;
        let report = CostReport {
            version: 3,
            digest: "deadbeef".to_string(),
            provenance: "test-prov".to_string(),
            provenance_summary: "T1=1 T2=0 T3=0 T4=0 T5=0 rows=1".to_string(),
            profile: "a76-pi5".to_string(),
            pipelines: 8,
            dispatch_mops: 4,
            dispatch_uops: 8,
            reorder_window: 128,
            total_proxy_cycles: 30,
            schedule_cycles: 30,
            footprint_cycles: 0,
            rank_cycles: 30,
            total_words: 30,
            sync_frame_max_bytes: 0,
            async_frame_total_bytes: 0,
            owner_totals: BTreeMap::from([
                ("app".to_string(), 10u64),
                ("runtime".to_string(), 12u64),
                ("driver".to_string(), 8u64),
            ]),
            fns: vec![],
            workloads_digest: Some("wdigest".to_string()),
            workload_totals: BTreeMap::from([("flat".to_string(), 30u64)]),
            workload_coverage: BTreeMap::new(),
            footprint: vec![cost::CoreBudget {
                n: 0,
                fetched_text_bytes: 1216,
                executable_code_bytes: 1200,
                l1i_bytes: 65536,
                over_l1i_lines: 0,
                over_l2_lines: 0,
                over_l3_lines: 0,
                text_pages: 1,
                itlb_entries: 48,
                over_itlb_pages: 0,
                tlb_l2_entries: 1280,
                over_tlb_l2_pages: 0,
                data_pages: 2,
                over_dtlb_pages: 0,
                over_data_tlb_l2_pages: 0,
                charge: 0,
            }],
        };
        let text =
            format_cost_summary(&report, &PlacementTable::default(), cost::DEFAULT_GHZ, None)
                .expect("format");
        assert_eq!(
            text,
            "  Cost version=3 scope=closure digest=deadbeef total=30 rank_cycles=30 schedule_cycles=30 footprint_cycles=0 executable_words=30 executable_code_bytes=120 fetched_text_bytes=1216 rodata_bytes=0 image_bytes=n/a sync_frame_max_bytes=0 async_frame_total_bytes=0 ghz=2.4 workloads_digest=wdigest\n\
               \x20   Workload name=flat proxy_cycles=30\n\
               \x20   Owner name=app proxy_cycles=10\n\
               \x20   Owner name=runtime proxy_cycles=12\n\
               \x20   Owner name=driver proxy_cycles=8\n\
               \x20   Core n=0 proxy_cycles=0 max_entry_method_proxy_cycles=0\n\
               \x20   Budget n=0 fetched_text_bytes=1216 executable_code_bytes=1200 l1i_bytes=65536 over_l1i_lines=0 over_l2_lines=0 over_l3_lines=0 text_pages=1 itlb_entries=48 over_itlb_pages=0 tlb_l2_entries=1280 over_tlb_l2_pages=0 data_pages=2 over_dtlb_pages=0 over_data_tlb_l2_pages=0 charge=0\n\
               \x20   Shared proxy_cycles=0\n"
        );
    }

    #[test]
    fn format_cost_summary_omits_cores_when_placement_empty() {
        use std::collections::BTreeMap;
        let report = CostReport {
            version: 3,
            digest: "deadbeef".to_string(),
            provenance: "test-prov".to_string(),
            provenance_summary: "T1=1 T2=0 T3=0 T4=0 T5=0 rows=1".to_string(),
            profile: "a76-pi5".to_string(),
            pipelines: 8,
            dispatch_mops: 4,
            dispatch_uops: 8,
            reorder_window: 128,
            total_proxy_cycles: 30,
            schedule_cycles: 30,
            footprint_cycles: 0,
            rank_cycles: 30,
            total_words: 30,
            sync_frame_max_bytes: 0,
            async_frame_total_bytes: 0,
            owner_totals: BTreeMap::from([
                ("app".to_string(), 10u64),
                ("runtime".to_string(), 12u64),
                ("driver".to_string(), 8u64),
            ]),
            fns: vec![],
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
            footprint: Vec::new(),
        };
        let empty = PlacementTable {
            entries: Vec::new(),
            cores: 0,
        };
        let text = format_cost_summary(&report, &empty, cost::DEFAULT_GHZ, None).expect("format");
        assert_eq!(
            text,
            "  Cost version=3 scope=closure digest=deadbeef total=30 rank_cycles=30 schedule_cycles=30 footprint_cycles=0 executable_words=30 executable_code_bytes=120 fetched_text_bytes=0 rodata_bytes=0 image_bytes=n/a sync_frame_max_bytes=0 async_frame_total_bytes=0 ghz=2.4\n\
               \x20   Workload name=flat proxy_cycles=30\n\
               \x20   Owner name=app proxy_cycles=10\n\
               \x20   Owner name=runtime proxy_cycles=12\n\
               \x20   Owner name=driver proxy_cycles=8\n"
        );
    }
}
