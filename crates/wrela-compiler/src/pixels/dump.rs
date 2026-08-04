//! Stable textual dump boundaries for the Pixels compiler stages.

use super::PlaneSkeleton;
use super::config::{RendererConfig, RendererConfigs};
use super::graph::{FieldKind, Primitive, TransformProgram};
use super::material_graph::{MaterialKind, NormalModel};
use super::scalar::{ProofObligation, ScalarOp};
use super::symbolic::SymbolicGraph;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelsDumpStage {
    FieldGraph,
    FrameProgram,
    RenderLayout,
}

pub fn dump_zero_renderers(stage: PixelsDumpStage) -> String {
    let header = match stage {
        PixelsDumpStage::FieldGraph => "FieldGraph v1",
        PixelsDumpStage::FrameProgram => "FrameProgram v1",
        PixelsDumpStage::RenderLayout => "RenderLayout v1",
    };
    format!("{header}\nRenderers count=0\n")
}

fn stage_header(stage: PixelsDumpStage) -> &'static str {
    match stage {
        PixelsDumpStage::FieldGraph => "FieldGraph v1",
        PixelsDumpStage::FrameProgram => "FrameProgram v1",
        PixelsDumpStage::RenderLayout => "RenderLayout v1",
    }
}

fn dump_config(config: &RendererConfig, out: &mut String) {
    out.push_str(&format!(
        "  Renderer index={} params={} field={} material={} material_type={}\n",
        config.declaration_index,
        crate::sema::types::render_type(&config.params_type),
        config.field,
        config.material,
        crate::sema::types::render_type(&config.material_type),
    ));
    out.push_str("    Compilation status=not-run\n");
    out.push_str(&format!(
        "    Display ref=driver#{}\n    Mode width={} height={} refresh_hz={} shade_hz={}\n",
        config.display_index, config.width, config.height, config.refresh_hz, config.shade_hz,
    ));
    out.push_str(&format!(
        "    Profile value={} tone_curve={}\n",
        config.profile, config.tone_curve
    ));
    out.push_str(&format!(
        "    Depth near={} far={}\n    World min=[{},{},{}] max=[{},{},{}]\n",
        config.near,
        config.far,
        config.world_min.x,
        config.world_min.y,
        config.world_min.z,
        config.world_max.x,
        config.world_max.y,
        config.world_max.z,
    ));
    out.push_str(&format!(
        "    Contracts camera_max_motion={} light_capacity={} light_kinds=[{}] exposure=[{},{}] environment=[{},{},{}]-[{},{},{}] ao={} probes={} probe_initialization_worst_case_ms={} initialization_deadline_ms={}\n",
        config.camera_max_motion,
        config.light_capacity,
        config.light_kinds.join(","),
        config.exposure.min,
        config.exposure.max,
        config.environment.min[0],
        config.environment.min[1],
        config.environment.min[2],
        config.environment.max[0],
        config.environment.max[1],
        config.environment.max[2],
        config.ao_enabled,
        config.probes_enabled,
        config.probe_initialization_worst_case_ms,
        config.initialization_deadline_ms,
    ));
    for parameter in &config.parameter_contracts {
        let range = parameter
            .range
            .exact_integer
            .map(|(min, max)| format!("{min},{max}"))
            .unwrap_or_else(|| format!("{},{}", parameter.range.min, parameter.range.max));
        out.push_str(&format!(
            "    Parameter path={:?} type={} range=[{}]",
            parameter.path,
            crate::sema::types::render_type(&parameter.ty),
            range,
        ));
        if let Some(rate) = parameter.rate {
            out.push_str(&format!(
                " rate=[{},{}]",
                rate.max_delta, rate.max_second_delta
            ));
        } else {
            out.push_str(" rate=none");
        }
        out.push('\n');
    }
}

pub fn dump_uncompiled_configs(
    stage: PixelsDumpStage,
    configs: &RendererConfigs,
    renderer_index: Option<usize>,
) -> String {
    let mut out = format!(
        "{}\nRenderers count={}\n",
        stage_header(stage),
        configs.renderers.len()
    );
    if let Some(index) = renderer_index {
        if let Some(config) = configs.renderers.get(index) {
            dump_config(config, &mut out);
        }
    } else {
        for config in &configs.renderers {
            dump_config(config, &mut out);
        }
    }
    out
}

