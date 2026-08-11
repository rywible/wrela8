//! Stable renderer-parameter path extraction for P1.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::attrs::{NumericRange, RateContract};
use crate::sema::typed::{
    TypedCallArg, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn,
    TypedForIter, TypedProgram, TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::Type;

use super::ids::{MaterialId, ParamId, ScalarId};
use super::material_graph::MaterialKind;
use super::scalar::ScalarOp;
use super::symbolic::SymbolicGraph;
use super::{LocatedFn, call_base, called_function, root_function};

#[derive(Debug, Clone, PartialEq)]
pub struct ParameterContract {
    pub path: Vec<usize>,
    pub ty: Type,
    pub range: NumericRange,
    pub rate: Option<RateContract>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParamUse {
    Geometry,
    Material,
    Camera,
    Light,
    Exposure,
    Post,
    Probe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarType {
    F32,
    F64,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    Usize,
    Isize,
}

impl ScalarType {
    pub fn size(self) -> u32 {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::F32 | Self::U32 | Self::I32 => 4,
            Self::F64 | Self::U64 | Self::I64 | Self::Usize | Self::Isize => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScalarRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScalarRate {
    pub max_delta: f64,
    pub max_second_delta: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamSlot {
    pub id: ParamId,
    pub path: Vec<usize>,
    pub component: Option<u8>,
    pub scalar_ty: ScalarType,
    pub range: ScalarRange,
    pub rate: Option<ScalarRate>,
    pub immutable: bool,
    pub uses: BTreeSet<ParamUse>,
    pub packed_offset: u32,
}

/// Stable identity shared by the verified parameter layout and generated
/// generic snapshot code. This is not an address or source-layout offset.
pub(crate) fn parameter_path_key(path: &[usize], component: Option<u8>) -> Result<u64, String> {
    let mut key = 0xcbf2_9ce4_8422_2325_u64;
    for byte in u64::try_from(path.len())
        .map_err(|_| "renderer snapshot parameter path is too long".to_string())?
        .to_le_bytes()
    {
        key = (key ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    for index in path {
        for byte in u64::try_from(*index)
            .map_err(|_| "renderer snapshot parameter path index exceeds u64".to_string())?
            .to_le_bytes()
        {
            key = (key ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
    }
    for byte in u64::from(component.map_or(u32::MAX, u32::from)).to_le_bytes() {
        key = (key ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    Ok(key)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyDigestSchema {
    pub fields: Vec<&'static str>,
    pub schema_digest: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParameterLayout {
    pub slots: Vec<ParamSlot>,
    pub packed_bytes: u32,
    pub frame_dependencies: FrameDependencyTuple,
    pub digest_schema: DependencyDigestSchema,
}

/// One exact field in the runtime/sealed frame-dependency tuple. Runtime
/// fields name bytes supplied by `RenderFrame`; sealed fields name immutable
/// renderer coefficients that are hashed alongside them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameInputDependency {
    pub path: String,
    pub use_kind: ParamUse,
    pub scalar_ty: ScalarType,
    pub element_count: u32,
    pub packed_offset: u32,
    pub runtime: bool,
}

/// Complete non-`P` state hashed by the frame dependency digest.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameDependencyTuple {
    pub fields: Vec<FrameInputDependency>,
    pub runtime_bytes: u32,
    pub camera_contract: [f64; 9],
    pub light_capacity: u32,
    pub light_kinds: Vec<String>,
    pub environment_min: [f32; 3],
    pub environment_max: [f32; 3],
    pub exposure: [f32; 2],
    pub post_id: String,
    pub ao_version: u32,
    pub probe_version: u32,
    pub output_mode: String,
    pub deterministic_frame_phase: [u32; 2],
}

fn frame_dependency_tuple(
    config: &super::config::RendererConfig,
) -> Result<FrameDependencyTuple, String> {
    let mut fields = Vec::new();
    let mut offset = 0_u32;
    let mut runtime = |path: &str,
                       use_kind: ParamUse,
                       scalar_ty: ScalarType,
                       element_count: u32|
     -> Result<(), String> {
        offset = align_up(offset, scalar_ty.size())?;
        let packed_offset = offset;
        offset = offset
            .checked_add(
                scalar_ty
                    .size()
                    .checked_mul(element_count)
                    .ok_or_else(|| "P015: frame dependency byte count overflow".to_string())?,
            )
            .ok_or_else(|| "P015: frame dependency byte count overflow".to_string())?;
        fields.push(FrameInputDependency {
            path: path.to_string(),
            use_kind,
            scalar_ty,
            element_count,
            packed_offset,
            runtime: true,
        });
        Ok(())
    };
    runtime("frame.camera.eye", ParamUse::Camera, ScalarType::F32, 3)?;
    runtime("frame.camera.forward", ParamUse::Camera, ScalarType::F32, 3)?;
    runtime("frame.camera.right", ParamUse::Camera, ScalarType::F32, 3)?;
    runtime("frame.camera.up", ParamUse::Camera, ScalarType::F32, 3)?;
    runtime(
        "frame.lights[*].coefficients",
        ParamUse::Light,
        ScalarType::F32,
        config
            .light_capacity
            .checked_mul(15)
            .ok_or_else(|| "P015: light coefficient count overflow".to_string())?,
    )?;
    runtime("frame.exposure", ParamUse::Exposure, ScalarType::F32, 1)?;
    runtime("frame.environment", ParamUse::Light, ScalarType::F32, 3)?;
    runtime("frame.frame_index", ParamUse::Probe, ScalarType::U64, 1)?;
    let runtime_bytes = offset;
    for (path, use_kind, scalar_ty) in [
        ("sealed.tone_curve_id", ParamUse::Post, ScalarType::U64),
        ("sealed.ao_version", ParamUse::Post, ScalarType::U32),
        ("sealed.probe_version", ParamUse::Probe, ScalarType::U32),
        ("sealed.output_mode", ParamUse::Post, ScalarType::U64),
        ("sealed.frame_phase", ParamUse::Post, ScalarType::U64),
    ] {
        fields.push(FrameInputDependency {
            path: path.to_string(),
            use_kind,
            scalar_ty,
            element_count: 1,
            packed_offset: 0,
            runtime: false,
        });
    }
    Ok(FrameDependencyTuple {
        fields,
        runtime_bytes,
        camera_contract: [
            config.near,
            config.far,
            f64::from(config.world_min.x),
            f64::from(config.world_min.y),
            f64::from(config.world_min.z),
            f64::from(config.world_max.x),
            f64::from(config.world_max.y),
            f64::from(config.world_max.z),
            f64::from(config.camera_max_motion),
        ],
        light_capacity: config.light_capacity,
        light_kinds: config.light_kinds.clone(),
        environment_min: config.environment.min,
        environment_max: config.environment.max,
        exposure: [config.exposure.min, config.exposure.max],
        post_id: config.tone_curve.clone(),
        ao_version: u32::from(config.ao_enabled),
        probe_version: u32::from(config.probes_enabled),
        output_mode: format!(
            "{}:{}x{}:display{}",
            config.profile, config.width, config.height, config.display_index
        ),
        deterministic_frame_phase: [config.refresh_hz, config.shade_hz],
    })
}

fn scalar_type(ty: &Type, component: Option<u8>) -> Result<ScalarType, String> {
    if component.is_some() {
        return Ok(ScalarType::F32);
    }
    Ok(match ty {
        Type::F32 => ScalarType::F32,
        Type::F64 => ScalarType::F64,
        Type::U8 => ScalarType::U8,
        Type::U16 => ScalarType::U16,
        Type::U32 => ScalarType::U32,
        Type::U64 => ScalarType::U64,
        Type::Usize => ScalarType::Usize,
        Type::I8 => ScalarType::I8,
        Type::I16 => ScalarType::I16,
        Type::I32 => ScalarType::I32,
        Type::I64 => ScalarType::I64,
        Type::Isize => ScalarType::Isize,
        other => {
            return Err(format!(
                "pixels::params: parameter leaf has unsupported packed type `{}`",
                crate::sema::types::render_type(other)
            ));
        }
    })
}

pub(crate) fn scalar_children(op: &ScalarOp) -> Vec<ScalarId> {
    match op {
        ScalarOp::ConstF32(_)
        | ScalarOp::ConstF64(_)
        | ScalarOp::CoordX
        | ScalarOp::CoordY
        | ScalarOp::CoordZ
        | ScalarOp::SurfacePosition(_)
        | ScalarOp::SurfaceNormal(_)
        | ScalarOp::Param(_) => Vec::new(),
        ScalarOp::Neg(value)
        | ScalarOp::Abs(value)
        | ScalarOp::Sqrt(value, _)
        | ScalarOp::Rsqrt(value, _)
        | ScalarOp::SinRestricted(value, _)
        | ScalarOp::CosRestricted(value, _)
        | ScalarOp::MaterialRoughness { value, .. } => vec![*value],
        ScalarOp::Add(a, b)
        | ScalarOp::Sub(a, b)
        | ScalarOp::Mul(a, b)
        | ScalarOp::Div(a, b)
        | ScalarOp::Min(a, b)
        | ScalarOp::Max(a, b)
        | ScalarOp::Compare { a, b, .. }
        | ScalarOp::FiniteOr {
            value: a,
            fallback: b,
            ..
        } => vec![*a, *b],
        ScalarOp::Clamp { value, lo, hi } => vec![*value, *lo, *hi],
        ScalarOp::Dot3(a, b) => a.iter().chain(b).copied().collect(),
        ScalarOp::Cross3Component { a, b, .. } => a.iter().chain(b).copied().collect(),
        ScalarOp::Length2(values) => values.to_vec(),
        ScalarOp::Length3(values) | ScalarOp::Normalize3Component { value: values, .. } => {
            values.to_vec()
        }
        ScalarOp::Select { predicate, a, b } => vec![*predicate, *a, *b],
        ScalarOp::SelectIndex { index, options } => std::iter::once(*index)
            .chain(options.iter().copied())
            .collect(),
        ScalarOp::SmoothMin { a, b, k, .. } => vec![*a, *b, *k],
    }
}

fn mark_scalar_use(
    graph: &SymbolicGraph,
    id: ScalarId,
    use_kind: ParamUse,
    uses: &mut BTreeMap<ParamId, BTreeSet<ParamUse>>,
    seen: &mut BTreeSet<(ScalarId, ParamUse)>,
) -> Result<(), String> {
    if !seen.insert((id, use_kind)) {
        return Ok(());
    }
    let node = graph.scalar.get(id)?;
    if let ScalarOp::Param(param) = node.op {
        uses.entry(param).or_default().insert(use_kind);
    }
    for child in scalar_children(&node.op) {
        mark_scalar_use(graph, child, use_kind, uses, seen)?;
    }
    Ok(())
}

fn mark_material_use(
    graph: &SymbolicGraph,
    id: MaterialId,
    uses: &mut BTreeMap<ParamId, BTreeSet<ParamUse>>,
    seen_material: &mut BTreeSet<MaterialId>,
    seen_scalar: &mut BTreeSet<(ScalarId, ParamUse)>,
) -> Result<(), String> {
    if !seen_material.insert(id) {
        return Ok(());
    }
    match &graph.materials.get(id)?.kind {
        MaterialKind::Sample(sample) => {
            let mut scalars = Vec::new();
            scalars.extend(sample.base_color);
            scalars.push(sample.opacity);
            scalars.extend(sample.emissive);
            scalars.extend([
                sample.roughness,
                sample.metallic,
                sample.specular_level,
                sample.ior,
            ]);
            if let super::material_graph::NormalModel::AnalyticSlope { x, y } = sample.normal {
                scalars.extend([x, y]);
            }
            for scalar in scalars {
                mark_scalar_use(graph, scalar, ParamUse::Material, uses, seen_scalar)?;
            }
        }
        MaterialKind::Select { predicate, a, b } => {
            mark_scalar_use(graph, *predicate, ParamUse::Material, uses, seen_scalar)?;
            mark_material_use(graph, *a, uses, seen_material, seen_scalar)?;
            mark_material_use(graph, *b, uses, seen_material, seen_scalar)?;
        }
        MaterialKind::IdentityTable { cases, .. } => {
            for (_, material) in cases {
                mark_material_use(graph, *material, uses, seen_material, seen_scalar)?;
            }
        }
    }
    Ok(())
}

fn align_up(value: u32, alignment: u32) -> Result<u32, String> {
    let mask = alignment
        .checked_sub(1)
        .ok_or_else(|| "pixels::params: zero alignment".to_string())?;
    value
        .checked_add(mask)
        .map(|value| value & !mask)
        .ok_or_else(|| "P015: packed parameter offset overflow".to_string())
}

pub fn derive_layout(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
) -> Result<ParameterLayout, String> {
    let mut uses = BTreeMap::<ParamId, BTreeSet<ParamUse>>::new();
    let mut seen_scalar = BTreeSet::new();
    mark_scalar_use(
        graph,
        graph.fields.get(graph.field_root)?.scalar_value,
        ParamUse::Geometry,
        &mut uses,
        &mut seen_scalar,
    )?;
    mark_material_use(
        graph,
        graph.material_root,
        &mut uses,
        &mut BTreeSet::new(),
        &mut seen_scalar,
    )?;

    let mut referenced = graph
        .params
        .iter()
        .filter(|param| uses.contains_key(&param.id))
        .collect::<Vec<_>>();
    referenced.sort_by_key(|param| (param.path.clone(), param.component, param.id));

    let mut offset = 0_u32;
    let mut slots = Vec::with_capacity(referenced.len());
    for param in referenced {
        let ty = scalar_type(&param.ty, param.component)?;
        offset = align_up(offset, ty.size())?;
        let packed_offset = offset;
        offset = offset
            .checked_add(ty.size())
            .ok_or_else(|| "P015: packed parameter byte count overflow".to_string())?;
        let rate = param.rate.map(|(max_delta, max_second_delta)| ScalarRate {
            max_delta,
            max_second_delta,
        });
        slots.push(ParamSlot {
            id: param.id,
            path: param.path.clone(),
            component: param.component,
            scalar_ty: ty,
            range: ScalarRange {
                min: param.range_min,
                max: param.range_max,
            },
            rate,
            immutable: rate
                .is_some_and(|rate| rate.max_delta == 0.0 && rate.max_second_delta == 0.0),
            uses: uses.remove(&param.id).unwrap_or_default(),
            packed_offset,
        });
    }
    let snapshot_alignment = slots
        .iter()
        .map(|slot| slot.scalar_ty.size())
        .max()
        .unwrap_or(1);
    let packed_bytes = align_up(offset, snapshot_alignment)?;
    let fields = vec![
        "packed-parameter-bytes",
        "camera-coefficients",
        "light-coefficients",
        "exposure-post-ids",
        "ao-version",
        "probe-version",
        "output-mode",
        "deterministic-frame-phase",
    ];
    let frame_dependencies = frame_dependency_tuple(config)?;
    let mut schema = fields.join("\n");
    for slot in &slots {
        schema.push_str("\nslot path=");
        schema.push_str(
            &slot
                .path
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("."),
        );
        schema.push_str(&format!(
            " component={:?} type={:?} offset={} size={} uses=",
            slot.component,
            slot.scalar_ty,
            slot.packed_offset,
            slot.scalar_ty.size(),
        ));
        schema.push_str(
            &slot
                .uses
                .iter()
                .map(|use_kind| format!("{use_kind:?}"))
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    for field in &frame_dependencies.fields {
        schema.push_str(&format!(
            "\nframe path={} use={:?} type={:?} count={} offset={} runtime={}",
            field.path,
            field.use_kind,
            field.scalar_ty,
            field.element_count,
            field.packed_offset,
            field.runtime,
        ));
    }
    let schema_digest = wrela_machine::sha256::sha256_hex(schema.as_bytes());
    Ok(ParameterLayout {
        slots,
        packed_bytes,
        frame_dependencies,
        digest_schema: DependencyDigestSchema {
            fields,
            schema_digest,
        },
    })
}

#[derive(Debug, Clone, PartialEq)]
struct Origin {
    path: Vec<usize>,
    /// Location of this source leaf inside a newly constructed aggregate.
    /// Projections consume components from the front without changing the
    /// canonical source `path`.
    projection: Vec<usize>,
    ty: Type,
    module: String,
    unmodified: bool,
    contracts: crate::sema::attrs::FieldContracts,
    exact_comptime_integer: Option<i128>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OriginKey {
    path: Vec<usize>,
    projection: Vec<usize>,
    ty: String,
    module: String,
    unmodified: bool,
    range: Option<(u64, u64, bool, Option<(i128, i128)>)>,
    rate: Option<(u64, u64)>,
    exact_comptime_integer: Option<i128>,
}

impl Origin {
    fn key(&self) -> OriginKey {
        OriginKey {
            path: self.path.clone(),
            projection: self.projection.clone(),
            ty: crate::sema::types::render_type(&self.ty),
            module: self.module.clone(),
            unmodified: self.unmodified,
            range: self.contracts.range.map(|range| {
                (
                    range.min.to_bits(),
                    range.max.to_bits(),
                    range.integer,
                    range.exact_integer,
                )
            }),
            rate: self
                .contracts
                .rate
                .map(|rate| (rate.max_delta.to_bits(), rate.max_second_delta.to_bits())),
            exact_comptime_integer: self.exact_comptime_integer,
        }
    }
}

struct Collector<'a> {
    programs: &'a BTreeMap<String, TypedProgram>,
    active: BTreeSet<String>,
    found: BTreeMap<Vec<usize>, ParameterContract>,
}

fn is_optional_integer_range(ty: &Type) -> bool {
    matches!(
        ty,
        Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::Isize
    )
}

fn requires_parameter_range(ty: &Type) -> bool {
    matches!(ty, Type::F32 | Type::F64)
}

impl<'a> Collector<'a> {
    fn is_field_nominal(&self, origin: &Origin, declarations: &[&str]) -> bool {
        let Type::Named(name, args) = &origin.ty else {
            return false;
        };
        args.is_empty()
            && super::program_for_decl_module(self.programs, &origin.module).is_some_and(
                |program| {
                    super::nominal_decl(program, name).is_some_and(|(module, declaration)| {
                        matches!(module, "field" | "core.field")
                            && declarations.contains(&declaration)
                    })
                },
            )
    }

    fn requires_parameter_range(&self, origin: &Origin) -> bool {
        requires_parameter_range(&origin.ty)
            || self.is_field_nominal(origin, &["Vec2", "Vec3", "Vec4", "Rgb"])
    }

    fn merge_origins(groups: impl IntoIterator<Item = Vec<Origin>>) -> Vec<Origin> {
        let mut merged = Vec::new();
        let mut keys = BTreeSet::new();
        for group in groups {
            for origin in group {
                if keys.insert(origin.key()) {
                    merged.push(origin);
                }
            }
        }
        merged
    }

    fn record_origins(&mut self, origins: &[Origin]) -> Result<(), String> {
        for origin in origins {
            self.record(origin)?;
        }
        Ok(())
    }

    fn merge_envs(
        base: &BTreeMap<String, Vec<Origin>>,
        branches: impl IntoIterator<Item = BTreeMap<String, Vec<Origin>>>,
    ) -> BTreeMap<String, Vec<Origin>> {
        let mut merged = base.clone();
        let mut keys = base
            .iter()
            .map(|(name, origins)| {
                (
                    name.clone(),
                    origins.iter().map(Origin::key).collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for branch in branches {
            for (name, origins) in branch {
                let entry_keys = keys.entry(name.clone()).or_default();
                let entry = merged.entry(name).or_default();
                for origin in origins {
                    if entry_keys.insert(origin.key()) {
                        entry.push(origin);
                    }
                }
            }
        }
        merged
    }

    fn find_struct<'b>(&'b self, origin: &Origin) -> Option<(&'b TypedStruct, String)> {
        let Type::Named(name, args) = &origin.ty else {
            return None;
        };
        let context = super::program_for_decl_module(self.programs, &origin.module)?;
        let (declaring_module, declaring_name) = super::nominal_decl(context, name)?;
        let declaration = super::program_for_decl_module(self.programs, declaring_module)?;
        if args.is_empty() {
            return Some((
                declaration.structs.get(declaring_name)?,
                declaring_module.to_string(),
            ));
        }
        super::instantiated_struct(context, name, args)
            .map(|strukt| (strukt, origin.module.clone()))
            .or_else(|| {
                super::instantiated_struct(declaration, declaring_name, args)
                    .map(|strukt| (strukt, declaring_module.to_string()))
            })
    }

    fn field_origin(&self, base: &Origin, field: &str) -> Option<Origin> {
        if self.is_field_nominal(base, &["Vec2", "Vec3", "Vec4", "Rgb"]) {
            return Some(base.clone());
        }
        let (strukt, declaring_module) = self.find_struct(base)?;
        let index = strukt.fields.iter().position(|name| name == field)?;
        let ty = strukt.field_types.get(field)?.clone();
        let contracts = strukt
            .field_contracts
            .get([index].as_slice())
            .cloned()
            .unwrap_or_default();
        let mut path = base.path.clone();
        path.push(index);
        Some(Origin {
            path,
            projection: Vec::new(),
            ty,
            module: declaring_module,
            unmodified: base.unmodified,
            contracts,
            exact_comptime_integer: None,
        })
    }

    fn field_index(&self, module: &str, ty: &Type, field: &str) -> Option<usize> {
        let aggregate = Origin {
            path: Vec::new(),
            projection: Vec::new(),
            ty: ty.clone(),
            module: module.to_string(),
            unmodified: true,
            contracts: Default::default(),
            exact_comptime_integer: None,
        };
        let (strukt, _) = self.find_struct(&aggregate)?;
        strukt.fields.iter().position(|name| name == field)
    }

    fn projected_origin(mut origin: Origin, index: usize) -> Option<Origin> {
        if origin.projection.first().copied()? != index {
            return None;
        }
        origin.projection.remove(0);
        Some(origin)
    }

    fn nest_origin(mut origin: Origin, index: usize) -> Origin {
        origin.projection.insert(0, index);
        origin
    }

    fn record(&mut self, origin: &Origin) -> Result<(), String> {
        if origin.exact_comptime_integer.is_some() {
            return Ok(());
        }
        let required = self.requires_parameter_range(origin);
        if !required && !is_optional_integer_range(&origin.ty) {
            return Ok(());
        }
        let Some(range) = origin.contracts.range else {
            if !required {
                return Ok(());
            }
            return Err(format!(
                "P005: parameter path {:?} influences rendering but has no `@range` (type `{}`)",
                origin.path,
                crate::sema::types::render_type(&origin.ty)
            ));
        };
        self.found
            .entry(origin.path.clone())
            .or_insert_with(|| ParameterContract {
                path: origin.path.clone(),
                ty: origin.ty.clone(),
                range,
                rate: origin.contracts.rate,
            });
        Ok(())
    }

    fn visit_args(
        &mut self,
        args: &[TypedCallArg],
        module: &str,
        env: &mut BTreeMap<String, Vec<Origin>>,
    ) -> Result<Vec<Vec<Origin>>, String> {
        args.iter()
            .map(|arg| match &arg.value {
                Some(value) => self.visit_expr(value, module, env),
                None => Ok(Vec::new()),
            })
            .collect()
    }

    fn traced_integer(
        &self,
        expr: &TypedExpr,
        module: &str,
        env: &BTreeMap<String, Vec<Origin>>,
    ) -> Option<i128> {
        if let Some(value) = super::legality::constant_integer(self.programs, module, expr) {
            return Some(value);
        }
        match &expr.kind {
            TypedExprKind::Local(name) => {
                let origins = env.get(name)?;
                (origins.len() == 1).then(|| origins[0].exact_comptime_integer)?
            }
            TypedExprKind::ToScalar(inner) => self.traced_integer(inner, module, env),
            TypedExprKind::Neg(inner) => self.traced_integer(inner, module, env)?.checked_neg(),
            TypedExprKind::Binary(operator, left, right) => {
                let left = self.traced_integer(left, module, env)?;
                let right = self.traced_integer(right, module, env)?;
                use crate::syntax::ast::BinOp;
                match operator {
                    BinOp::Add => left.checked_add(right),
                    BinOp::Sub => left.checked_sub(right),
                    BinOp::Mul => left.checked_mul(right),
                    BinOp::Div => left.checked_div(right),
                    BinOp::Rem => left.checked_rem(right),
                    BinOp::BitAnd => Some(left & right),
                    BinOp::BitOr => Some(left | right),
                    BinOp::BitXor => Some(left ^ right),
                    BinOp::Shl => u32::try_from(right)
                        .ok()
                        .and_then(|shift| left.checked_shl(shift)),
                    BinOp::Shr => u32::try_from(right)
                        .ok()
                        .and_then(|shift| left.checked_shr(shift)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn visit_expr(
        &mut self,
        expr: &TypedExpr,
        module: &str,
        env: &mut BTreeMap<String, Vec<Origin>>,
    ) -> Result<Vec<Origin>, String> {
        match &expr.kind {
            TypedExprKind::Local(name) => Ok(env.get(name).cloned().unwrap_or_default()),
            TypedExprKind::Field(base, field) => {
                let mut origins = Vec::new();
                let field_index = self.field_index(module, &base.ty, field);
                for base_origin in self.visit_expr(base, module, env)? {
                    let selected = if base_origin.projection.is_empty() {
                        self.field_origin(&base_origin, field)
                    } else {
                        field_index.and_then(|index| Self::projected_origin(base_origin, index))
                    };
                    if let Some(origin) = selected {
                        if !origins.contains(&origin) {
                            origins.push(origin);
                        }
                    }
                }
                Ok(origins)
            }
            TypedExprKind::Index(base, index) => {
                let bases = self.visit_expr(base, module, env)?;
                let index_origins = self.visit_expr(index, module, env)?;
                let constant_index =
                    super::legality::constant_integer(self.programs, module, index)
                        .or_else(|| self.traced_integer(index, module, env))
                        .and_then(|index| usize::try_from(index).ok());
                let extent = match &base.ty {
                    Type::Array(_, length) => crate::sema::bodies::literal_array_len(length)
                        .and_then(|length| usize::try_from(length).ok()),
                    _ => None,
                };
                if constant_index.is_none() {
                    let Some(extent) = extent else {
                        return Err(
                            "P005: renderer dynamic index has no comptime array extent".to_string()
                        );
                    };
                    if index_origins.is_empty() {
                        return Err(
                            "P005: renderer dynamic array index must be a direct parameter with an \
                             exact in-bounds integer `@range`"
                                .to_string(),
                        );
                    }
                    for origin in &index_origins {
                        let exact_range =
                            origin.contracts.range.and_then(|range| range.exact_integer);
                        let in_bounds = origin.unmodified
                            && exact_range.is_some_and(|(min, max)| {
                                min >= 0
                                    && usize::try_from(max).is_ok_and(|maximum| maximum < extent)
                            });
                        if !in_bounds {
                            return Err(format!(
                                "P005: dynamic array index parameter path {:?} must have an exact \
                                 integer `@range` wholly within [0, {})",
                                origin.path, extent
                            ));
                        }
                    }
                    self.record_origins(&index_origins)?;
                }
                let mut origins = Vec::new();
                for origin in bases {
                    if !origin.projection.is_empty() {
                        let indices: Vec<usize> = match constant_index {
                            Some(index) => vec![index],
                            None => origin.projection.first().copied().into_iter().collect(),
                        };
                        for index in indices {
                            if let Some(indexed) = Self::projected_origin(origin.clone(), index)
                                && !origins.contains(&indexed)
                            {
                                origins.push(indexed);
                            }
                        }
                        continue;
                    }
                    let Type::Array(element, length) = &origin.ty else {
                        continue;
                    };
                    let length = crate::sema::bodies::literal_array_len(length)
                        .and_then(|length| usize::try_from(length).ok())
                        .ok_or_else(|| {
                            "P005: renderer parameter array index cannot be resolved \
                             conservatively because its extent is not a nonnegative comptime \
                             integer"
                                .to_string()
                        })?;
                    let indices = match constant_index {
                        Some(index) if index < length => index..index + 1,
                        Some(index) => {
                            return Err(format!(
                                "P005: renderer parameter array index {index} is outside its \
                                 comptime extent {length}"
                            ));
                        }
                        None => 0..length,
                    };
                    for index in indices {
                        let mut indexed = origin.clone();
                        indexed.path.push(index);
                        indexed.ty = (**element).clone();
                        indexed.contracts = Default::default();
                        if !origins.contains(&indexed) {
                            origins.push(indexed);
                        }
                    }
                }
                Ok(origins)
            }
            TypedExprKind::Call {
                callee,
                receiver,
                args,
            } => {
                let receiver_origins = match receiver {
                    Some(receiver) => self.visit_expr(receiver, module, env)?,
                    None => Vec::new(),
                };
                let origins = self.visit_args(args, module, env)?;
                let spelling = callee.spelling();
                let name = call_base(&spelling);
                let positive_contract = |index: usize,
                                         label: &str,
                                         operation: &str|
                 -> Result<(), String> {
                    let Some(argument) =
                        args.get(index).and_then(|argument| argument.value.as_ref())
                    else {
                        return Ok(());
                    };
                    if super::legality::constant_number(self.programs, module, argument).is_some() {
                        return Ok(());
                    }
                    let Some(argument_origins) =
                        origins.get(index).filter(|origins| !origins.is_empty())
                    else {
                        return Err(format!(
                            "P004: `{operation}` `{label}` must be a positive comptime constant or a \
                             direct renderer parameter with a strictly positive `@range`"
                        ));
                    };
                    for origin in argument_origins {
                        if !origin.unmodified {
                            return Err(format!(
                                "P004: `{operation}` `{label}` must be a positive comptime constant or \
                                 direct renderer parameter with a strictly positive `@range`; \
                                 transformed parameter expressions are not admitted"
                            ));
                        }
                        let Some(range) = origin.contracts.range else {
                            return Err(format!(
                                "P005: parameter path {:?} influences rendering but has no `@range`",
                                origin.path
                            ));
                        };
                        if range.min <= 0.0 {
                            let code = if matches!(
                                operation,
                                "smooth_union" | "smooth_intersection" | "smooth_subtract"
                            ) {
                                "P011"
                            } else {
                                "P004"
                            };
                            return Err(format!(
                                "{code}: `{operation}` `{label}` range minimum {} must be strictly positive",
                                range.min
                            ));
                        }
                    }
                    Ok(())
                };
                let resolved = called_function(self.programs, module, &spelling);
                let canonical_name = resolved
                    .as_ref()
                    .filter(|located| super::is_core_field_function(located))
                    .map(|located| located.decl_name.as_str())
                    .unwrap_or(name);
                if matches!(
                    canonical_name,
                    "smooth_union" | "smooth_intersection" | "smooth_subtract"
                ) {
                    positive_contract(2, "k", canonical_name)?;
                }
                if canonical_name == "uniform_scale" {
                    positive_contract(1, "scale", canonical_name)?;
                }
                if resolved.as_ref().is_some_and(super::is_core_field_function) {
                    self.record_origins(&receiver_origins)?;
                    for argument_origins in &origins {
                        self.record_origins(argument_origins)?;
                    }
                    return Ok(Vec::new());
                }
                if super::is_core_material_constructor(self.programs, module, &spelling) {
                    self.record_origins(&receiver_origins)?;
                    for argument_origins in &origins {
                        self.record_origins(argument_origins)?;
                    }
                    return Ok(Vec::new());
                }
                if resolved
                    .as_ref()
                    .is_some_and(super::is_core_scalar_function)
                {
                    let mut origins =
                        Self::merge_origins(std::iter::once(receiver_origins).chain(origins));
                    for origin in &mut origins {
                        origin.unmodified = false;
                    }
                    return Ok(origins);
                }
                if resolved
                    .as_ref()
                    .is_some_and(super::is_core_field_vector_method)
                {
                    let mut origins =
                        Self::merge_origins(std::iter::once(receiver_origins).chain(origins));
                    for origin in &mut origins {
                        origin.unmodified = false;
                    }
                    return Ok(origins);
                }
                if let Some(function) = resolved {
                    let mut call_env: BTreeMap<String, Vec<Origin>> = function
                        .function
                        .params
                        .iter()
                        .zip(origins)
                        .map(|(param, origin)| (param.name.clone(), origin))
                        .collect();
                    if function.function.receiver.is_some() {
                        call_env.insert("self".to_string(), receiver_origins);
                    }
                    return self.visit_function(function, call_env);
                }
                Ok(Vec::new())
            }
            TypedExprKind::CallValue(callee, args) => {
                self.visit_expr(callee, module, env)?;
                self.visit_args(args, module, env)?;
                Ok(Vec::new())
            }
            TypedExprKind::Take(value)
            | TypedExprKind::Try(value, _)
            | TypedExprKind::Panic(value)
            | TypedExprKind::Await(value)
            | TypedExprKind::Send(value) => self.visit_expr(value, module, env),
            TypedExprKind::ToScalar(value)
            | TypedExprKind::Neg(value)
            | TypedExprKind::BitNot(value)
            | TypedExprKind::Not(value) => {
                let mut origins = self.visit_expr(value, module, env)?;
                for origin in &mut origins {
                    origin.unmodified = false;
                }
                Ok(origins)
            }
            TypedExprKind::Binary(_, left, right)
            | TypedExprKind::OpCall(_, left, right)
            | TypedExprKind::And(left, right)
            | TypedExprKind::Or(left, right) => {
                let left = self.visit_expr(left, module, env)?;
                let right = self.visit_expr(right, module, env)?;
                let mut origins = Self::merge_origins([left, right]);
                for origin in &mut origins {
                    origin.unmodified = false;
                }
                Ok(origins)
            }
            TypedExprKind::Is(value, _) => self.visit_expr(value, module, env),
            TypedExprKind::EnumConstruct { args, .. } => {
                let origins = self.visit_args(args, module, env)?;
                Ok(Self::merge_origins(origins))
            }
            TypedExprKind::Closure { body, .. } => {
                match body {
                    TypedClosureBody::Expr(value) => {
                        self.visit_expr(value, module, env)?;
                    }
                    TypedClosureBody::Suite(body) => {
                        self.visit_stmts(body, module, env.clone())?;
                    }
                }
                Ok(Vec::new())
            }
            TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
                let mut origins = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    origins.push(
                        self.visit_expr(item, module, env)?
                            .into_iter()
                            .map(|origin| Self::nest_origin(origin, index))
                            .collect(),
                    );
                }
                Ok(Self::merge_origins(origins))
            }
            TypedExprKind::StructLiteral { fields, .. } => {
                let mut origins = Vec::new();
                for (field, value) in fields {
                    let Some(index) = self.field_index(module, &expr.ty, field) else {
                        return Err(format!(
                            "P005: cannot resolve aggregate field `{field}` while tracing \
                             renderer parameters"
                        ));
                    };
                    origins.push(
                        self.visit_expr(value, module, env)?
                            .into_iter()
                            .map(|origin| Self::nest_origin(origin, index))
                            .collect(),
                    );
                }
                Ok(Self::merge_origins(origins))
            }
            TypedExprKind::Intrinsic { receiver, args, .. } => {
                let mut origins = Vec::new();
                if let Some(receiver) = receiver {
                    origins.push(self.visit_expr(receiver, module, env)?);
                }
                for (_, value) in args {
                    origins.push(self.visit_expr(value, module, env)?);
                }
                Ok(Self::merge_origins(origins))
            }
            TypedExprKind::Int(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::Str(_)
            | TypedExprKind::BStr(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Unit
            | TypedExprKind::Const(_)
            | TypedExprKind::Static(_)
            | TypedExprKind::FnRef(_)
            | TypedExprKind::PoolName(_)
            | TypedExprKind::GroupChild(_) => Ok(Vec::new()),
        }
    }

    fn visit_function(
        &mut self,
        located: LocatedFn,
        env: BTreeMap<String, Vec<Origin>>,
    ) -> Result<Vec<Origin>, String> {
        let identity = format!("{}::{}", located.module, located.key);
        if !self.active.insert(identity.clone()) {
            return Ok(Vec::new());
        }
        let result = self
            .visit_stmts(&located.function.body, &located.module, env)
            .map(|(returns, _)| returns);
        self.active.remove(&identity);
        result
    }

    fn visit_stmts(
        &mut self,
        stmts: &[TypedStmt],
        module: &str,
        mut env: BTreeMap<String, Vec<Origin>>,
    ) -> Result<(Vec<Origin>, BTreeMap<String, Vec<Origin>>), String> {
        let mut returns = Vec::new();
        for stmt in stmts {
            match &stmt.kind {
                TypedStmtKind::Let { name, value, .. } => {
                    let origin = self.visit_expr(value, module, &mut env)?;
                    env.insert(name.clone(), origin);
                }
                TypedStmtKind::Assign { target, value } => {
                    let origin = self.visit_expr(value, module, &mut env)?;
                    if let TypedExprKind::Local(name) = &target.kind {
                        env.insert(name.clone(), origin);
                    } else {
                        self.visit_expr(target, module, &mut env)?;
                    }
                }
                TypedStmtKind::If {
                    cond,
                    then_branch,
                    elifs,
                    else_branch,
                } => {
                    let cond_origins = self.visit_expr(cond, module, &mut env)?;
                    self.record_origins(&cond_origins)?;
                    let mut branch_envs = Vec::new();
                    let (branch_returns, branch_env) =
                        self.visit_stmts(then_branch, module, env.clone())?;
                    returns.extend(branch_returns);
                    branch_envs.push(branch_env);
                    for elif in elifs {
                        let cond_origins = self.visit_expr(&elif.cond, module, &mut env)?;
                        self.record_origins(&cond_origins)?;
                        let (branch_returns, branch_env) =
                            self.visit_stmts(&elif.body, module, env.clone())?;
                        returns.extend(branch_returns);
                        branch_envs.push(branch_env);
                    }
                    if let Some(body) = else_branch {
                        let (branch_returns, branch_env) =
                            self.visit_stmts(body, module, env.clone())?;
                        returns.extend(branch_returns);
                        branch_envs.push(branch_env);
                    }
                    env = Self::merge_envs(&env, branch_envs);
                }
                TypedStmtKind::Match { scrutinee, arms } => {
                    let scrutinee_origins = self.visit_expr(scrutinee, module, &mut env)?;
                    self.record_origins(&scrutinee_origins)?;
                    let mut branch_envs = Vec::new();
                    for arm in arms {
                        if let Some(guard) = &arm.guard {
                            let guard_origins = self.visit_expr(guard, module, &mut env)?;
                            self.record_origins(&guard_origins)?;
                        }
                        let (branch_returns, branch_env) =
                            self.visit_stmts(&arm.body, module, env.clone())?;
                        returns.extend(branch_returns);
                        branch_envs.push(branch_env);
                    }
                    env = Self::merge_envs(&env, branch_envs);
                }
                TypedStmtKind::For {
                    name, iter, body, ..
                } => {
                    let mut loop_env = env.clone();
                    let iterations = match iter {
                        TypedForIter::Range(start, end, inclusive) => {
                            self.visit_expr(start, module, &mut env)?;
                            self.visit_expr(end, module, &mut env)?;
                            let start_value =
                                super::legality::constant_integer(self.programs, module, start)
                                    .ok_or_else(|| {
                                        "P024: field loop bound is not comptime-known".to_string()
                                    })?;
                            let end_value =
                                super::legality::constant_integer(self.programs, module, end)
                                    .ok_or_else(|| {
                                        "P024: field loop bound is not comptime-known".to_string()
                                    })?;
                            let exclusive_end = if *inclusive {
                                end_value.checked_add(1).ok_or_else(|| {
                                    "P024: field loop bound is out of range".to_string()
                                })?
                            } else {
                                end_value
                            };
                            (start_value..exclusive_end)
                                .map(|value| {
                                    vec![Origin {
                                        path: Vec::new(),
                                        projection: Vec::new(),
                                        ty: start.ty.clone(),
                                        module: module.to_string(),
                                        unmodified: true,
                                        contracts: Default::default(),
                                        exact_comptime_integer: Some(value),
                                    }]
                                })
                                .collect::<Vec<_>>()
                        }
                        TypedForIter::Expr(value) => {
                            let bases = self.visit_expr(value, module, &mut env)?;
                            let mut elements: Vec<Vec<Origin>> = Vec::new();
                            for base in bases {
                                if let Some(index) = base.projection.first().copied() {
                                    if let Some(origin) = Self::projected_origin(base, index) {
                                        elements.push(vec![origin]);
                                    }
                                    continue;
                                }
                                let Type::Array(element, length) = &base.ty else {
                                    continue;
                                };
                                let Some(length) = crate::sema::bodies::literal_array_len(length)
                                    .and_then(|length| usize::try_from(length).ok())
                                else {
                                    continue;
                                };
                                for index in 0..length {
                                    let mut origin = base.clone();
                                    origin.path.push(index);
                                    origin.ty = (**element).clone();
                                    origin.contracts = Default::default();
                                    elements.push(vec![origin]);
                                }
                            }
                            elements
                        }
                    };
                    for iteration in iterations {
                        loop_env.insert(name.clone(), iteration);
                        let (loop_returns, next_loop_env) =
                            self.visit_stmts(body, module, loop_env)?;
                        returns.extend(loop_returns);
                        loop_env = next_loop_env;
                    }
                    env = loop_env;
                }
                TypedStmtKind::While { cond, body, .. } => {
                    self.visit_expr(cond, module, &mut env)?;
                    let (loop_returns, final_loop_env) =
                        self.visit_stmts(body, module, env.clone())?;
                    returns.extend(loop_returns);
                    env = Self::merge_envs(&env, [final_loop_env]);
                }
                TypedStmtKind::Return(Some(value)) => {
                    for origin in self.visit_expr(value, module, &mut env)? {
                        if !returns.contains(&origin) {
                            returns.push(origin);
                        }
                    }
                }
                TypedStmtKind::ExprStmt(value) | TypedStmtKind::BareSend { expr: value, .. } => {
                    self.visit_expr(value, module, &mut env)?;
                }
                TypedStmtKind::Assert { cond, message }
                | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
                    self.visit_expr(cond, module, &mut env)?;
                    if let Some(message) = message {
                        self.visit_expr(message, module, &mut env)?;
                    }
                }
                TypedStmtKind::Defer(TypedDeferBody::Expr(value)) => {
                    self.visit_expr(value, module, &mut env)?;
                }
                TypedStmtKind::Defer(TypedDeferBody::Suite(body))
                | TypedStmtKind::WithGroup { body, .. } => {
                    let (body_returns, body_env) = self.visit_stmts(body, module, env.clone())?;
                    returns.extend(body_returns);
                    env = Self::merge_envs(&env, [body_env]);
                }
                TypedStmtKind::Break
                | TypedStmtKind::Continue
                | TypedStmtKind::Pass
                | TypedStmtKind::Return(None) => {}
            }
        }
        Ok((returns, env))
    }
}

fn root_env(function: &TypedFn, module: &str) -> BTreeMap<String, Vec<Origin>> {
    function
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let origins = if index == 1 {
                vec![Origin {
                    path: Vec::new(),
                    projection: Vec::new(),
                    ty: param.ty.clone(),
                    module: module.to_string(),
                    unmodified: true,
                    contracts: Default::default(),
                    exact_comptime_integer: None,
                }]
            } else {
                Vec::new()
            };
            (param.name.clone(), origins)
        })
        .collect()
}

pub fn collect_parameter_contracts(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    _params_type: &Type,
    field: &str,
    material: &str,
) -> Result<Vec<ParameterContract>, String> {
    let field = root_function(owner, programs, field)?;
    let material = root_function(owner, programs, material)?;
    let field_env = root_env(&field.function, &field.module);
    let material_env = root_env(&material.function, &material.module);
    let mut collector = Collector {
        programs,
        active: BTreeSet::new(),
        found: BTreeMap::new(),
    };
    collector.visit_function(field, field_env)?;
    collector.visit_function(material, material_env)?;
    Ok(collector.found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::config::{RgbRangeConfig, ScalarRangeConfig, Vec3Config};

    #[test]
    fn coefficient_snapshot_stride_preserves_maximum_slot_alignment() {
        assert_eq!(align_up(9, 8).unwrap(), 16);
        assert_eq!(align_up(12, 4).unwrap(), 12);
    }

    fn dependency_config(ao_enabled: bool) -> super::super::config::RendererConfig {
        super::super::config::RendererConfig {
            declaration_index: 0,
            worker_count: 1,
            params_type: Type::Unit,
            field: String::new(),
            material: String::new(),
            material_type: Type::Unit,
            display_index: 0,
            width: 1,
            height: 1,
            refresh_hz: 60,
            shade_hz: 60,
            profile: "AaaByteExact".to_string(),
            tone_curve: "Linear".to_string(),
            near: 0.1,
            far: 10.0,
            world_min: Vec3Config {
                x: -1.0,
                y: -1.0,
                z: -1.0,
            },
            world_max: Vec3Config {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
            camera_pose: None,
            camera_max_motion: 0.0,
            light_capacity: 0,
            light_kinds: Vec::new(),
            exposure: ScalarRangeConfig {
                min: -1.0,
                max: 1.0,
            },
            environment: RgbRangeConfig {
                min: [0.0; 3],
                max: [1.0; 3],
            },
            ao_enabled,
            probes_enabled: false,
            probe_initialization_worst_case_ms: 0,
            initialization_deadline_ms: 1,
            parameter_contracts: Vec::new(),
        }
    }

    #[test]
    fn integer_ranges_are_optional_but_retained_when_present() {
        for ty in [
            Type::U8,
            Type::U16,
            Type::U32,
            Type::U64,
            Type::Usize,
            Type::I8,
            Type::I16,
            Type::I32,
            Type::I64,
            Type::Isize,
        ] {
            assert!(is_optional_integer_range(&ty), "{ty:?}");
            assert!(!requires_parameter_range(&ty), "{ty:?}");
        }
        assert!(!is_optional_integer_range(&Type::Bool));
        assert!(requires_parameter_range(&Type::F32));
    }

    #[test]
    fn frame_dependency_tuple_seals_ao_mode() {
        let disabled = frame_dependency_tuple(&dependency_config(false)).unwrap();
        let enabled = frame_dependency_tuple(&dependency_config(true)).unwrap();
        assert_eq!(disabled.ao_version, 0);
        assert_eq!(enabled.ao_version, 1);
        assert!(enabled.fields.iter().any(|field| {
            field.path == "sealed.ao_version" && field.use_kind == ParamUse::Post && !field.runtime
        }));
        assert_ne!(disabled, enabled);
    }

    #[test]
    fn field_renames_change_source_digest_without_changing_path_order() {
        fn paths(fields: &[&str]) -> Vec<Vec<usize>> {
            let mut program = TypedProgram {
                module_path: "scene".to_string(),
                ..TypedProgram::default()
            };
            program
                .type_decl_modules
                .insert("Params".to_string(), "scene".to_string());
            program
                .type_decl_names
                .insert("Params".to_string(), "Params".to_string());
            let mut params = TypedStruct {
                name: "Params".to_string(),
                fields: fields.iter().map(|field| (*field).to_string()).collect(),
                ..TypedStruct::default()
            };
            for field in fields {
                params.field_types.insert((*field).to_string(), Type::F32);
            }
            program.structs.insert("Params".to_string(), params);
            let programs = BTreeMap::from([("scene".to_string(), program)]);
            let collector = Collector {
                programs: &programs,
                active: BTreeSet::new(),
                found: BTreeMap::new(),
            };
            let root = Origin {
                path: Vec::new(),
                projection: Vec::new(),
                ty: Type::Named("Params".to_string(), Vec::new()),
                module: "scene".to_string(),
                unmodified: true,
                contracts: Default::default(),
                exact_comptime_integer: None,
            };
            fields
                .iter()
                .map(|field| collector.field_origin(&root, field).unwrap().path)
                .collect()
        }

        let original = b"struct Params:\n    height: f32\n    tint: f32\n";
        let renamed = b"struct Params:\n    elevation: f32\n    tone: f32\n";
        assert_ne!(
            wrela_machine::sha256::sha256_hex(original),
            wrela_machine::sha256::sha256_hex(renamed)
        );
        assert_eq!(paths(&["height", "tint"]), paths(&["elevation", "tone"]));
    }
}
