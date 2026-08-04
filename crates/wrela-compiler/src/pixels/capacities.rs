//! Checked derivation of every finite P3 renderer storage class.

use super::bounds::ValueBounds;
use super::config::RendererConfig;
use super::csg::CsgProgram;
use super::deform::DeformationTemplate;
use super::features::FeatureRecord;
use super::material::MaterialEvent;
use super::material_graph::MaterialKind;
use super::objects::ObjectPartition;
use super::params::ParameterLayout;
use super::repeat::RepeatTemplate;
use super::symbolic::SymbolicGraph;

pub(crate) const CANDIDATE_RECORD_BYTES_V1: u64 = 64;
pub(crate) const ROOT_RECORD_BYTES_V1: u64 = 32;
pub(crate) const SHEET_RECORD_BYTES_V1: u64 = 64;
pub(crate) const EVENT_RECORD_BYTES_V1: u64 = 32;
pub(crate) const RUN_RECORD_BYTES_V1: u64 = 32;
pub(crate) const CORRIDOR_RECORD_BYTES_V1: u64 = 24;
pub(crate) const FIXED_Q_RECORD_BYTES_V1: u64 = 16;
pub(crate) const SHADING_RECORD_BYTES_V1: u64 = 64;
pub(crate) const TRANSPARENCY_LAYER_BYTES_V1: u64 = 32;
pub(crate) const KINETIC_CERTIFICATE_BYTES_V1: u64 = 128;
pub(crate) const RENDERER_STATE_HEADER_BYTES_V1: u64 = 256;
pub(crate) const FRAME_COMPLEX_PIXEL_BYTES_V1: u64 = 32;
pub(crate) const TILE_DESCRIPTOR_BYTES_V1: u64 = 32;
pub(crate) const TILE_OWNERSHIP_BYTES_V1: u64 = 4;
pub(crate) const FAILURE_RECORD_BYTES_V1: u64 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelsCeilings {
    pub objects: u32,
    pub features: u32,
    pub repeated_instances: u32,
    pub parameter_slots: u32,
    pub csg_stack: u32,
    pub event_records: u32,
    pub run_records_per_tile_row: u32,
    pub repeat_analysis_candidates: u32,
    pub structural_depth: u32,
    pub event_isolation_depth: u32,
    pub renderer_state_bytes: u64,
}

impl PixelsCeilings {
    pub const MACHINE_V1: Self = Self {
        objects: 1024,
        features: 2048,
        repeated_instances: 1024,
        parameter_slots: 4096,
        csg_stack: 256,
        event_records: 1_048_576,
        run_records_per_tile_row: 1_048_576,
        repeat_analysis_candidates: 1_000_000,
        structural_depth: 1024,
        event_isolation_depth: 2,
        renderer_state_bytes: 512 * 1024 * 1024,
    };
}

#[repr(u8)]
pub enum CertificateProofMethodV1 {
    EndpointSign,
    Monotone,
    Convex,
    Sturm,
    Taylor,
    IntervalNewton,
    Krawczyk,
    AnalyticLinear,
    AnalyticQuadratic,
    AnalyticQuartic,
    CsgStable,
    IdentityStable,
    FrontOrder,
    Exclusion,
    RadianceTail,
    Fallback,
    Count,
}

#[repr(u8)]
pub enum CertificateExpiryCauseV1 {
    ParameterDelta,
    CameraDelta,
    LightDelta,
    ProjectionMargin,
    RootMargin,
    IdentityMargin,
    OrderMargin,
    CsgInfluence,
    MaterialBoundary,
    RepeatWrap,
    Deformation,
    TileBoundary,
    ProbeVersion,
    Transparency,
    Generation,
    ExplicitInvalidate,
    Count,
}

#[repr(u8)]
pub enum RebuildReasonV1 {
    Initialization,
    MissingCertificate,
    ExpiredCertificate,
    RootFailure,
    UniquenessFailure,
    IdentityFailure,
    OrderFailure,
    CsgFailure,
    MaterialFailure,
    RepeatSplit,
    DeformationFailure,
    CapacityFailure,
    NumericFailure,
    TransparencyFailure,
    ProbeFailure,
    PresentationRetry,
    Count,
}