pub fn dump_symbolic_graphs(
    graphs: &[(usize, SymbolicGraph)],
    configs: &RendererConfigs,
) -> String {
    let mut out = format!(
        "FieldGraph v1\nRenderers count={}\n",
        configs.renderers.len()
    );
    for (index, graph) in graphs {
        let config = &configs.renderers[*index];
        out.push_str(&format!(
            "  Renderer index={} field={} material={} params={} material_type={}\n",
            graph.renderer_index,
            graph.field_key,
            graph.material_key,
            crate::sema::types::render_type(&graph.params_type),
            crate::sema::types::render_type(&graph.material_type),
        ));
        out.push_str(&format!(
            "    Config display=driver#{} mode={}x{}@{} shade_hz={} profile={} tone_curve={} depth=[{},{}]\n",
            config.display_index,
            config.width,
            config.height,
            config.refresh_hz,
            config.shade_hz,
            config.profile,
            config.tone_curve,
            format_f64(config.near),
            format_f64(config.far),
        ));
        out.push_str(&format!(
            "      World min=[{},{},{}] max=[{},{},{}]\n",
            format_f64(f64::from(config.world_min.x)),
            format_f64(f64::from(config.world_min.y)),
            format_f64(f64::from(config.world_min.z)),
            format_f64(f64::from(config.world_max.x)),
            format_f64(f64::from(config.world_max.y)),
            format_f64(f64::from(config.world_max.z)),
        ));
        out.push_str(&format!(
            "      RuntimeBounds camera_max_motion={} light_capacity={} light_kinds=[{}] exposure=[{},{}] environment=[{},{},{}]-[{},{},{}]\n",
            format_f64(f64::from(config.camera_max_motion)),
            config.light_capacity,
            config.light_kinds.join(","),
            format_f64(f64::from(config.exposure.min)),
            format_f64(f64::from(config.exposure.max)),
            format_f64(f64::from(config.environment.min[0])),
            format_f64(f64::from(config.environment.min[1])),
            format_f64(f64::from(config.environment.min[2])),
            format_f64(f64::from(config.environment.max[0])),
            format_f64(f64::from(config.environment.max[1])),
            format_f64(f64::from(config.environment.max[2])),
        ));
        out.push_str(&format!(
            "      Initialization ao={} probes={} probe_worst_case_ms={} deadline_ms={}\n",
            config.ao_enabled,
            config.probes_enabled,
            config.probe_initialization_worst_case_ms,
            config.initialization_deadline_ms,
        ));
        for param in &graph.params {
            let param_ty = if param.component.is_some() {
                crate::sema::types::Type::F32
            } else {
                param.ty.clone()
            };
            out.push_str(&format!(
                "    Param id={} path={} indexes={} component={} ty={} range=[{},{}]",
                param.id,
                param.spelling,
                format_path(&param.path),
                param
                    .component
                    .map(|component| component.to_string())
                    .unwrap_or_else(|| "scalar".to_string()),
                crate::sema::types::render_type(&param_ty),
                format_f64(param.range_min),
                format_f64(param.range_max),
            ));
            if let Some((min, max)) = param.exact_integer {
                out.push_str(&format!(" exact_integer=[{min},{max}]"));
            }
            if let Some((max_delta, max_second_delta)) = param.rate {
                out.push_str(&format!(
                    "\n      Rate max_delta={} max_second_delta={}",
                    format_f64(max_delta),
                    format_f64(max_second_delta)
                ));
            }
            out.push('\n');
        }
        for (id, node) in graph.scalar.iter() {
            out.push_str(&format!(
                "    Scalar id={} dependency={} {}\n",
                id,
                dependency_name(node.dependency),
                render_scalar(&node.op),
            ));
        }
        for (id, node) in graph.fields.iter() {
            out.push_str(&format!(
                "    Field id={} scalar={} {}\n",
                id,
                node.scalar_value,
                render_field(&node.kind),
            ));
        }
        for (id, node) in graph.materials.iter() {
            out.push_str(&format!(
                "    Material id={} {}\n",
                id,
                render_material(&node.kind),
            ));
        }
        let mut identities = std::collections::BTreeSet::new();
        for (_, node) in graph.fields.iter() {
            if let FieldKind::Mark {
                object_source,
                material_source,
                ..
            } = &node.kind
            {
                identities.insert((
                    "object",
                    object_source.enum_key.as_str(),
                    object_source.variant.as_str(),
                ));
                identities.insert((
                    "material",
                    material_source.enum_key.as_str(),
                    material_source.variant.as_str(),
                ));
            }
        }
        for (kind, enum_key, variant) in identities {
            out.push_str(&format!(
                "    Identity kind={kind} enum={enum_key} variant={variant}\n"
            ));
        }
        for obligation in &graph.obligations {
            match obligation {
                super::symbolic::PendingObligation::Scalar(obligation) => out.push_str(&format!(
                    "    Obligation kind=scalar {}\n",
                    render_obligation(obligation)
                )),
                super::symbolic::PendingObligation::MaterialEvent { predicate } => {
                    out.push_str(&format!(
                        "    Obligation kind=material-event predicate={predicate}\n"
                    ));
                }
            }
        }
        for (id, _) in graph.scalar.iter() {
            dump_origin(
                "scalar",
                &id.to_string(),
                graph.scalar.origin(id).expect("covered"),
                &mut out,
            );
        }
        for (id, _) in graph.fields.iter() {
            dump_origin(
                "field",
                &id.to_string(),
                graph.fields.origin(id).expect("covered"),
                &mut out,
            );
        }
        for (id, _) in graph.materials.iter() {
            dump_origin(
                "material",
                &id.to_string(),
                graph.materials.origin(id).expect("covered"),
                &mut out,
            );
        }
        out.push_str(&format!(
            "    Roots field={} material={}\n    SymbolicQuota steps={}/{} nodes={}/{} unrolled_statements={}/{} aggregate_elements={}/{} call_depth={}/{}\n",
            graph.field_root,
            graph.material_root,
            graph.quota.steps,
            graph.quota.max_steps,
            graph.quota.peak_nodes,
            graph.quota.max_nodes,
            graph.quota.unrolled_statements,
            graph.quota.max_unrolled_statements,
            graph.quota.aggregate_elements,
            graph.quota.max_aggregate_elements,
            graph.quota.peak_call_depth,
            graph.quota.max_call_depth,
        ));
    }
    out.push_str("Analysis status=pending\n");
    out
}

