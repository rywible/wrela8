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
use super::projection_bounds::TILE_WIDTH_V1;
use super::repeat::RepeatTemplate;
use super::symbolic::SymbolicGraph;

pub(crate) const CANDIDATE_RECORD_BYTES_V1: u64 = 64;
pub(crate) const ROOT_RECORD_BYTES_V1: u64 = 32;
pub(crate) const ROOT_CELL_BYTES_V1: u64 = 16;
pub(crate) const SHEET_RECORD_BYTES_V1: u64 = 64;
pub(crate) const EVENT_RECORD_BYTES_V1: u64 = 32;
pub(crate) const EVENT_CELL_BYTES_V1: u64 = 16;
// P7 run records retain the complete transport/recheck evidence named by the
// milestone contract: q jet/error, order/root slack, normal cone, event
// ownership, proof owner/method, and visible sheet metadata. Corridors keep
// the common header plus side/coverage/arrangement evidence.
pub(crate) const RUN_RECORD_BYTES_V1: u64 = 128;
pub(crate) const CORRIDOR_RECORD_BYTES_V1: u64 = 64;
pub(crate) const REBUILD_CELL_BYTES_V1: u64 = 24;
pub(crate) const FIXED_Q_RECORD_BYTES_V1: u64 = 16;
pub(crate) const SHADING_RECORD_BYTES_V1: u64 = 64;
pub(crate) const TRANSPARENCY_LAYER_BYTES_V1: u64 = 32;
pub(crate) const KINETIC_CERTIFICATE_BYTES_V1: u64 = 128;
pub(crate) const RENDERER_STATE_HEADER_BYTES_V1: u64 = 256;
pub(crate) const FRAME_COMPLEX_PIXEL_BYTES_V1: u64 = 32;
pub(crate) const TILE_DESCRIPTOR_BYTES_V1: u64 =
    wrela_machine::pixels::DISPLAY_TILE_DESC_BYTES_V1 as u64;
pub(crate) const TILE_OWNERSHIP_BYTES_V1: u64 = 1;
pub(crate) const FAILURE_RECORD_BYTES_V1: u64 = 64;
pub(crate) const WORKSPACE_HEADER_BYTES_V1: u64 = 64;
pub(crate) const P7_CANONICAL_FRAME_SNAPSHOT_BYTES: u64 = 688;

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
    pub immutable_index_bytes: u64,
    pub renderer_state_bytes: u64,
}

