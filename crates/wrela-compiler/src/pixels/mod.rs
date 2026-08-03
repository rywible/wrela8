//! Pixels compiler subsystem.
//!
//! The P-1 walking skeleton below accepts exactly one directly marked plane.
//! It is deliberately replaced by the symbolic compiler in P2-P8; expanding
//! this matcher is not a supported extension path.

pub mod diagnostics;
pub mod dump;

pub use dump::{
    PixelsDumpStage, dump_field_graph, dump_frame_program, dump_render_layout, dump_zero_renderers,
};

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::image::{DeclArg, ImageDeclRef, ImageGraph, RendererDecl};
use crate::eval::value::Value;
use crate::sema::typed::{
    TypedCallArg, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn,
    TypedForIter, TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::sema::types::{self, Type, TypeArg};

pub const FRAME_PROGRAM_HEADER_BYTES: usize = 80;
pub const FRAME_PROGRAM_MAGIC: &[u8; 8] = b"WRELAPX\0";
pub const RENDERER_LABELS: &[&str] = &[
    "field",
    "material",
    "display",
    "width",
    "height",
    "refresh_hz",
    "shade_hz",
    "profile",
    "tone_curve",
    "near",
    "far",
    "world_min",
    "world_max",
    "camera_bounds",
    "light_config",
    "exposure_range",
    "environment_range",
    "ao",
    "probes",
    "initialization_deadline_ms",
];

const FIELD_OPERATIONS: &[&str] = &[
    "plane",
    "sphere",
    "box",
    "round_box",
    "capsule",
    "finite_cylinder",
    "finite_cone",
    "torus",
    "uniform_scale",
    "union",
    "intersection",
    "subtract",
    "smooth_union",
    "smooth_intersection",
    "smooth_subtract",
    "sinusoidal_displace",
];

#[derive(Debug, Clone, PartialEq)]
pub struct PlaneSkeleton {
    pub renderer_index: usize,
    pub field: String,
    pub material: String,
    pub material_type: String,
    pub display: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub shade_hz: u32,
    pub frame_program: [u8; FRAME_PROGRAM_HEADER_BYTES],
    pub frame_program_digest: String,
    pub semantic_digest: String,
    pub semantic_seed: [u8; 32],
}

fn arg<'a>(renderer: &'a RendererDecl, label: &str) -> Result<&'a DeclArg, String> {
    renderer
        .args
        .iter()
        .find(|arg| arg.label == label)
        .ok_or_else(|| format!("pixels: renderer is missing `{label}=` after sema"))
}

fn fn_name(renderer: &RendererDecl, label: &str) -> Result<String, String> {
    match &arg(renderer, label)?.value {
        Value::Fn(key) => Ok(key.spelling()),
        _ => Err(format!("pixels: renderer `{label}=` is not a function")),
    }
}

fn u32_arg(renderer: &RendererDecl, label: &str) -> Result<u32, String> {
    let value = crate::eval::value::as_i128(&arg(renderer, label)?.value)
        .ok_or_else(|| format!("pixels: renderer `{label}=` is not an integer"))?;
    u32::try_from(value).map_err(|_| format!("pixels: renderer `{label}={value}` is out of range"))
}

fn call_base(spelling: &str) -> &str {
    spelling
        .strip_prefix("fn:")
        .unwrap_or(spelling)
        .split('[')
        .next()
        .unwrap_or(spelling)
}

#[derive(Clone)]
struct LocatedFn {
    module: String,
    key: String,
    function: TypedFn,
}

fn local_function(program: &TypedProgram, name: &str) -> Option<TypedFn> {
    program
        .fns
        .get(name)
        .or_else(|| program.imported.fns.get(name))
        .cloned()
}

fn root_function(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    name: &str,
) -> Result<LocatedFn, String> {
    let mut local = programs.iter().filter_map(|(module, program)| {
        program.fns.get(name).cloned().map(|function| LocatedFn {
            module: module.clone(),
            key: name.to_string(),
            function,
        })
    });
    if let Some(found) = local.next() {
        if local.next().is_none() {
            return Ok(found);
        }
    }
    if let Some(function) = local_function(owner, name) {
        return Ok(LocatedFn {
            module: "<image-owner>".to_string(),
            key: name.to_string(),
            function,
        });
    }
    Err(format!(
        "pixels: renderer root `{name}` is unavailable or ambiguous"
    ))
}