fn format_path(path: &[usize]) -> String {
    let body = path
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("[{body}]")
}

fn format_f64(value: f64) -> String {
    if value == 0.0 && value.is_sign_negative() {
        "-0.0".to_string()
    } else {
        value.to_string()
    }
}

fn format_f32_bits(bits: u32) -> String {
    let value = f32::from_bits(bits);
    if value == 0.0 && value.is_sign_negative() {
        "-0.0".to_string()
    } else {
        value.to_string()
    }
}

fn dependency_name(dependency: super::scalar::Dependency) -> &'static str {
    use super::scalar::Dependency;
    match dependency {
        Dependency::Constant => "constant",
        Dependency::Coordinate => "coordinate",
        Dependency::Parameter => "parameter",
        Dependency::Surface => "surface",
        Dependency::CoordinateAndParameter => "coordinate+parameter",
        Dependency::CoordinateAndSurface => "coordinate+surface",
        Dependency::ParameterAndSurface => "parameter+surface",
        Dependency::CoordinateParameterAndSurface => "coordinate+parameter+surface",
    }
}

fn scalar_ids(ids: &[super::ids::ScalarId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn render_scalar(op: &ScalarOp) -> String {
    match op {
        ScalarOp::ConstF32(bits) => format!(
            "kind=ConstF32 value={} bits=0x{bits:08x}",
            format_f32_bits(*bits)
        ),
        ScalarOp::ConstF64(bits) => format!(
            "kind=ConstF64 value={} bits=0x{bits:016x}",
            format_f64(f64::from_bits(*bits))
        ),
        ScalarOp::CoordX => "kind=Coord axis=x".to_string(),
        ScalarOp::CoordY => "kind=Coord axis=y".to_string(),
        ScalarOp::CoordZ => "kind=Coord axis=z".to_string(),
        ScalarOp::SurfacePosition(component) => {
            format!("kind=SurfacePosition component={component}")
        }
        ScalarOp::SurfaceNormal(component) => {
            format!("kind=SurfaceNormal component={component}")
        }
        ScalarOp::Param(param) => format!("kind=Param param={param}"),
        ScalarOp::Add(a, b) => format!("kind=Add a={a} b={b}"),
        ScalarOp::Sub(a, b) => format!("kind=Sub a={a} b={b}"),
        ScalarOp::Mul(a, b) => format!("kind=Mul a={a} b={b}"),
        ScalarOp::Div(a, b) => format!("kind=Div numerator={a} denominator={b}"),
        ScalarOp::Neg(value) => format!("kind=Neg value={value}"),
        ScalarOp::Abs(value) => format!("kind=Abs value={value}"),
        ScalarOp::Min(a, b) => format!("kind=Min a={a} b={b}"),
        ScalarOp::Max(a, b) => format!("kind=Max a={a} b={b}"),
        ScalarOp::Clamp { value, lo, hi } => {
            format!("kind=Clamp value={value} lo={lo} hi={hi}")
        }
        ScalarOp::Sqrt(value, semantic)
        | ScalarOp::Rsqrt(value, semantic)
        | ScalarOp::SinRestricted(value, semantic)
        | ScalarOp::CosRestricted(value, semantic) => {
            format!(
                "kind={} value={value} semantic={semantic:?}",
                match op {
                    ScalarOp::Sqrt(..) => "Sqrt",
                    ScalarOp::Rsqrt(..) => "Rsqrt",
                    ScalarOp::SinRestricted(..) => "SinRestricted",
                    _ => "CosRestricted",
                }
            )
        }
        ScalarOp::Dot3(a, b) => {
            format!("kind=Dot3 a=[{}] b=[{}]", scalar_ids(a), scalar_ids(b))
        }
        ScalarOp::Cross3Component { component, a, b } => format!(
            "kind=Cross3 component={component} a=[{}] b=[{}]",
            scalar_ids(a),
            scalar_ids(b)
        ),
        ScalarOp::Length2(value) => format!("kind=Length2 value=[{}]", scalar_ids(value)),
        ScalarOp::Length3(value) => format!("kind=Length3 value=[{}]", scalar_ids(value)),
        ScalarOp::Normalize3Component {
            component,
            value,
            semantic,
        } => format!(
            "kind=Normalize3 component={component} value=[{}] semantic={semantic:?}",
            scalar_ids(value)
        ),
        ScalarOp::Compare { op, a, b } => format!("kind=Compare op={op:?} a={a} b={b}"),
        ScalarOp::Select { predicate, a, b } => {
            format!("kind=Select predicate={predicate} a={a} b={b}")
        }
        ScalarOp::SelectIndex { index, options } => format!(
            "kind=SelectIndex index={index} options=[{}]",
            scalar_ids(options)
        ),
        ScalarOp::SmoothMin { a, b, k, semantic } => {
            format!("kind=SmoothMin a={a} b={b} k={k} semantic={semantic:?}")
        }
        ScalarOp::FiniteOr {
            value,
            fallback,
            semantic,
        } => format!("kind=FiniteOr value={value} fallback={fallback} semantic={semantic:?}"),
        ScalarOp::MaterialRoughness { value, semantic } => {
            format!("kind=MaterialRoughness value={value} semantic={semantic:?}")
        }
    }
}

fn render_primitive(primitive: &Primitive) -> String {
    match primitive {
        Primitive::Plane { normal, offset } => {
            format!("Plane normal=[{}] offset={offset}", scalar_ids(normal))
        }
        Primitive::Sphere { center, radius } => {
            format!("Sphere center=[{}] radius={radius}", scalar_ids(center))
        }
        Primitive::Box { center, half } => format!(
            "Box center=[{}] half=[{}]",
            scalar_ids(center),
            scalar_ids(half)
        ),
        Primitive::RoundBox {
            center,
            half,
            radius,
        } => format!(
            "RoundBox center=[{}] half=[{}] radius={radius}",
            scalar_ids(center),
            scalar_ids(half)
        ),
        Primitive::Capsule { a, b, radius } => format!(
            "Capsule a=[{}] b=[{}] radius={radius}",
            scalar_ids(a),
            scalar_ids(b)
        ),
        Primitive::FiniteCylinder { a, b, radius } => format!(
            "FiniteCylinder a=[{}] b=[{}] radius={radius}",
            scalar_ids(a),
            scalar_ids(b)
        ),
        Primitive::FiniteCone {
            a,
            b,
            radius_a,
            radius_b,
        } => format!(
            "FiniteCone a=[{}] b=[{}] radius_a={radius_a} radius_b={radius_b}",
            scalar_ids(a),
            scalar_ids(b)
        ),
        Primitive::Torus {
            center,
            axis,
            major,
            minor,
        } => format!(
            "Torus center=[{}] axis=[{}] major={major} minor={minor}",
            scalar_ids(center),
            scalar_ids(axis)
        ),
    }
}

fn render_transform(transform: &TransformProgram) -> String {
    match transform {
        TransformProgram::Translate { by } => format!("Translate by=[{}]", scalar_ids(by)),
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => format!(
            "Rotate rows=[{}];[{}];[{}]",
            scalar_ids(row_x),
            scalar_ids(row_y),
            scalar_ids(row_z)
        ),
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => format!(
            "Rigid translation=[{}] rows=[{}];[{}];[{}]",
            scalar_ids(translation),
            scalar_ids(row_x),
            scalar_ids(row_y),
            scalar_ids(row_z)
        ),
        TransformProgram::UniformScale { scale } => format!("UniformScale scale={scale}"),
        TransformProgram::SourceRigidSequence { steps, composed }
        | TransformProgram::RigidSequence { steps, composed } => format!(
            "{} steps=[{}] composed={}",
            if matches!(transform, TransformProgram::SourceRigidSequence { .. }) {
                "SourceRigidSequence"
            } else {
                "RigidSequence"
            },
            steps
                .iter()
                .map(render_transform)
                .collect::<Vec<_>>()
                .join(","),
            render_transform(composed),
        ),
    }
}

fn render_field(kind: &FieldKind) -> String {
    match kind {
        FieldKind::Primitive(primitive) => format!("kind={}", render_primitive(primitive)),
        FieldKind::HardUnion { a, b } => format!("kind=HardUnion a={a} b={b}"),
        FieldKind::HardIntersection { a, b } => {
            format!("kind=HardIntersection a={a} b={b}")
        }
        FieldKind::HardSubtract { a, b } => format!("kind=HardSubtract a={a} b={b}"),
        FieldKind::SmoothUnion { a, b, k } => format!("kind=SmoothUnion a={a} b={b} k={k}"),
        FieldKind::SmoothIntersection { a, b, k } => {
            format!("kind=SmoothIntersection a={a} b={b} k={k}")
        }
        FieldKind::SmoothSubtract { a, b, k } => {
            format!("kind=SmoothSubtract a={a} b={b} k={k}")
        }
        FieldKind::Neg { child } => format!("kind=Neg child={child}"),
        FieldKind::Transform { child, transform } => {
            format!(
                "kind=Transform child={child} transform={}",
                render_transform(transform)
            )
        }
        FieldKind::FiniteRepeat {
            child,
            axis,
            first,
            count,
            period,
        } => format!(
            "kind=FiniteRepeat child={child} axis={axis:?} first={first} count={count} period={period}"
        ),
        FieldKind::BoundedDisplace {
            base,
            displacement,
            contract,
        } => format!(
            "kind=BoundedDisplace base={base} displacement={displacement} derivation={:?} bounds=[{},{},{},{}]",
            contract.derivation,
            contract.amplitude_bound,
            contract.gradient_bound,
            contract.hessian_bound,
            contract.third_derivative_bound
        ),
        FieldKind::Mark {
            child,
            object_source,
            material_source,
        } => format!(
            "kind=Mark child={child} object={}::{} material={}::{}",
            object_source.enum_key,
            object_source.variant,
            material_source.enum_key,
            material_source.variant
        ),
    }
}

fn render_material(kind: &MaterialKind) -> String {
    match kind {
        MaterialKind::Sample(sample) => format!(
            "kind=Sample base_color=[{}] opacity={} emissive=[{}] roughness={} metallic={} specular={} ior={} normal={} pattern={}",
            scalar_ids(&sample.base_color),
            sample.opacity,
            scalar_ids(&sample.emissive),
            sample.roughness,
            sample.metallic,
            sample.specular_level,
            sample.ior,
            match &sample.normal {
                NormalModel::Geometric => "geometric".to_string(),
                NormalModel::AnalyticSlope { x, y } => format!("analytic-slope({x},{y})"),
            },
            sample.pattern.as_deref().unwrap_or("none")
        ),
        MaterialKind::Select { predicate, a, b } => {
            format!("kind=MaterialSelect predicate={predicate} a={a} b={b}")
        }
        MaterialKind::IdentityTable { enum_key, cases } => format!(
            "kind=IdentityTable enum={} cases=[{}]",
            enum_key,
            cases
                .iter()
                .map(|(identity, material)| format!("{}={material}", identity.variant))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn render_obligation(obligation: &ProofObligation) -> String {
    match obligation {
        ProofObligation::DenominatorNonZero { denominator } => {
            format!("denominator-nonzero value={denominator}")
        }
        ProofObligation::GuardedDenominatorNonZero {
            denominator,
            predicate,
        } => {
            format!("guarded-denominator-nonzero value={denominator} when={predicate}")
        }
        ProofObligation::RestrictedTrigDomain { argument } => {
            format!("restricted-trig-domain value={argument}")
        }
        ProofObligation::DynamicIndexInBounds { index, extent } => {
            format!("dynamic-index-in-bounds index={index} extent={extent}")
        }
    }
}

fn dump_origin(kind: &str, id: &str, origin: &super::arena::NodeOrigin, out: &mut String) {
    out.push_str(&format!(
        "    Origin kind={kind} id={id} primary={}:{}:{}@bytes={}..{}",
        origin.primary.module,
        origin.primary.span.line,
        origin.primary.span.col,
        origin.primary.span.byte_start,
        origin.primary.span.byte_end,
    ));
    if !origin.expansion_chain.is_empty() {
        out.push_str(" expansion=[");
        out.push_str(
            &origin
                .expansion_chain
                .iter()
                .map(|site| {
                    format!(
                        "{}:{}:{}@bytes={}..{}",
                        site.module,
                        site.span.line,
                        site.span.col,
                        site.span.byte_start,
                        site.span.byte_end
                    )
                })
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push(']');
    }
    if !origin.merged.is_empty() {
        out.push_str(" merged=[");
        out.push_str(
            &origin
                .merged
                .iter()
                .map(|site| {
                    format!(
                        "{}:{}:{}@bytes={}..{}",
                        site.module,
                        site.span.line,
                        site.span.col,
                        site.span.byte_start,
                        site.span.byte_end
                    )
                })
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push(']');
    }
    out.push('\n');
}

pub fn dump_frame_program(skeleton: &PlaneSkeleton) -> String {
    format!(
        "FrameProgram v1 renderer={} digest={}\n  Header magic=WRELAPX\\0 version=1 bytes=80 flags=[] total_bytes=80\n  Directory count=0 offset=80\n  WalkingSkeleton version=P-1 semantic_seed={} storage=generated-actor\n",
        skeleton.renderer_index, skeleton.frame_program_digest, skeleton.semantic_digest
    )
}

pub fn dump_render_layout(skeleton: &PlaneSkeleton) -> String {
    let renderer = crate::codegen::emit_pixels_plane_renderer(
        &skeleton.frame_program,
        &skeleton.semantic_seed,
    );
    let code_bytes = renderer.code.len() * 4;
    let memory_bytes = wrela_machine::pixels::FRAME_BYTES
        + wrela_machine::pixels::CONTROL_BYTES
        + wrela_machine::pixels::QUEUE_CAPACITY as usize * wrela_machine::pixels::TILE_BYTES
        + skeleton.frame_program.len();
    format!(
        "RenderLayout v1\n  Renderer index={}\n    FrameProgram base={:#010x} size=80\n    GeneratedActor type=Renderer entry={} worker_count=0\n    Display ref={}\n    Mode width={} height={} refresh_hz={} shade_hz={}\n    Tile owner=renderer#0 range=[0,1)\n    Buffer base={:#010x} bytes={} format=BGRA8\n    Baseline code_bytes={} memory_bytes={} frame_cost_instructions={}\n",
        skeleton.renderer_index,
        wrela_machine::pixels::FRAME_PROGRAM_BASE,
        crate::codegen::PIXELS_RENDERER_SYMBOL,
        skeleton.display,
        skeleton.width,
        skeleton.height,
        skeleton.refresh_hz,
        skeleton.shade_hz,
        wrela_machine::pixels::FRAMEBUFFER_BASE,
        wrela_machine::pixels::FRAME_BYTES,
        code_bytes,
        memory_bytes,
        renderer.code.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_renderer_dumps_are_complete_and_byte_stable() {
        assert_eq!(
            dump_zero_renderers(PixelsDumpStage::FieldGraph),
            "FieldGraph v1\nRenderers count=0\n"
        );
        assert_eq!(
            dump_zero_renderers(PixelsDumpStage::FrameProgram),
            "FrameProgram v1\nRenderers count=0\n"
        );
        assert_eq!(
            dump_zero_renderers(PixelsDumpStage::RenderLayout),
            "RenderLayout v1\nRenderers count=0\n"
        );
    }

    #[test]
    fn uncompiled_dump_keeps_stage_boundary_explicit() {
        let configs = RendererConfigs {
            renderers: vec![RendererConfig {
                declaration_index: 0,
                params_type: crate::sema::types::Type::U32,
                field: "world".to_string(),
                material: "shade".to_string(),
                material_type: crate::sema::types::Type::U8,
                display_index: 0,
                width: 64,
                height: 32,
                refresh_hz: 60,
                shade_hz: 30,
                profile: "AaaByteExact".to_string(),
                tone_curve: "Linear".to_string(),
                near: 0.1,
                far: 10.0,
                world_min: crate::pixels::config::Vec3Config {
                    x: -1.0,
                    y: -1.0,
                    z: -1.0,
                },
                world_max: crate::pixels::config::Vec3Config {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
                camera_max_motion: 0.0,
                light_capacity: 0,
                light_kinds: vec![],
                exposure: crate::pixels::config::ScalarRangeConfig { min: 0.0, max: 1.0 },
                environment: crate::pixels::config::RgbRangeConfig {
                    min: [0.0; 3],
                    max: [1.0; 3],
                },
                ao_enabled: false,
                probes_enabled: false,
                probe_initialization_worst_case_ms: 0,
                initialization_deadline_ms: 1,
                parameter_contracts: vec![],
            }],
        };
        let dump = dump_uncompiled_configs(PixelsDumpStage::FieldGraph, &configs, None);
        assert!(dump.contains("Compilation status=not-run"));
        assert!(!dump.contains("Field id="));
    }

    #[test]
    fn dedicated_float_formatters_round_trip_exact_finite_bits() {
        for bits in [
            0u32,
            (-0.0f32).to_bits(),
            1u32,
            0x0080_0000,
            1.0f32.to_bits(),
            f32::MAX.to_bits(),
        ] {
            let text = format_f32_bits(bits);
            assert_eq!(text.parse::<f32>().unwrap().to_bits(), bits, "{text}");
        }
        for bits in [
            0u64,
            (-0.0f64).to_bits(),
            1u64,
            0x0010_0000_0000_0000,
            1.0f64.to_bits(),
            f64::MAX.to_bits(),
        ] {
            let text = format_f64(f64::from_bits(bits));
            assert_eq!(text.parse::<f64>().unwrap().to_bits(), bits, "{text}");
        }
    }
}