impl PixelsCeilings {
    pub const MACHINE_V1: Self = Self {
        objects: 1024,
        features: 2048,
        repeated_instances: 1024,
        // P7 snapshots are copied into actor job records. 256 f32 slots keep
        // every generated ownership/recovery frame within the sealed AArch64
        // scaled-offset addressing envelope.
        parameter_slots: 16,
        csg_stack: 256,
        event_records: 1_048_576,
        run_records_per_tile_row: 1_048_576,
        repeat_analysis_candidates: 1_000_000,
        structural_depth: 1024,
        event_isolation_depth: 2,
        immutable_index_bytes: 64 * 1024 * 1024,
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
    super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectiveCapacities {
    pub candidate_features_per_tile: u32,
    pub row_start_roots: u32,
    pub active_sheets_per_row: u32,
    pub event_generators: u32,
    pub competition_pairs_per_tile: u32,
    pub row_event_intervals: u32,
    pub root_stack_nodes: u32,
    pub event_stack_nodes: u32,
    pub runs_per_row: u32,
    pub corridors_per_row: u32,
    pub max_index_slice: u32,
    pub polynomial_programs: u32,
    pub rational_programs: u32,
    pub polynomial_terms_per_program: u32,
    pub coefficient_nodes: u32,
    pub derivative_bundles: u32,
    pub derivative_clusters: u32,
    pub index_bytes: u64,
    pub workspace_header_bytes: u64,
    pub candidate_bytes: u64,
    pub root_records_bytes: u64,
    pub roots_tmp_bytes: u64,
    pub root_stack_bytes: u64,
    pub sheet_bytes: u64,
    pub event_records_bytes: u64,
    pub event_stack_bytes: u64,
    pub run_bytes: u64,
    pub rebuild_bytes: u64,
    pub corridor_bytes: u64,
    pub fixed_q_bytes: u64,
    pub shading_bytes: u64,
    pub transparency_bytes: u64,
    pub per_worker_scratch_bytes: u64,
    pub all_worker_scratch_bytes: u64,
    pub final_per_worker_scratch_bytes: u64,
    pub final_all_worker_scratch_bytes: u64,
    pub total_renderer_state_bytes: u64,
    pub total_renderer_state_bytes_instrumented: u64,
    pub derivations: Vec<CapacityDerivation>,
}

impl ProjectiveCapacities {
    pub(crate) fn worker_workspace_regions(&self) -> Result<Vec<(&'static str, u64, u64)>, String> {
        let fields = [
            ("HEADER", self.workspace_header_bytes),
            ("ACTIVE_FEATURES", self.candidate_bytes),
            ("ROOTS", self.root_records_bytes),
            ("ROOTS_TMP", self.roots_tmp_bytes),
            ("ROOT_STACK", self.root_stack_bytes),
            ("ACTIVE_SHEETS", self.sheet_bytes),
            ("EVENTS", self.event_records_bytes),
            ("EVENT_STACK", self.event_stack_bytes),
            ("RUNS", self.run_bytes),
            ("REBUILD", self.rebuild_bytes),
            ("CORRIDORS", self.corridor_bytes),
            ("FIXED_Q", self.fixed_q_bytes),
            ("SHADING", self.shading_bytes),
            ("TRANSPARENCY", self.transparency_bytes),
        ];
        let mut cursor = 0_u64;
        let mut regions = Vec::with_capacity(fields.len());
        for (name, bytes) in fields {
            regions.push((name, cursor, bytes));
            cursor = cursor
                .checked_add(bytes)
                .ok_or_else(|| format!("P025: worker `{name}` region end overflow"))?;
        }
        if cursor != self.final_per_worker_scratch_bytes {
            return Err(format!(
                "P025: worker workspace fields cover {cursor} bytes, expected {}",
                self.final_per_worker_scratch_bytes
            ));
        }
        Ok(regions)
    }
}

fn verify_immutable_index_bytes(index_bytes: u64) -> Result<(), String> {
    ceiling(
        "immutable_index_bytes",
        index_bytes,
        PixelsCeilings::MACHINE_V1.immutable_index_bytes,
        &[
            format!("completed local indexes require {index_bytes} bytes"),
            "P5 frame-program placement uses this sealed P4 immutable-index budget".to_string(),
        ],
    )
}

fn final_renderer_state_bytes(
    structural: &StructuralCapacities,
    all_worker_scratch_bytes: u64,
) -> Result<(u64, u64), String> {
    let pre_framebuffer = [
        structural.state_header_bytes,
        structural.coefficient_snapshot_bytes,
        structural.frame_dependency_snapshot_bytes,
        structural.frame_complex_double_buffer_bytes,
        all_worker_scratch_bytes,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, bytes| {
        add(sum, bytes, "p4_final_renderer_state_bytes")
    })?;
    let page = wrela_machine::layout::PIXELS_STATE_PAGE_ALIGNMENT;
    let framebuffer_offset = pre_framebuffer
        .checked_add(page - 1)
        .map(|value| value & !(page - 1))
        .ok_or_else(|| "P015: P4 framebuffer state alignment overflow".to_string())?;
    let after_framebuffers = add(
        framebuffer_offset,
        structural.output_double_buffer_bytes,
        "p4_final_renderer_state_bytes",
    )?;
    let probe_offset = if structural.probe_bytes == 0 {
        after_framebuffers
    } else {
        after_framebuffers
            .checked_add(page - 1)
            .map(|value| value & !(page - 1))
            .ok_or_else(|| "P015: P4 probe state alignment overflow".to_string())?
    };
    let total = [
        add(
            probe_offset,
            structural.probe_bytes,
            "p4_final_renderer_state_bytes",
        )?,
        structural.kinetic_certificate_bytes,
        structural.tile_descriptor_bytes,
        structural.tile_ownership_bytes,
        structural.failure_record_bytes,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, bytes| {
        add(sum, bytes, "p4_final_renderer_state_bytes")
    })?;
    let telemetry_offset = total
        .checked_add(7)
        .map(|value| value & !7)
        .ok_or_else(|| "P015: P4 telemetry state alignment overflow".to_string())?;
    let instrumented = add(
        telemetry_offset,
        structural.telemetry_bytes_instrumented,
        "p4_final_instrumented_renderer_state_bytes",
    )?;
    Ok((total, instrumented))
}

pub fn derive_projective(
    structural: &StructuralCapacities,
    projective: &super::projective::ProjectiveEquations,
    derivatives: &super::derivatives::DerivativePrograms,
    spans: &[super::projection_bounds::ProjectedFeatureSpan],
    events: &super::events::EventPrograms,
    competitions: &super::competition::CompetitionPrograms,
    indexes: &super::index::LocalIndexes,
) -> Result<ProjectiveCapacities, String> {
    let ceilings = PixelsCeilings::MACHINE_V1;
    let (candidate_features_per_row, candidate_features_per_tile) =
        super::projection_bounds::exact_max_overlap(spans)?;
    if candidate_features_per_row > structural.max_projected_features_per_row {
        return Err(format!(
            "P015: P4 projected row overlap {} exceeds the sealed P3 ceiling of {}",
            candidate_features_per_row, structural.max_projected_features_per_row
        ));
    }
    if candidate_features_per_tile > structural.max_projected_features_per_tile {
        return Err(format!(
            "P015: P4 projected tile overlap {} exceeds the sealed P3 ceiling of {}",
            candidate_features_per_tile, structural.max_projected_features_per_tile
        ));
    }
    let row_start_roots_u64 = indexes
        .tile_features
        .cells
        .iter()
        .map(|slice| {
            let start = usize::try_from(slice.offset).map_err(|_| {
                "pixels::capacities: feature index offset exceeds usize".to_string()
            })?;
            let end = slice
                .offset
                .checked_add(slice.count)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| "pixels::capacities: feature index end overflow".to_string())?;
            let ids = indexes.tile_features.ids.get(start..end).ok_or_else(|| {
                "pixels::capacities: feature index slice is out of bounds".to_string()
            })?;
            let ids = ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            derivatives
                .clusters
                .iter()
                .filter(|cluster| {
                    cluster.bundles.iter().any(|bundle| {
                        derivatives
                            .bundles
                            .get(bundle.index())
                            .is_some_and(|bundle| ids.contains(&bundle.feature.0))
                    })
                })
                .try_fold(0_u64, |sum, cluster| {
                    add(
                        sum,
                        u64::from(cluster.root_tube.maximum_object_roots),
                        "p4_row_start_object_roots",
                    )
                })
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    let row_start_roots = u32::try_from(row_start_roots_u64)
        .map_err(|_| "P015: P4 row-start root count exceeds u32".to_string())?;
    let active_sheets_per_row = row_start_roots;
    let event_generators = u32_count(events.generators.len(), "p4_event_generators")?;
    let competition_pairs_per_tile = indexes
        .tile_competitions
        .cells
        .iter()
        .map(|slice| slice.count)
        .max()
        .unwrap_or(0);
    let leaves_per_root = 1_u64
        .checked_shl(ceilings.event_isolation_depth)
        .ok_or_else(|| "P015: P4 event subdivision shift overflow".to_string())?;
    let leaves_per_root_u32 = u32::try_from(leaves_per_root)
        .map_err(|_| "P015: P4 event subdivision count exceeds u32".to_string())?;
    let row_event_intervals_u64 = indexes
        .tile_events
        .cells
        .iter()
        .map(|slice| {
            let start = usize::try_from(slice.offset)
                .map_err(|_| "pixels::capacities: event index offset exceeds usize".to_string())?;
            let end = slice
                .offset
                .checked_add(slice.count)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| "pixels::capacities: event index end overflow".to_string())?;
            indexes
                .tile_events
                .ids
                .get(start..end)
                .ok_or_else(|| {
                    "pixels::capacities: event index slice is out of bounds".to_string()
                })?
                .iter()
                .try_fold(0_u64, |sum, id| {
                    let id_index = usize::try_from(*id)
                        .map_err(|_| "pixels::capacities: event ID exceeds usize".to_string())?;
                    let event = events.generators.get(id_index).ok_or_else(|| {
                        format!("pixels::capacities: event index names missing e{id}")
                    })?;
                    add(
                        sum,
                        u64::from(event.maximum_root_count),
                        "p4_event_intervals",
                    )
                })
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    ceiling(
        "event_records",
        row_event_intervals_u64,
        ceilings.event_records.into(),
        &[
            format!("{event_generators} completed P4 event generators indexed by tile"),
            "each generator contributes its sealed root count; isolation uses a shared stack"
                .to_string(),
        ],
    )?;
    let row_event_intervals = u32::try_from(row_event_intervals_u64)
        .map_err(|_| "P015: P4 row event count exceeds u32".to_string())?;
    let root_stack_nodes = u32::try_from(
        row_start_roots_u64
            .checked_mul(leaves_per_root)
            .ok_or_else(|| "P015: P4 root stack count overflow".to_string())?,
    )
    .map_err(|_| "P015: P4 root stack count exceeds u32".to_string())?;
    let event_stack_nodes = events
        .generators
        .iter()
        .map(|event| u32::from(event.maximum_root_count).checked_mul(leaves_per_root_u32))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "P015: P4 event isolation stack count overflow".to_string())?
        .into_iter()
        .max()
        .unwrap_or(0);
    let event_runs_per_row_u64 = row_event_intervals_u64
        .checked_add(1)
        .ok_or_else(|| "P015: P4 run count overflow".to_string())?;
    let runs_per_row_u64 = event_runs_per_row_u64.max(u64::from(TILE_WIDTH_V1));
    ceiling(
        "run_records_per_tile_row",
        runs_per_row_u64,
        ceilings.run_records_per_tile_row.into(),
        &[
            format!("{row_event_intervals} completed event intervals plus one terminal run"),
            format!(
                "{TILE_WIDTH_V1} terminal pixel-cell records for the bounded P7 rebuild ladder"
            ),
        ],
    )?;
    let runs_per_row = u32::try_from(runs_per_row_u64)
        .map_err(|_| "P015: P4 run count exceeds u32".to_string())?;
    let corridors_per_row = row_event_intervals.max(TILE_WIDTH_V1);
    let max_index_slice = [
        &indexes.tile_features,
        &indexes.tile_events,
        &indexes.tile_competitions,
        &indexes.row_block_repeats,
        &indexes.tile_lights,
        &indexes.tile_probes,
    ]
    .into_iter()
    .flat_map(|index| index.cells.iter().map(|slice| slice.count))
    .max()
    .unwrap_or(0);
    let polynomial_programs = u32_count(projective.polynomials.len(), "polynomial_programs")?;
    let rational_programs = u32_count(projective.rationals.len(), "rational_programs")?;
    let polynomial_terms_per_program = projective
        .polynomials
        .iter()
        .map(|program| program.terms.len())
        .max()
        .map(|count| u32_count(count, "polynomial_terms"))
        .transpose()?
        .unwrap_or(0);
    let coefficient_nodes = u32_count(projective.coefficients.nodes.len(), "coefficient_nodes")?;
    let derivative_bundles = u32_count(derivatives.bundles.len(), "derivative_bundles")?;
    let derivative_clusters = u32_count(derivatives.clusters.len(), "derivative_clusters")?;
    let candidate_bytes = mul(
        u64::from(candidate_features_per_tile),
        CANDIDATE_RECORD_BYTES_V1,
        "p4_candidate_bytes",
    )?;
    let root_records_bytes = mul(
        u64::from(root_stack_nodes),
        ROOT_RECORD_BYTES_V1,
        "p7_root_records_bytes",
    )?;
    let roots_tmp_bytes = root_records_bytes;
    let root_stack_bytes = mul(
        u64::from(root_stack_nodes),
        ROOT_CELL_BYTES_V1,
        "p7_root_stack_bytes",
    )?;
    let sheet_bytes = mul(
        u64::from(active_sheets_per_row),
        SHEET_RECORD_BYTES_V1,
        "p4_sheet_bytes",
    )?;
    let event_records_bytes = mul(
        u64::from(row_event_intervals),
        EVENT_RECORD_BYTES_V1,
        "p7_event_records_bytes",
    )?;
    let event_stack_bytes = mul(
        u64::from(event_stack_nodes),
        EVENT_CELL_BYTES_V1,
        "p7_event_stack_bytes",
    )?;
    let run_bytes = mul(u64::from(runs_per_row), RUN_RECORD_BYTES_V1, "p4_run_bytes")?;
    let rebuild_bytes = mul(
        u64::from(structural.max_local_rebuild_queue),
        REBUILD_CELL_BYTES_V1,
        "p7_rebuild_bytes",
    )?;
    let corridor_bytes = mul(
        u64::from(corridors_per_row),
        CORRIDOR_RECORD_BYTES_V1,
        "p4_corridor_bytes",
    )?;
    let per_worker_scratch_bytes = [
        WORKSPACE_HEADER_BYTES_V1,
        candidate_bytes,
        root_records_bytes,
        roots_tmp_bytes,
        root_stack_bytes,
        sheet_bytes,
        event_records_bytes,
        event_stack_bytes,
        run_bytes,
        rebuild_bytes,
        corridor_bytes,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, bytes| {
        add(sum, bytes, "p4_per_worker_scratch_bytes")
    })?;
    let all_worker_scratch_bytes = mul(
        per_worker_scratch_bytes,
        u64::from(structural.worker_count),
        "p4_all_worker_scratch_bytes",
    )?;
    let retained_per_worker_scratch_bytes = [
        structural.fixed_q_bytes,
        structural.shading_bytes,
        structural.transparency_bytes,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, bytes| {
        add(sum, bytes, "p4_retained_per_worker_scratch_bytes")
    })?;
    let final_per_worker_scratch_bytes = add(
        per_worker_scratch_bytes,
        retained_per_worker_scratch_bytes,
        "p4_final_per_worker_scratch_bytes",
    )?;
    let final_all_worker_scratch_bytes = mul(
        final_per_worker_scratch_bytes,
        u64::from(structural.worker_count),
        "p4_final_all_worker_scratch_bytes",
    )?;
    let (total_renderer_state_bytes, total_renderer_state_bytes_instrumented) =
        final_renderer_state_bytes(structural, final_all_worker_scratch_bytes)?;
    verify_immutable_index_bytes(indexes.bytes)?;
    ceiling(
        "renderer_state_bytes",
        total_renderer_state_bytes_instrumented,
        ceilings.renderer_state_bytes,
        &[
            format!("P4 final per-worker scratch={final_per_worker_scratch_bytes}"),
            format!("workers={}", structural.worker_count),
            format!("final instrumented state={total_renderer_state_bytes_instrumented}"),
        ],
    )?;
    Ok(ProjectiveCapacities {
        candidate_features_per_tile,
        row_start_roots,
        active_sheets_per_row,
        event_generators,
        competition_pairs_per_tile,
        row_event_intervals,
        root_stack_nodes,
        event_stack_nodes,
        runs_per_row,
        corridors_per_row,
        max_index_slice,
        polynomial_programs,
        rational_programs,
        polynomial_terms_per_program,
        coefficient_nodes,
        derivative_bundles,
        derivative_clusters,
        index_bytes: indexes.bytes,
        workspace_header_bytes: WORKSPACE_HEADER_BYTES_V1,
        candidate_bytes,
        root_records_bytes,
        roots_tmp_bytes,
        root_stack_bytes,
        sheet_bytes,
        event_records_bytes,
        event_stack_bytes,
        run_bytes,
        rebuild_bytes,
        corridor_bytes,
        fixed_q_bytes: structural.fixed_q_bytes,
        shading_bytes: structural.shading_bytes,
        transparency_bytes: structural.transparency_bytes,
        per_worker_scratch_bytes,
        all_worker_scratch_bytes,
        final_per_worker_scratch_bytes,
        final_all_worker_scratch_bytes,
        total_renderer_state_bytes,
        total_renderer_state_bytes_instrumented,
        derivations: vec![
            CapacityDerivation {
                field: "p4_candidate_features_per_tile",
                value: u64::from(candidate_features_per_tile),
                why: vec![format!(
                    "exact endpoint/cell sweep gives row overlap {candidate_features_per_row} \
                         within P3 ceiling {} and tile overlap {candidate_features_per_tile} \
                         within P3 ceiling {}",
                    structural.max_projected_features_per_row,
                    structural.max_projected_features_per_tile,
                )],
            },
            CapacityDerivation {
                field: "p4_final_renderer_state_bytes",
                value: total_renderer_state_bytes,
                why: vec![
                    format!("P4-refined six-class scratch={per_worker_scratch_bytes} per worker"),
                    format!(
                        "retained fixed-q/shading/transparency scratch={retained_per_worker_scratch_bytes} per worker"
                    ),
                    format!("workers={}", structural.worker_count),
                ],
            },
            CapacityDerivation {
                field: "p4_row_start_roots",
                value: row_start_roots_u64,
                why: vec![
                    "exact tile overlap over composed-object root clusters".to_string(),
                    format!(
                        "smooth clusters retain up to 2^{} root/corridor leaves per predictor slab",
                        ceilings.event_isolation_depth
                    ),
                ],
            },
            CapacityDerivation {
                field: "p4_row_event_intervals",
                value: row_event_intervals_u64,
                why: vec![
                    format!("{event_generators} completed local generators"),
                    format!("{} competition pairs survive", competitions.pairs.len()),
                    format!(
                        "2^{} leaves per isolated root",
                        ceilings.event_isolation_depth
                    ),
                ],
            },
            CapacityDerivation {
                field: "p4_index_bytes",
                value: indexes.bytes,
                why: vec![
                    "completed offset/count cells plus sorted contiguous ID tables".to_string(),
                ],
            },
        ],
    })
}

fn u32_count(value: usize, kind: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("P015: renderer capacity `{kind}` overflows u32"))
}

pub(crate) fn checked_event_isolation_depth(value: u32) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| {
        format!("P015: event isolation depth {value} exceeds the u8 storage ceiling of 255")
    })
}