fn called_function(
    programs: &BTreeMap<String, TypedProgram>,
    current_module: &str,
    name: &str,
) -> Option<LocatedFn> {
    if let Some(program) = programs.get(current_module) {
        if let Some(function) = local_function(program, name) {
            return Some(LocatedFn {
                module: current_module.to_string(),
                key: name.to_string(),
                function,
            });
        }
    }
    let mut found = programs.iter().filter_map(|(module, program)| {
        program.fns.get(name).cloned().map(|function| LocatedFn {
            module: module.clone(),
            key: name.to_string(),
            function,
        })
    });
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

struct SceneAnalysis<'a> {
    programs: &'a BTreeMap<String, TypedProgram>,
    active: BTreeSet<String>,
    visited: BTreeSet<String>,
    functions: BTreeMap<String, TypedFn>,
    field_operations: Vec<String>,
    mark_types: Vec<Type>,
}

impl<'a> SceneAnalysis<'a> {
    fn new(programs: &'a BTreeMap<String, TypedProgram>) -> Self {
        Self {
            programs,
            active: BTreeSet::new(),
            visited: BTreeSet::new(),
            functions: BTreeMap::new(),
            field_operations: Vec::new(),
            mark_types: Vec::new(),
        }
    }

    fn visit_function(&mut self, located: LocatedFn, field_root: bool) -> Result<(), String> {
        let identity = format!("{}::{}", located.module, located.key);
        if self.visited.contains(&identity) {
            return Ok(());
        }
        if !self.active.insert(identity.clone()) {
            return Err(format!(
                "pixels: P-1 field helper recursion reaches `{}`",
                located.key
            ));
        }
        self.functions
            .insert(identity.clone(), located.function.clone());
        self.visit_stmts(&located.function.body, &located.module, field_root)?;
        self.active.remove(&identity);
        self.visited.insert(identity);
        Ok(())
    }

    fn visit_args(
        &mut self,
        args: &[TypedCallArg],
        module: &str,
        field_root: bool,
    ) -> Result<(), String> {
        for arg in args {
            if let Some(value) = &arg.value {
                self.visit_expr(value, module, field_root)?;
            }
        }
        Ok(())
    }

