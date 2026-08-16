use std::collections::BTreeMap;

use crate::eval::image::ImageGraph;
use crate::eval::interp::{self, EvalError};
use crate::eval::value::Value;
use crate::layout::ImageLayout;
use crate::sema::typed::{TypedFn, TypedProgram, TypedStruct};
use crate::sema::types::Type;
use wrela_machine::{console, layout as machine_layout};

const REPORT_FIELDS: &[(&str, ReportField)] = &[
    ("machine_revision", ReportField::MachineRevision),
    ("entry", ReportField::Entry),
    ("pages_base", ReportField::PagesBase),
    ("pages_size", ReportField::PagesSize),
    ("stacks_base", ReportField::StacksBase),
    ("stacks_size", ReportField::StacksSize),
    ("code_base", ReportField::CodeBase),
    ("code_size", ReportField::CodeSize),
];

#[derive(Clone, Copy)]
enum ReportField {
    MachineRevision,
    Entry,
    PagesBase,
    PagesSize,
    StacksBase,
    StacksSize,
    CodeBase,
    CodeSize,
}

pub fn run(program: &TypedProgram, graph: &ImageGraph, layout: &ImageLayout) -> Result<(), String> {
    if graph.layout_asserts.is_empty() {
        return Ok(());
    }
    let report = synthesize_report(program, layout).map_err(|e| format!("error[build]: {e}\n"))?;
    for a in &graph.layout_asserts {
        let fn_key = &a.fn_key;
        let Some(f) = resolve_fn(program, fn_key) else {
            return Err(format!(
                "error[build]: `@layout_assert` fn `{fn_key}` is not in the `@image` module's \
                 typed program\n"
            ));
        };
        match interp::eval_layout_assert(program, fn_key, f, report.clone()) {
            Ok(()) => {}
            Err(e) => return Err(render_failure(fn_key, e)),
        }
    }
    Ok(())
}

fn resolve_fn<'a>(program: &'a TypedProgram, fn_key: &str) -> Option<&'a TypedFn> {
    program.fns.get(fn_key).or_else(|| {
        program
            .imported
            .fns
            .get(fn_key)
            .map(|function| function.as_ref())
    })
}

fn render_failure(fn_key: &str, e: EvalError) -> String {
    let mut s = format!(
        "error[build]: `@layout_assert` fn `{fn_key}` failed: {}\n",
        e.message
    );
    for frame in &e.stack {
        s.push_str(&format!("  while evaluating `{frame}`\n"));
    }
    s
}

fn synthesize_report(program: &TypedProgram, layout: &ImageLayout) -> Result<Value, String> {
    let s = find_image_report_struct(program).ok_or_else(|| {
        "registered `@layout_assert` fn(s) need the stdlib `ImageReport` type \
         (`from core.image_report import ImageReport`); it is not in this module's \
         typed program"
            .to_string()
    })?;
    let (pages_base, pages_size) = pages_region();
    let (stacks_base, stacks_size) = stacks_region(layout.cores);
    let code = layout
        .sections
        .iter()
        .find(|sec| sec.name == "code")
        .ok_or_else(|| {
            "laid-out image has no `code` section; cannot synthesize `ImageReport`".to_string()
        })?;

    let mut by_name: BTreeMap<&str, Value> = BTreeMap::new();
    for (name, kind) in REPORT_FIELDS {
        let v = match kind {
            ReportField::MachineRevision => Value::U32(wrela_machine::MACHINE_REVISION),
            ReportField::Entry => Value::U64(layout.entry),
            ReportField::PagesBase => Value::U64(pages_base),
            ReportField::PagesSize => Value::U64(pages_size),
            ReportField::StacksBase => Value::U64(stacks_base),
            ReportField::StacksSize => Value::U64(stacks_size),
            ReportField::CodeBase => Value::U64(code.base),
            ReportField::CodeSize => Value::U64(code.size),
        };
        by_name.insert(*name, v);
    }

    let mut fields = Vec::with_capacity(s.fields.len());
    for name in &s.fields {
        let Some(expected_ty) = s.field_types.get(name) else {
            return Err(format!(
                "ImageReport field `{name}` has no recorded type in the typed tree"
            ));
        };
        let Some(v) = by_name.remove(name.as_str()) else {
            return Err(format!(
                "stdlib `ImageReport` field `{name}` is not one this compiler synthesizes \
                 (expected exactly: {})",
                REPORT_FIELDS
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        check_field_ty(name, expected_ty, &v)?;
        fields.push(v);
    }
    if !by_name.is_empty() {
        let missing: Vec<&str> = by_name.keys().copied().collect();
        return Err(format!(
            "stdlib `ImageReport` is missing field(s) this compiler synthesizes: {}",
            missing.join(", ")
        ));
    }
    Ok(Value::Struct(fields))
}

fn check_field_ty(name: &str, ty: &Type, v: &Value) -> Result<(), String> {
    let ok = match (ty, v) {
        (Type::U32, Value::U32(_)) => true,
        (Type::U64 | Type::Usize, Value::U64(_)) => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "stdlib `ImageReport` field `{name}` has type `{}`; the synthesizer expects \
             `u32` for `machine_revision` and `u64`/`usize` for section bases/sizes",
            crate::sema::types::render_type(ty)
        ))
    }
}

fn find_image_report_struct(program: &TypedProgram) -> Option<&TypedStruct> {
    for s in program.structs.values().chain(
        program
            .imported
            .structs
            .values()
            .map(|value| value.as_ref()),
    ) {
        if struct_matches_report(s) {
            return Some(s);
        }
    }
    None
}

fn struct_matches_report(s: &TypedStruct) -> bool {
    if s.fields.len() != REPORT_FIELDS.len() {
        return false;
    }
    let names: BTreeMap<&str, ()> = s.fields.iter().map(|n| (n.as_str(), ())).collect();
    REPORT_FIELDS.iter().all(|(n, _)| names.contains_key(n))
}

fn pages_region() -> (u64, u64) {
    let base = machine_layout::MACHINE_INFO_BASE;
    let end = console::DATA_BASE + console::DATA_SIZE;
    (base, end - base)
}

fn stacks_region(n_cores: usize) -> (u64, u64) {
    let n = n_cores.max(1);
    (
        machine_layout::core_stack_base_n(0, n),
        n as u64 * machine_layout::CORE_STACK_SIZE,
    )
}