fn add(a: u64, b: u64, kind: &str) -> Result<u64, String> {
    a.checked_add(b)
        .ok_or_else(|| format!("P015: renderer capacity `{kind}` arithmetic overflow"))
}

fn mul(a: u64, b: u64, kind: &str) -> Result<u64, String> {
    a.checked_mul(b)
        .ok_or_else(|| format!("P015: renderer capacity `{kind}` arithmetic overflow"))
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
    super::deform::oscillation_bound(deform, values)
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
    let event_run_records_u64 = add(max_event_records_u64, 1, "run_records_per_tile_row")?;
    let max_run_records_u64 = event_run_records_u64.max(u64::from(TILE_WIDTH_V1));
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
            format!(
                "{TILE_WIDTH_V1} terminal pixel-cell records for the bounded P7 rebuild ladder"
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
    let pixel_count = mul(
        u64::from(config.width),
        u64::from(config.height),
        "output_pixels",
    )?;
    let telemetry_bytes_production = 0;
    let telemetry_bytes_per_worker = mul(
        telemetry_counter_count_v1(),
        8,
        "telemetry_bytes_per_worker",
    )?;
    let telemetry_counter_bytes_instrumented = mul(
        telemetry_bytes_per_worker,
        u64::from(config.worker_count),
        "telemetry_counter_bytes_instrumented",
    )?;
    // Instrumented conformance retains the three fixed-q recurrence values
    // and claim flags for every output pixel. It is deliberately outside the
    // production layout and lets the host validate every regular lane rather
    // than extrapolating from one representative run.
    let raster_evidence_bytes = mul(pixel_count, 24, "raster_evidence_bytes")?;
    let telemetry_bytes_instrumented = add(
        telemetry_counter_bytes_instrumented,
        raster_evidence_bytes,
        "telemetry_bytes_instrumented",
    )?;
    let scanout_tile_columns =
        u64::from(config.width).div_ceil(u64::from(wrela_machine::pixels::TILE_WIDTH));
    let scanout_tile_rows =
        u64::from(config.height).div_ceil(u64::from(wrela_machine::pixels::TILE_HEIGHT));
    let scanout_tile_count = mul(
        scanout_tile_columns,
        scanout_tile_rows,
        "scanout_tile_count",
    )?;
    let output_tile_bytes = mul(
        scanout_tile_count,
        wrela_machine::pixels::TILE_ALLOCATION_BYTES as u64,
        "output_tile_bytes",
    )?;
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
    // P7 workers receive a padding-free canonical copy of every frame-owned
    // input: 16 parameters, 12 camera components, 8 * (kind + 15 scalar
    // light components), exposure, environment, frame index, and validity.
    // Keep current/previous storage even when a particular later stage does
    // not consume every field; visibility correctness must not depend on
    // source struct layout or coordinator turn-frame lifetime.
    let frame_dependency_snapshot_bytes = mul(
        u64::from(params.frame_dependencies.runtime_bytes).max(P7_CANONICAL_FRAME_SNAPSHOT_BYTES),
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
    // Each generation owns a complete control followed by an ascending,
    // row-major descriptor list. Keeping the two lists disjoint makes failed
    // submissions recoverable without mutating the current front list.
    let descriptors_per_generation = add(
        wrela_machine::pixels::CONTROL_BYTES as u64,
        mul(
            scanout_tile_count,
            TILE_DESCRIPTOR_BYTES_V1,
            "tile_descriptor_bytes",
        )?,
        "tile_descriptor_bytes",
    )?;
    let tile_descriptor_bytes = mul(descriptors_per_generation, 2, "tile_descriptor_bytes")?;
    let tile_ownership_bytes = mul(
        mul(
            scanout_tile_count,
            TILE_OWNERSHIP_BYTES_V1,
            "tile_ownership_bytes",
        )?,
        2,
        "tile_ownership_bytes",
    )?;
    let failure_record_bytes = FAILURE_RECORD_BYTES_V1;
    let pre_framebuffer_bytes = [
        state_header_bytes,
        coefficient_snapshot_bytes,
        frame_dependency_snapshot_bytes,
        frame_complex_double_buffer_bytes,
        all_worker_scratch_bytes,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, value| add(sum, value, "renderer_state_bytes"))?;
    let page = wrela_machine::layout::PIXELS_STATE_PAGE_ALIGNMENT;
    let framebuffer_offset = pre_framebuffer_bytes
        .checked_add(page - 1)
        .map(|value| value & !(page - 1))
        .ok_or_else(|| "P015: framebuffer state alignment overflow".to_string())?;
    let after_framebuffers = add(
        framebuffer_offset,
        output_double_buffer_bytes,
        "renderer_state_bytes",
    )?;
    let probe_offset = if probe_bytes == 0 {
        after_framebuffers
    } else {
        after_framebuffers
            .checked_add(page - 1)
            .map(|value| value & !(page - 1))
            .ok_or_else(|| "P015: probe state alignment overflow".to_string())?
    };
    let after_probes = add(probe_offset, probe_bytes, "renderer_state_bytes")?;
    let state_alignment_padding_bytes = (framebuffer_offset - pre_framebuffer_bytes)
        .checked_add(probe_offset - after_framebuffers)
        .ok_or_else(|| "P015: renderer state padding overflow".to_string())?;
    let total_renderer_state_bytes = [
        after_probes,
        kinetic_certificate_bytes,
        tile_descriptor_bytes,
        tile_ownership_bytes,
        failure_record_bytes,
    ]
    .into_iter()
    .try_fold(0_u64, |sum, value| add(sum, value, "renderer_state_bytes"))?;
    let telemetry_offset = total_renderer_state_bytes
        .checked_add(7)
        .map(|value| value & !7)
        .ok_or_else(|| "P015: telemetry state alignment overflow".to_string())?;
    let total_renderer_state_bytes_instrumented = add(
        telemetry_offset,
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
            format!("{state_alignment_padding_bytes} page-alignment padding bytes"),
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
                format!(
                    "one terminal run, with a floor of {TILE_WIDTH_V1} terminal pixel-cell records"
                ),
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
                format!("page_alignment_padding={state_alignment_padding_bytes}"),
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
        assert!(
            verify_immutable_index_bytes(PixelsCeilings::MACHINE_V1.immutable_index_bytes).is_ok()
        );
        assert!(
            verify_immutable_index_bytes(PixelsCeilings::MACHINE_V1.immutable_index_bytes + 1)
                .unwrap_err()
                .contains("completed local indexes require 67108865 bytes")
        );
    }

    #[test]
    fn telemetry_bytes_come_from_schema_counts() {
        assert_eq!(
            telemetry_counter_count_v1(),
            crate::pixels::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2
        );
        assert_eq!(telemetry_counter_count_v1() * 8, 1200);
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
        assert_eq!(checked_event_isolation_depth(255).unwrap(), 255);
        assert_eq!(
            checked_event_isolation_depth(256).unwrap_err(),
            "P015: event isolation depth 256 exceeds the u8 storage ceiling of 255"
        );
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
            frequency_scalar: super::super::ids::ScalarId(2),
            phase_scalar: super::super::ids::ScalarId(3),
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