    fn visit_expr(
        &mut self,
        expr: &TypedExpr,
        module: &str,
        field_root: bool,
    ) -> Result<(), String> {
        match &expr.kind {
            TypedExprKind::Call {
                callee,
                receiver,
                args,
            } => {
                let spelling = callee.spelling();
                let base = call_base(&spelling);
                if field_root && FIELD_OPERATIONS.contains(&base) {
                    self.field_operations.push(base.to_string());
                } else if field_root && base == "mark" {
                    let material = args
                        .get(2)
                        .and_then(|arg| arg.value.as_ref())
                        .map(|value| value.ty.clone())
                        .ok_or_else(|| {
                            "pixels: `mark` lacks its material value after sema".to_string()
                        })?;
                    self.mark_types.push(material);
                } else if let Some(function) = called_function(self.programs, module, base) {
                    self.visit_function(function, field_root)?;
                } else if field_root && expr.ty == Type::Named("Field".to_string(), vec![]) {
                    return Err(format!(
                        "pixels: P-1 cannot resolve field-producing helper `{spelling}`"
                    ));
                }
                if let Some(receiver) = receiver {
                    self.visit_expr(receiver, module, field_root)?;
                }
                self.visit_args(args, module, field_root)?;
            }
            TypedExprKind::CallValue(callee, args) => {
                if field_root {
                    return Err(
                        "pixels: P-1 field roots may not call a closure or function value"
                            .to_string(),
                    );
                }
                self.visit_expr(callee, module, field_root)?;
                self.visit_args(args, module, field_root)?;
            }
            TypedExprKind::Field(base, _)
            | TypedExprKind::ToScalar(base)
            | TypedExprKind::Neg(base)
            | TypedExprKind::BitNot(base)
            | TypedExprKind::Take(base)
            | TypedExprKind::Try(base, _)
            | TypedExprKind::Not(base)
            | TypedExprKind::Panic(base)
            | TypedExprKind::Await(base)
            | TypedExprKind::Send(base) => self.visit_expr(base, module, field_root)?,
            TypedExprKind::Index(a, b)
            | TypedExprKind::Binary(_, a, b)
            | TypedExprKind::OpCall(_, a, b)
            | TypedExprKind::And(a, b)
            | TypedExprKind::Or(a, b) => {
                self.visit_expr(a, module, field_root)?;
                self.visit_expr(b, module, field_root)?;
            }
            TypedExprKind::Is(value, _) => self.visit_expr(value, module, field_root)?,
            TypedExprKind::EnumConstruct { args, .. } => {
                self.visit_args(args, module, field_root)?
            }
            TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
                for item in items {
                    self.visit_expr(item, module, field_root)?;
                }
            }
            TypedExprKind::StructLiteral { fields, .. } => {
                for (_, value) in fields {
                    self.visit_expr(value, module, field_root)?;
                }
            }
            TypedExprKind::Intrinsic { receiver, args, .. } => {
                if let Some(receiver) = receiver {
                    self.visit_expr(receiver, module, field_root)?;
                }
                for (_, value) in args {
                    self.visit_expr(value, module, field_root)?;
                }
            }
            TypedExprKind::Closure { body, .. } => {
                if field_root {
                    return Err("pixels: P-1 field roots may not contain closures".to_string());
                }
                match body {
                    TypedClosureBody::Expr(value) => self.visit_expr(value, module, field_root)?,
                    TypedClosureBody::Suite(body) => self.visit_stmts(body, module, field_root)?,
                }
            }
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
            | TypedExprKind::FnRef(_)
            | TypedExprKind::PoolName(_)
            | TypedExprKind::GroupChild(_) => {}
        }
        Ok(())
    }

    fn visit_stmts(
        &mut self,
        stmts: &[TypedStmt],
        module: &str,
        field_root: bool,
    ) -> Result<(), String> {
        for stmt in stmts {
            match &stmt.kind {
                TypedStmtKind::Let { value, .. } => self.visit_expr(value, module, field_root)?,
                TypedStmtKind::Assign { target, value } => {
                    self.visit_expr(target, module, field_root)?;
                    self.visit_expr(value, module, field_root)?;
                }
                TypedStmtKind::If {
                    cond,
                    then_branch,
                    elifs,
                    else_branch,
                } => {
                    self.visit_expr(cond, module, field_root)?;
                    self.visit_stmts(then_branch, module, field_root)?;
                    for elif in elifs {
                        self.visit_expr(&elif.cond, module, field_root)?;
                        self.visit_stmts(&elif.body, module, field_root)?;
                    }
                    if let Some(branch) = else_branch {
                        self.visit_stmts(branch, module, field_root)?;
                    }
                }
                TypedStmtKind::Match { scrutinee, arms } => {
                    self.visit_expr(scrutinee, module, field_root)?;
                    for arm in arms {
                        if let Some(guard) = &arm.guard {
                            self.visit_expr(guard, module, field_root)?;
                        }
                        self.visit_stmts(&arm.body, module, field_root)?;
                    }
                }
                TypedStmtKind::For { iter, body, .. } => {
                    match iter {
                        TypedForIter::Range(a, b, _) => {
                            self.visit_expr(a, module, field_root)?;
                            self.visit_expr(b, module, field_root)?;
                        }
                        TypedForIter::Expr(value) => self.visit_expr(value, module, field_root)?,
                    }
                    self.visit_stmts(body, module, field_root)?;
                }
                TypedStmtKind::While { cond, body, .. } => {
                    self.visit_expr(cond, module, field_root)?;
                    self.visit_stmts(body, module, field_root)?;
                }
                TypedStmtKind::Return(Some(value))
                | TypedStmtKind::ExprStmt(value)
                | TypedStmtKind::BareSend { expr: value, .. } => {
                    self.visit_expr(value, module, field_root)?
                }
                TypedStmtKind::Assert { cond, message }
                | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
                    self.visit_expr(cond, module, field_root)?;
                    if let Some(message) = message {
                        self.visit_expr(message, module, field_root)?;
                    }
                }
                TypedStmtKind::Defer(TypedDeferBody::Expr(value)) => {
                    self.visit_expr(value, module, field_root)?
                }
                TypedStmtKind::Defer(TypedDeferBody::Suite(body))
                | TypedStmtKind::WithGroup { body, .. } => {
                    self.visit_stmts(body, module, field_root)?
                }
                TypedStmtKind::Break
                | TypedStmtKind::Continue
                | TypedStmtKind::Pass
                | TypedStmtKind::Return(None) => {}
            }
        }
        Ok(())
    }
}