fn telemetry_counter_count_v1() -> u64 {
    CertificateProofMethodV1::Count as u64
        + CertificateExpiryCauseV1::Count as u64
        + RebuildReasonV1::Count as u64
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapacityDerivation {
    pub field: &'static str,
    pub value: u64,
    pub why: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuralCapacities {
    pub worker_count: u32,
    pub object_count: u32,
    pub feature_template_count: u32,
    pub feature_count: u32,
    pub repeated_instance_count: u32,
    pub scalar_program_slots: u32,
    pub derivative_program_slots: u32,
    pub parameter_slots: u32,
    pub max_csg_stack: u32,
    pub max_projected_features_per_row: u32,
    pub max_projected_features_per_tile: u32,
    pub max_object_roots_per_row_start: u32,
    pub max_active_sheet_records_per_row: u32,
    pub event_generator_count: u32,
    pub max_event_subdivisions: u32,
    pub max_event_records: u32,
    pub max_run_records_per_tile_row: u32,
    pub max_csg_events_per_row: u32,
    pub max_transparent_layers: u32,
    pub max_local_rebuild_queue: u32,
    pub candidate_bytes: u64,
    pub root_bytes: u64,
    pub sheet_bytes: u64,
    pub event_bytes: u64,
    pub run_bytes: u64,
    pub corridor_bytes: u64,
    pub fixed_q_bytes: u64,
    pub shading_bytes: u64,
    pub transparency_bytes: u64,
    pub per_worker_scratch_bytes: u64,
    pub all_worker_scratch_bytes: u64,
    pub telemetry_bytes_production: u64,
    pub telemetry_bytes_instrumented: u64,
    pub output_tile_bytes: u64,
    pub output_double_buffer_bytes: u64,
    pub probe_bytes: u64,
    pub kinetic_certificate_bytes: u64,
    pub state_header_bytes: u64,
    pub coefficient_snapshot_bytes: u64,
    pub frame_dependency_snapshot_bytes: u64,
    pub frame_complex_double_buffer_bytes: u64,
    pub tile_descriptor_bytes: u64,
    pub tile_ownership_bytes: u64,
    pub failure_record_bytes: u64,
    pub total_renderer_state_bytes: u64,
    pub total_renderer_state_bytes_instrumented: u64,
    pub derivations: Vec<CapacityDerivation>,
}

fn u32_count(value: usize, kind: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("P015: renderer capacity `{kind}` overflows u32"))
}

fn add(a: u64, b: u64, kind: &str) -> Result<u64, String> {
    a.checked_add(b)
        .ok_or_else(|| format!("P015: renderer capacity `{kind}` arithmetic overflow"))
}

fn mul(a: u64, b: u64, kind: &str) -> Result<u64, String> {
    a.checked_mul(b)
        .ok_or_else(|| format!("P015: renderer capacity `{kind}` arithmetic overflow"))
}

fn ceil_u64(value: f64, kind: &str) -> Result<u64, String> {
    if !value.is_finite() || value < 0.0 || value.ceil() > u64::MAX as f64 {
        return Err(format!(
            "P015: renderer capacity `{kind}` cannot represent derived value {value}"
        ));
    }
    Ok(value.ceil() as u64)
}

fn ceiling(kind: &str, needed: u64, ceiling: u64, why: &[String]) -> Result<(), String> {
    if needed <= ceiling {
        return Ok(());
    }
    let mut message = format!(
        "P015: renderer capacity `{kind}` needs {needed} slots, which exceeds the machine-v1 ceiling of {ceiling}"
    );
    for reason in why {
        message.push_str("\n  ");
        message.push_str(reason);
    }
    Err(message)
}

fn feature_root_bound(
    kind: super::primitive::FeatureKind,
    deformation_oscillations: u64,
) -> Result<u64, String> {
    let degree = match kind {
        super::primitive::FeatureKind::Plane => 1_u64,
        super::primitive::FeatureKind::Quadric => 2,
        super::primitive::FeatureKind::Quartic => 4,
    };
    add(
        degree,
        mul(deformation_oscillations, 2, "feature_root_bound")?,
        "feature_root_bound",
    )
}

fn event_subdivision_capacity(
    maximum_roots: u64,
    dyadic_isolation_depth: u32,
) -> Result<u32, String> {
    let leaves_per_root = 1_u64
        .checked_shl(dyadic_isolation_depth)
        .ok_or_else(|| "P015: dyadic event isolation depth overflows u64".to_string())?;
    let required = maximum_roots
        .checked_mul(leaves_per_root)
        .ok_or_else(|| "P015: event subdivision count overflow".to_string())?;
    u32::try_from(required).map_err(|_| "P015: event subdivision count exceeds u32".to_string())
}

fn deformation_oscillation_bound(
    values: &ValueBounds,
    deform: &DeformationTemplate,
) -> Result<u64, String> {
    let coordinate = values.get(deform.coordinate_x)?;
    let angle = coordinate
        .mul_outward(deform.frequency)?
        .add_outward(deform.phase)?;
    add(
        ceil_u64(
            angle.width() / std::f64::consts::PI,
            "deformation_oscillations",
        )?,
        2,
        "deformation_oscillations",
    )
}

pub fn derive(
    graph: &SymbolicGraph,
    config: &RendererConfig,
    params: &ParameterLayout,
    values: &ValueBounds,
    objects: &ObjectPartition,
    csg: &CsgProgram,
    features: &[FeatureRecord],
    repeats: &[RepeatTemplate],
    deformations: &[DeformationTemplate],
    material_events: &[MaterialEvent],
) -> Result<StructuralCapacities, String> {
    let ceilings = PixelsCeilings::MACHINE_V1;
    let object_count = u32_count(objects.objects.len(), "objects")?;
    let repeated_instance_count = u32_count(
        objects
            .objects
            .iter()
            .filter(|object| !object.repeat_instances.is_empty())
            .count(),
        "repeated_instances",
    )?;
    let feature_template_count = features
        .iter()
        .map(|feature| feature.template_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let feature_template_count = u32_count(feature_template_count, "feature_templates")?;
    let feature_count = u32_count(features.len(), "features")?;
    let parameter_slots = u32_count(params.slots.len(), "parameter_slots")?;
    let scalar_program_slots = u32_count(graph.scalar.len(), "scalar_program_slots")?;
    let derivative_program_slots = scalar_program_slots;

    let feature_why = vec![
        format!("{object_count} maximal smooth objects"),
        format!("{feature_template_count} immutable fused primitive feature templates"),
        format!(
            "{feature_count} total feature slots after instantiating those templates across {object_count} objects"
        ),
        format!("{} repeat template records", repeats.len()),
    ];
    ceiling(
        "objects",
        object_count.into(),
        ceilings.objects.into(),
        &feature_why,
    )?;
    ceiling(
        "features",
        feature_count.into(),
        ceilings.features.into(),
        &feature_why,
    )?;
    ceiling(
        "repeated_instances",
        repeated_instance_count.into(),
        ceilings.repeated_instances.into(),
        &feature_why,
    )?;
    ceiling(
        "parameter_slots",
        parameter_slots.into(),
        ceilings.parameter_slots.into(),
        &[format!("{} packed coefficient bytes", params.packed_bytes)],
    )?;
    ceiling(
        "csg_stack",
        csg.max_stack.into(),
        ceilings.csg_stack.into(),
        &[format!(
            "{} postfix CSG instructions",
            csg.instructions.len()
        )],
    )?;

    // Before P4 supplies tighter projected spans, the conservative endpoint
    // sweep is the single interval containing the complete output extent.
    // Thus every feature may overlap every row and tile.
    let max_projected_features_per_row = feature_count;
    let max_projected_features_per_tile = feature_count;
    let deformation_oscillations = deformations
        .iter()
        .map(|deform| {
            Ok::<_, String>((deform.field, deformation_oscillation_bound(values, deform)?))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let feature_roots = |feature: &FeatureRecord| -> Result<u64, String> {
        let path_oscillations =
            feature
                .occurrence_path
                .iter()
                .try_fold(0_u64, |sum, occurrence| {
                    add(
                        sum,
                        deformation_oscillations
                            .get(&occurrence.field)
                            .copied()
                            .unwrap_or(0),
                        "feature_deformation_oscillations",
                    )
                })?;
        feature_root_bound(feature.kind, path_oscillations)
    };
    let total_feature_root_bound = features.iter().try_fold(0_u64, |total, feature| {
        add(total, feature_roots(feature)?, "feature_root_bound")
    })?;
    let max_object_roots_per_row_start = u32::try_from(total_feature_root_bound)
        .map_err(|_| "P015: object-root capacity exceeds u32".to_string())?;
    let max_active_sheet_records_per_row = max_object_roots_per_row_start;
    let primitive_generators = u64::from(feature_count);
    let feature_boundary_generators = features.iter().try_fold(0_u64, |count, feature| {
        add(
            count,
            u64::from(feature.validity.boundary_generator_count()?),
            "event_generators",
        )
    })?;
    let material_generators = material_events.iter().try_fold(0_u64, |count, event| {
        add(
            count,
            mul(
                u64::try_from(event.feature_owners.len())
                    .map_err(|_| "P015: material event owner count overflow".to_string())?,
                u64::from(event.crossing_bound),
                "event_generators",
            )?,
            "event_generators",
        )
    })?;
    let repeat_wrap_generators = repeats.iter().try_fold(0_u64, |count, template| {
        add(
            count,
            u64::try_from(template.wrap_events.len())
                .map_err(|_| "P015: repeat wrap generator count overflow".to_string())?,
            "event_generators",
        )
    })?;
    let event_generator_count_u64 = add(
        add(
            add(
                primitive_generators,
                feature_boundary_generators,
                "event_generators",
            )?,
            material_generators,
            "event_generators",
        )?,
        repeat_wrap_generators,
        "event_generators",
    )?;
    let event_generator_count = u32::try_from(event_generator_count_u64)
        .map_err(|_| "P015: event generator count exceeds u32".to_string())?;
    let maximum_roots_per_generator = features
        .iter()
        .map(feature_roots)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .unwrap_or(1);
    let max_event_subdivisions =
        event_subdivision_capacity(maximum_roots_per_generator, ceilings.event_isolation_depth)?;
    let max_event_records_u64 = mul(
        event_generator_count_u64,
        u64::from(max_event_subdivisions),
        "event_records",
    )?;
    ceiling(
        "event_records",
        max_event_records_u64,
        ceilings.event_records.into(),
        &[
            format!("{event_generator_count} structural event generators"),
            format!(
                "{max_event_subdivisions} subdivisions = {maximum_roots_per_generator} roots * 2^{} fixed dyadic isolation depth",
                ceilings.event_isolation_depth
            ),
        ],
    )?;
    let max_event_records = u32::try_from(max_event_records_u64)
        .map_err(|_| "P015: event record count exceeds u32".to_string())?;
    let max_run_records_u64 = add(max_event_records_u64, 1, "run_records_per_tile_row")?;
    ceiling(
        "run_records_per_tile_row",
        max_run_records_u64,
        ceilings.run_records_per_tile_row.into(),
        &[
            format!("{event_generator_count} structural event generators"),
            format!(
                "{max_event_subdivisions} subdivisions = {maximum_roots_per_generator} roots * 2^{} fixed dyadic isolation depth",
                ceilings.event_isolation_depth
            ),
        ],
    )?;
    let max_run_records_per_tile_row = u32::try_from(max_run_records_u64)
        .map_err(|_| "P015: run record count exceeds u32".to_string())?;
    let max_csg_events_per_row = max_object_roots_per_row_start;
    let max_transparent_layers = if object_count == 0 {
        0
    } else if material_may_transmit(graph, values)? {
        max_object_roots_per_row_start
    } else {
        1
    };
    let max_local_rebuild_queue = max_run_records_per_tile_row;

    let candidate_bytes = mul(
        u64::from(max_projected_features_per_tile),
        CANDIDATE_RECORD_BYTES_V1,
        "candidate_bytes",
    )?;
    let root_bytes = mul(
        u64::from(max_object_roots_per_row_start),
        ROOT_RECORD_BYTES_V1,
        "root_bytes",
    )?;
    let sheet_bytes = mul(
        u64::from(max_active_sheet_records_per_row),
        SHEET_RECORD_BYTES_V1,
        "sheet_bytes",
    )?;
    let event_bytes = mul(max_event_records_u64, EVENT_RECORD_BYTES_V1, "event_bytes")?;
    let run_bytes = mul(max_run_records_u64, RUN_RECORD_BYTES_V1, "run_bytes")?;
    let corridor_bytes = mul(
        max_run_records_u64,
        CORRIDOR_RECORD_BYTES_V1,
        "corridor_bytes",
    )?;
    let fixed_q_bytes = mul(
        max_run_records_u64,
        FIXED_Q_RECORD_BYTES_V1,
        "fixed_q_bytes",
    )?;
    let shading_bytes = mul(
        max_run_records_u64,
        SHADING_RECORD_BYTES_V1,
        "shading_bytes",
    )?;
    let transparency_bytes = mul(
        mul(
            max_run_records_u64,
            u64::from(max_transparent_layers),
            "transparency_bytes",
        )?,
        TRANSPARENCY_LAYER_BYTES_V1,
        "transparency_bytes",
    )?;
    let per_worker_scratch_bytes = [
        candidate_bytes,
        root_bytes,
        sheet_bytes,
        event_bytes,
        run_bytes,
        corridor_bytes,
        fixed_q_bytes,
        shading_bytes,
        transparency_bytes,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, bytes| {
        add(sum, bytes, "per_worker_scratch_bytes")
    })?;
    let all_worker_scratch_bytes = mul(
        per_worker_scratch_bytes,
        u64::from(config.worker_count),
        "all_worker_scratch_bytes",
    )?;
    let telemetry_bytes_production = 0;
    let telemetry_bytes_instrumented = mul(
        telemetry_counter_count_v1(),
        8,
        "telemetry_bytes_instrumented",
    )?;
    let pixel_count = mul(
        u64::from(config.width),
        u64::from(config.height),
        "output_pixels",
    )?;
    let output_tile_bytes = mul(pixel_count, 4, "output_tile_bytes")?;
    let output_double_buffer_bytes = mul(output_tile_bytes, 2, "output_double_buffer_bytes")?;
    let probe_bytes = if config.probes_enabled {
        mul(pixel_count, 16, "probe_bytes")?
    } else {
        0
    };
    let kinetic_certificate_bytes = mul(
        u64::from(feature_count),
        KINETIC_CERTIFICATE_BYTES_V1,
        "kinetic_certificate_bytes",
    )?;
    let state_header_bytes = RENDERER_STATE_HEADER_BYTES_V1;
    let coefficient_snapshot_bytes = mul(
        u64::from(params.packed_bytes),
        2,
        "coefficient_snapshot_bytes",
    )?;
    let frame_dependency_snapshot_bytes = mul(
        u64::from(params.frame_dependencies.runtime_bytes),
        2,
        "frame_dependency_snapshot_bytes",
    )?;
    let frame_complex_double_buffer_bytes = mul(
        mul(
            pixel_count,
            FRAME_COMPLEX_PIXEL_BYTES_V1,
            "frame_complex_bytes",
        )?,
        2,
        "frame_complex_double_buffer_bytes",
    )?;
    // P3 has not fixed the later render-tile dimensions. Reserving one
    // descriptor/owner per output pixel is the finite conservative endpoint;
    // later milestones may prove a smaller exact tiling.
    let tile_descriptor_bytes = mul(
        pixel_count,
        TILE_DESCRIPTOR_BYTES_V1,
        "tile_descriptor_bytes",
    )?;
    let tile_ownership_bytes = mul(pixel_count, TILE_OWNERSHIP_BYTES_V1, "tile_ownership_bytes")?;
    let failure_record_bytes = FAILURE_RECORD_BYTES_V1;
    let total_renderer_state_bytes = [
        state_header_bytes,
        coefficient_snapshot_bytes,
        frame_dependency_snapshot_bytes,
        frame_complex_double_buffer_bytes,
        all_worker_scratch_bytes,
        output_double_buffer_bytes,
        probe_bytes,
        kinetic_certificate_bytes,
        tile_descriptor_bytes,
        tile_ownership_bytes,
        failure_record_bytes,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, value| add(sum, value, "renderer_state_bytes"))?;
    let total_renderer_state_bytes_instrumented = add(
        total_renderer_state_bytes,
        telemetry_bytes_instrumented,
        "instrumented_renderer_state_bytes",
    )?;
    ceiling(
        "renderer_state_bytes",
        total_renderer_state_bytes_instrumented,
        ceilings.renderer_state_bytes,
        &[
            format!("{state_header_bytes} renderer-state header bytes"),
            format!("{coefficient_snapshot_bytes} current/previous coefficient snapshot bytes"),
            format!(
                "{frame_dependency_snapshot_bytes} current/previous camera/light/post snapshot bytes"
            ),
            format!("{frame_complex_double_buffer_bytes} double-buffered FrameComplex bytes"),
            format!(
                "{all_worker_scratch_bytes} structural scratch bytes = {per_worker_scratch_bytes} per worker * {} workers",
                config.worker_count
            ),
            format!("{output_double_buffer_bytes} output double-buffer bytes"),
            format!("{probe_bytes} probe bytes"),
            format!("{kinetic_certificate_bytes} kinetic certificate bytes"),
            format!("{tile_descriptor_bytes} tile descriptor bytes"),
            format!("{tile_ownership_bytes} tile ownership bytes"),
            format!("{failure_record_bytes} failure record bytes"),
            format!("{telemetry_bytes_instrumented} instrumented telemetry bytes"),
        ],
    )?;

    let derivations = vec![
        CapacityDerivation {
            field: "feature_count",
            value: feature_count.into(),
            why: feature_why,
        },
        CapacityDerivation {
            field: "max_projected_features_per_row",
            value: max_projected_features_per_row.into(),
            why: vec![
                "P3 conservative projected interval is the complete output row domain".to_string(),
                "P4 may tighten but may not exceed this count".to_string(),
            ],
        },
        CapacityDerivation {
            field: "max_run_records_per_tile_row",
            value: max_run_records_u64,
            why: vec![
                format!("{event_generator_count} generators"),
                format!("{primitive_generators} primitive equation generators"),
                format!("{feature_boundary_generators} feature-validity boundary generators"),
                format!("{material_generators} material event/owner generators"),
                format!("{repeat_wrap_generators} repeat wrap generators"),
                format!(
                    "{max_event_subdivisions} subdivisions each ({maximum_roots_per_generator} roots * 2^{} dyadic depth)",
                    ceilings.event_isolation_depth
                ),
                "one terminal run".to_string(),
            ],
        },
        CapacityDerivation {
            field: "renderer_state_bytes",
            value: total_renderer_state_bytes,
            why: vec![
                format!("header={state_header_bytes}"),
                format!("coefficient_snapshots={coefficient_snapshot_bytes}"),
                format!("frame_snapshots={frame_dependency_snapshot_bytes}"),
                format!("frame_complexes={frame_complex_double_buffer_bytes}"),
                format!(
                    "scratch={all_worker_scratch_bytes} ({per_worker_scratch_bytes} * {} workers)",
                    config.worker_count
                ),
                format!("output={output_double_buffer_bytes}"),
                format!("probes={probe_bytes}"),
                format!("kinetic={kinetic_certificate_bytes}"),
                format!("tile_descriptors={tile_descriptor_bytes}"),
                format!("tile_ownership={tile_ownership_bytes}"),
                format!("failure={failure_record_bytes}"),
            ],
        },
        CapacityDerivation {
            field: "instrumented_renderer_state_bytes",
            value: total_renderer_state_bytes_instrumented,
            why: vec![
                format!("production={total_renderer_state_bytes}"),
                format!("telemetry={telemetry_bytes_instrumented}"),
            ],
        },
    ];

    Ok(StructuralCapacities {
        worker_count: config.worker_count,
        object_count,
        feature_template_count,
        feature_count,
        repeated_instance_count,
        scalar_program_slots,
        derivative_program_slots,
        parameter_slots,
        max_csg_stack: csg.max_stack,
        max_projected_features_per_row,
        max_projected_features_per_tile,
        max_object_roots_per_row_start,
        max_active_sheet_records_per_row,
        event_generator_count,
        max_event_subdivisions,
        max_event_records,
        max_run_records_per_tile_row,
        max_csg_events_per_row,
        max_transparent_layers,
        max_local_rebuild_queue,
        candidate_bytes,
        root_bytes,
        sheet_bytes,
        event_bytes,
        run_bytes,
        corridor_bytes,
        fixed_q_bytes,
        shading_bytes,
        transparency_bytes,
        per_worker_scratch_bytes,
        all_worker_scratch_bytes,
        telemetry_bytes_production,
        telemetry_bytes_instrumented,
        output_tile_bytes,
        output_double_buffer_bytes,
        probe_bytes,
        kinetic_certificate_bytes,
        state_header_bytes,
        coefficient_snapshot_bytes,
        frame_dependency_snapshot_bytes,
        frame_complex_double_buffer_bytes,
        tile_descriptor_bytes,
        tile_ownership_bytes,
        failure_record_bytes,
        total_renderer_state_bytes,
        total_renderer_state_bytes_instrumented,
        derivations,
    })
}

pub(crate) fn material_may_transmit(
    graph: &SymbolicGraph,
    values: &ValueBounds,
) -> Result<bool, String> {
    for (_, material) in graph.materials.iter() {
        if let MaterialKind::Sample(sample) = &material.kind {
            if values.get(sample.opacity)?.lo < 1.0 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_and_ceiling_checks_fail_before_truncation() {
        assert!(mul(u64::MAX, 2, "test").unwrap_err().contains("overflow"));
        assert!(
            ceiling("features", 257, 256, &["257 sphere instances".to_string()])
                .unwrap_err()
                .contains("257 sphere instances")
        );
    }

    #[test]
    fn telemetry_bytes_come_from_schema_counts() {
        assert_eq!(
            telemetry_counter_count_v1(),
            CertificateProofMethodV1::Count as u64
                + CertificateExpiryCauseV1::Count as u64
                + RebuildReasonV1::Count as u64
        );
        assert_eq!(telemetry_counter_count_v1() * 8, 384);
    }

    #[test]
    fn root_and_subdivision_capacities_follow_degree_and_deformation_frequency() {
        assert_eq!(
            feature_root_bound(super::super::primitive::FeatureKind::Plane, 0).unwrap(),
            1
        );
        assert_eq!(
            feature_root_bound(super::super::primitive::FeatureKind::Quartic, 3).unwrap(),
            10
        );
        assert_eq!(event_subdivision_capacity(10, 2).unwrap(), 40);
        assert_eq!(event_subdivision_capacity(33, 2).unwrap(), 132);
    }

    #[test]
    fn deformation_oscillations_use_the_full_product_image_not_coordinate_width() {
        let coordinate = super::super::ids::ScalarId(0);
        let values = ValueBounds {
            scalar: [(
                coordinate,
                super::super::bounds::ScalarBound {
                    value: super::super::reference::interval::F64Interval::new(100.0, 101.0)
                        .unwrap(),
                    rule: "test",
                },
            )]
            .into_iter()
            .collect(),
        };
        let deform = DeformationTemplate {
            field: super::super::ids::FieldId(0),
            displacement: super::super::ids::ScalarId(1),
            derivation: super::super::graph::ClosedDeformDerivation::SinusoidalX,
            amplitude: 1.0,
            gradient: 100.0,
            hessian: 10_000.0,
            third_derivative: 1_000_000.0,
            coordinate_x: coordinate,
            frequency: super::super::reference::interval::F64Interval::new(1.0, 100.0).unwrap(),
            phase: super::super::reference::interval::F64Interval::point(0.0).unwrap(),
        };
        let bound = deformation_oscillation_bound(&values, &deform).unwrap();
        assert!(
            bound > 3_000,
            "full angle image spans roughly 10,000 radians"
        );
    }
}