fn material_type(function: &TypedFn) -> Option<Type> {
    let Type::Named(name, args) = &function.params.first()?.ty else {
        return None;
    };
    if name != "SurfaceContext" {
        return None;
    }
    match args.as_slice() {
        [TypeArg::Type(material)] => Some(material.clone()),
        _ => None,
    }
}

fn digest_bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn encode_header(renderer_index: usize) -> Result<([u8; 80], String), String> {
    let renderer_index = u16::try_from(renderer_index)
        .map_err(|_| "pixels: renderer index exceeds u16".to_string())?;
    let mut bytes = [0u8; FRAME_PROGRAM_HEADER_BYTES];
    bytes[0..8].copy_from_slice(FRAME_PROGRAM_MAGIC);
    bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
    bytes[10..12].copy_from_slice(&(FRAME_PROGRAM_HEADER_BYTES as u16).to_le_bytes());
    // P-1 reserves no production wire-format flags.
    bytes[16..20].copy_from_slice(&(FRAME_PROGRAM_HEADER_BYTES as u32).to_le_bytes());
    bytes[20..22].copy_from_slice(&renderer_index.to_le_bytes());
    bytes[24..28].copy_from_slice(&1u32.to_le_bytes()); // numeric revision
    bytes[28..32].copy_from_slice(&1u32.to_le_bytes()); // formal revision
    // The P-1 walking skeleton has no v1 tables. Keep table_count and every
    // reserved byte zero; P5 owns the table-kind namespace and will replace
    // this header-only program with the production representation.
    let digest = wrela_machine::sha256::sha256_hex(&bytes);
    bytes[48..80].copy_from_slice(&digest_bytes(&digest));
    Ok((bytes, digest))
}

fn walking_skeleton_seed(
    frame_program: &[u8; FRAME_PROGRAM_HEADER_BYTES],
    semantic_digest: &str,
) -> [u8; 32] {
    // Preserve the reviewed P-1 displayed-frame digest without emitting the
    // malformed P-1 envelope. Before P0, the walking skeleton mixed its
    // semantic prefix into v1 table-count/reserved bytes and then used that
    // envelope's digest as its pixel seed. P0 keeps that seed derivation only
    // as generated-actor compatibility metadata; `frame_program` itself stays
    // a valid zero-table v1 header.
    let mut seed_input = *frame_program;
    seed_input[12..16].copy_from_slice(&1u32.to_le_bytes());
    seed_input[32..48].copy_from_slice(&digest_bytes(semantic_digest)[..16]);
    seed_input[48..80].fill(0);
    digest_bytes(&wrela_machine::sha256::sha256_hex(&seed_input))
        .try_into()
        .expect("SHA-256 hex decodes to 32 bytes")
}

pub fn compile_plane_skeleton(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    graph: &ImageGraph,
) -> Result<PlaneSkeleton, String> {
    let [renderer] = graph.renderers.as_slice() else {
        return Err(format!(
            "pixels: plane skeleton requires exactly one renderer, found {}",
            graph.renderers.len()
        ));
    };
    let field = fn_name(renderer, "field")?;
    let material = fn_name(renderer, "material")?;
    let display = arg(renderer, "display")?.value.clone();
    let (display_index, display) = match display {
        Value::ImageDecl(ImageDeclRef::Driver(index)) => {
            (index, ImageDeclRef::Driver(index).render())
        }
        Value::ImageDecl(reference) => {
            return Err(format!(
                "pixels: renderer `display=` must name a display driver, found {}",
                reference.render()
            ));
        }
        _ => return Err("pixels: renderer display is not an image declaration".to_string()),
    };
    let display_decl = graph
        .drivers
        .get(display_index)
        .ok_or_else(|| format!("pixels: renderer display driver#{display_index} is unavailable"))?;
    if !matches!(&display_decl.actor_type, Type::Named(name, _) if name == "Display") {
        return Err(format!(
            "pixels: renderer `display=` must bind `Display`, found `{}`",
            types::render_type(&display_decl.actor_type)
        ));
    }
    let field_fn = root_function(owner, programs, &field)?;
    let material_fn = root_function(owner, programs, &material)?;
    let expected_material = material_type(&material_fn.function)
        .ok_or_else(|| "pixels: material root lacks `SurfaceContext[M]`".to_string())?;
    let mut analysis = SceneAnalysis::new(programs);
    analysis.visit_function(field_fn, true)?;
    analysis.visit_function(material_fn, false)?;
    let mut nominal_materials: Vec<Type> = Vec::new();
    for material in &analysis.mark_types {
        if !nominal_materials.iter().any(|prior| prior == material) {
            nominal_materials.push(material.clone());
        }
    }
    if nominal_materials.is_empty() {
        return Err(
            "pixels: renderer `@field` must reach at least one `mark(..., material=M.*)`"
                .to_string(),
        );
    }
    if nominal_materials.len() != 1 {
        return Err(format!(
            "pixels: `@field` reaches more than one nominal material type: {}",
            nominal_materials
                .iter()
                .map(types::render_type)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if nominal_materials[0] != expected_material {
        return Err(format!(
            "pixels: renderer material mismatch: field marks use `{}`, `SurfaceContext` uses `{}`",
            types::render_type(&nominal_materials[0]),
            types::render_type(&expected_material)
        ));
    }
    if analysis.field_operations != ["plane"] {
        return Err(format!(
            "pixels: P-1 plane skeleton accepts exactly one marked `plane`; found [{}]",
            analysis.field_operations.join(", ")
        ));
    }
    if analysis.mark_types.len() != 1 {
        return Err(format!(
            "pixels: P-1 plane skeleton requires exactly one `mark`, found {}",
            analysis.mark_types.len()
        ));
    }
    let width = u32_arg(renderer, "width")?;
    let height = u32_arg(renderer, "height")?;
    if width != wrela_machine::pixels::WIDTH || height != wrela_machine::pixels::HEIGHT {
        return Err(format!(
            "pixels: P-1 plane skeleton extent must be {}x{}, found {width}x{height}",
            wrela_machine::pixels::WIDTH,
            wrela_machine::pixels::HEIGHT
        ));
    }
    let refresh_hz = u32_arg(renderer, "refresh_hz")?;
    let shade_hz = u32_arg(renderer, "shade_hz")?;
    if refresh_hz != wrela_machine::pixels::REFRESH_HZ
        || shade_hz != wrela_machine::pixels::REFRESH_HZ
    {
        return Err(format!(
            "pixels: P-1 plane skeleton rate must be {0}/{0} Hz, found {refresh_hz}/{shade_hz}",
            wrela_machine::pixels::REFRESH_HZ
        ));
    }
    let mut semantic_program = TypedProgram::default();
    semantic_program.fns = analysis.functions;
    let semantic_text = crate::sema::typed::dump(&semantic_program);
    let semantic_digest = wrela_machine::sha256::sha256_hex(semantic_text.as_bytes());
    let (frame_program, frame_program_digest) = encode_header(0)?;
    let semantic_seed = walking_skeleton_seed(&frame_program, &semantic_digest);
    Ok(PlaneSkeleton {
        renderer_index: 0,
        field,
        material,
        material_type: types::render_type(&expected_material),
        display,
        width,
        height,
        refresh_hz,
        shade_hz,
        frame_program,
        frame_program_digest,
        semantic_digest,
        semantic_seed,
    })
}

pub fn smooth_min(a: f64, b: f64, k: f64) -> f64 {
    if a <= b - k {
        a
    } else if b <= a - k {
        b
    } else {
        let h = 0.5 + 0.5 * (b - a) / k;
        b + (a - b) * h - k * h * (1.0 - h)
    }
}

pub fn sinusoidal_contract(amplitude: f64, frequency: f64) -> [f64; 4] {
    [
        amplitude.abs(),
        (amplitude * frequency).abs(),
        (amplitude * frequency * frequency).abs(),
        (amplitude * frequency * frequency * frequency).abs(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_interior_root_is_not_a_leaf_root() {
        let k = 4.0;
        let a = k / 4.0;
        assert_ne!(a, 0.0);
        assert_eq!(smooth_min(a, a, k), 0.0);
        assert!(a.abs() <= k / 4.0);
    }

    #[test]
    fn saturated_branch_returns_selected_operand_bits() {
        let a = -3.25f64;
        let b = 2.0f64;
        assert_eq!(smooth_min(a, b, 0.5).to_bits(), a.to_bits());
    }

    #[test]
    fn nested_smooth_composition_keeps_interior_candidate() {
        let inner = smooth_min(1.0, 1.0, 4.0);
        assert_eq!(smooth_min(inner, 2.0, 0.5), 0.0);
    }

    #[test]
    fn deformation_contract_is_derived_and_rejects_false_bound() {
        let contract = sinusoidal_contract(2.0, 3.0);
        assert_eq!(contract, [2.0, 6.0, 18.0, 54.0]);
        let author_claim = [2.0, 5.9, 18.0, 54.0];
        assert!(
            author_claim
                .iter()
                .zip(contract)
                .any(|(claimed, derived)| *claimed < derived)
        );
    }

    #[test]
    fn frame_program_header_is_explicitly_eighty_bytes() {
        let (bytes, digest) = encode_header(0).unwrap();
        assert_eq!(bytes.len(), 80);
        assert_eq!(&bytes[0..8], FRAME_PROGRAM_MAGIC);
        assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(bytes[10..12].try_into().unwrap()), 80);
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 80);
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(bytes[32..34].try_into().unwrap()), 0);
        assert_eq!(&bytes[34..48], &[0; 14]);
        assert_eq!(digest.len(), 64);
        assert_eq!(&bytes[48..80], digest_bytes(&digest));
    }

    #[test]
    fn semantic_source_digest_changes_generated_pixels_not_header_only_program() {
        let semantic_a = wrela_machine::sha256::sha256_hex(b"plane material blue");
        let semantic_b = wrela_machine::sha256::sha256_hex(b"plane material red");
        let (header_a, digest_a) = encode_header(0).unwrap();
        let (header_b, digest_b) = encode_header(0).unwrap();
        let seed_a = walking_skeleton_seed(&header_a, &semantic_a);
        let seed_b = walking_skeleton_seed(&header_b, &semantic_b);
        assert_eq!(digest_a, digest_b);
        assert_eq!(header_a, header_b);

        let code_a = crate::codegen::emit_pixels_plane_renderer(&header_a, &seed_a);
        let code_b = crate::codegen::emit_pixels_plane_renderer(&header_b, &seed_b);
        let words_a: Vec<u32> = code_a.code.iter().map(|word| word.word).collect();
        let words_b: Vec<u32> = code_b.code.iter().map(|word| word.word).collect();
        assert_ne!(words_a, words_b);
    }

    #[test]
    fn walking_skeleton_keeps_the_reviewed_p1_pixel_seed() {
        let semantic = "4339211ebc497254b70372e4bcf9501407d9ff588427661aa93d101d047c4583";
        let (header, _) = encode_header(0).unwrap();
        assert_eq!(
            walking_skeleton_seed(&header, semantic).as_slice(),
            digest_bytes("e778817fd997a1eb45c0a463f792569fd0534004153b18dfec13691a923048b4")
        );
    }

    #[test]
    fn field_operation_census_catches_wrappers_and_deformations() {
        for name in [
            "plane",
            "sphere",
            "uniform_scale",
            "smooth_union",
            "sinusoidal_displace",
        ] {
            assert!(FIELD_OPERATIONS.contains(&name), "missing `{name}`");
        }
        assert_eq!(call_base("fn:mark[Material]"), "mark");
    }
}
