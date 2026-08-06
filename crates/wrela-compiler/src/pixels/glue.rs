//! Deterministic generated renderer actor/configuration metadata.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::config::RendererConfig;
use super::program::VerifiedFrameProgram;
use super::projection_bounds::{TILE_HEIGHT_V1, TILE_WIDTH_V1};

pub(crate) const RENDERER_FRAME_BOUNDS_WORDS: usize = 41;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedWorker {
    pub actor: String,
    pub core: usize,
    pub tiles_start: u32,
    pub tiles_end: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedRenderer {
    pub renderer_index: usize,
    pub coordinator: String,
    pub display_index: usize,
    pub workers: Vec<GeneratedWorker>,
    pub exposure_range: [f32; 2],
    pub environment_min: [f32; 3],
    pub environment_max: [f32; 3],
    pub camera_bounds: [[f32; 2]; 12],
    pub light_capacity: usize,
    pub light_kinds: [usize; 8],
    pub rooted_functions: Vec<String>,
    pub bootstrap_families: Vec<String>,
}

fn bootstrap_families(program: &VerifiedFrameProgram) -> BTreeSet<&'static str> {
    program
        .program()
        .tables
        .iter()
        .filter(|table| !table.records.is_empty())
        .map(|table| table.kind.stable_name())
        .collect()
}

fn write_renderer_constants(output: &mut String, index: usize, values: &[(&str, u64)]) {
    for (name, value) in values {
        writeln!(output, "const R{index}_{name}: usize = {value}")
            .expect("String writes cannot fail");
    }
}

fn outward_f32_interval(interval: super::reference::interval::F64Interval) -> [f32; 2] {
    let mut lo = interval.lo as f32;
    if f64::from(lo) > interval.lo {
        lo = super::reference::interval::next_down_f32(lo);
    }
    let mut hi = interval.hi as f32;
    if f64::from(hi) < interval.hi {
        hi = super::reference::interval::next_up_f32(hi);
    }
    [lo, hi]
}

fn light_kind_tag(kind: &str) -> Result<usize, String> {
    super::config::light_kind_tag(kind)
        .and_then(|tag| usize::try_from(tag).ok())
        .ok_or_else(|| format!("pixels::glue: sealed renderer has unknown light kind `{kind}`"))
}

fn state_region_constants(
    state_base: u64,
    base_name: &'static str,
    bytes_name: &'static str,
    region: super::state::StateRegion,
    bytes: Option<u64>,
) -> Result<[(&'static str, u64); 2], String> {
    let base = state_base
        .checked_add(region.offset)
        .ok_or_else(|| format!("P025: generated {base_name} state address overflow"))?;
    Ok([
        (base_name, base),
        (bytes_name, bytes.unwrap_or(region.bytes)),
    ])
}

fn canonical_wire_view_source() -> Result<String, String> {
    let (_, loaded) = crate::loader::load_render_program_module()
        .map_err(|_| "pixels::glue: stdlib/core/render_program.wr missing".to_string())?;
    let expected = BTreeSet::from([
        "FrameProgramHeaderV1",
        "FrameProgramTableV1",
        "FrameProgramRecordV1",
        "FrameProgramImmediateV1",
    ]);
    let mut module = loaded.module;
    module.path = vec!["__image_pixels".to_string()];
    module.imports.clear();
    module.items.retain(|item| {
        matches!(
            item,
            crate::syntax::ast::Item::Struct(item) if expected.contains(item.name.as_str())
        )
    });
    let found = module
        .items
        .iter()
        .filter_map(|item| match item {
            crate::syntax::ast::Item::Struct(item) => Some(item.name.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if found != expected {
        return Err("pixels::glue: canonical render-program wire views are incomplete".to_string());
    }
    let source = crate::syntax::printer::pretty(&module);
    let body = source
        .strip_prefix("module __image_pixels\n")
        .ok_or_else(|| {
            "pixels::glue: canonical render-program module has an unexpected address".to_string()
        })?;
    Ok(body.to_string())
}

pub fn generate(
    renderer_index: usize,
    config: &RendererConfig,
    program: &VerifiedFrameProgram,
) -> Result<GeneratedRenderer, String> {
    let workers = usize::try_from(config.worker_count)
        .map_err(|_| "pixels::glue: worker count exceeds usize".to_string())?;
    if workers == 0 {
        return Err("pixels::glue: renderer has zero workers".to_string());
    }
    let tiles_x = config.width.div_ceil(TILE_WIDTH_V1);
    let tiles_y = config.height.div_ceil(TILE_HEIGHT_V1);
    let tile_count = tiles_x
        .checked_mul(tiles_y)
        .ok_or_else(|| "P015: renderer tile count overflow".to_string())?;
    let workers_u32 =
        u32::try_from(workers).map_err(|_| "pixels::glue: worker count exceeds u32".to_string())?;
    let generated_workers = (0..workers)
        .map(|worker| {
            let worker_u32 =
                u32::try_from(worker).map_err(|_| "pixels::glue: worker index overflow")?;
            let start = u64::from(tile_count) * u64::from(worker_u32) / u64::from(workers_u32);
            let end = u64::from(tile_count) * u64::from(worker_u32 + 1) / u64::from(workers_u32);
            Ok(GeneratedWorker {
                actor: format!("__wrela_renderer_{renderer_index}_worker_{worker}"),
                core: worker,
                tiles_start: u32::try_from(start)
                    .map_err(|_| "pixels::glue: tile start overflow")?,
                tiles_end: u32::try_from(end).map_err(|_| "pixels::glue: tile end overflow")?,
            })
        })
        .collect::<Result<Vec<_>, &str>>()
        .map_err(str::to_string)?;

    let families = bootstrap_families(program);
    let coordinator = format!("__wrela_renderer_{renderer_index}_coordinator");
    let renderer_key = format!(
        "struct:Renderer[{}]",
        crate::sema::types::render_type(&config.params_type)
    );
    let mut rooted_functions = vec![
        format!("{renderer_key}.init"),
        format!("{renderer_key}.render"),
        "__wrela_pixels_bootstrap_dispatch".to_string(),
        "__wrela_pixels_display_present".to_string(),
        "__wrela_abort_val".to_string(),
    ];
    if workers != 0 {
        rooted_functions.push("RendererWorker.init".to_string());
        rooted_functions.push("RendererWorker.run_job".to_string());
    }
    for worker in 0..workers {
        rooted_functions.push(format!("{renderer_key}.__bootstrap_worker_path_{worker}"));
    }
    for family in &families {
        rooted_functions.push(format!(
            "__wrela_pixels_bootstrap_{}",
            family.replace('-', "_")
        ));
    }
    rooted_functions.sort();
    rooted_functions.dedup();
    let camera = super::camera::CameraContract::derive(config)?;
    let camera_bounds = [
        camera.eye_component[0],
        camera.eye_component[1],
        camera.eye_component[2],
        camera.forward_component[0],
        camera.forward_component[1],
        camera.forward_component[2],
        camera.right_component[0],
        camera.right_component[1],
        camera.right_component[2],
        camera.up_component[0],
        camera.up_component[1],
        camera.up_component[2],
    ]
    .map(outward_f32_interval);
    let mut light_kinds = [0usize; 8];
    for (slot, kind) in config.light_kinds.iter().enumerate() {
        let Some(target) = light_kinds.get_mut(slot) else {
            return Err("pixels::glue: sealed light topology exceeds eight slots".to_string());
        };
        *target = light_kind_tag(kind)?;
    }
    Ok(GeneratedRenderer {
        renderer_index,
        coordinator,
        display_index: config.display_index,
        workers: generated_workers,
        exposure_range: [config.exposure.min, config.exposure.max],
        environment_min: config.environment.min,
        environment_max: config.environment.max,
        camera_bounds,
        light_capacity: usize::try_from(config.light_capacity)
            .map_err(|_| "pixels::glue: light capacity exceeds usize".to_string())?,
        light_kinds,
        rooted_functions,
        bootstrap_families: families.into_iter().map(str::to_string).collect(),
    })
}

pub fn configuration_source(
    placements: &[crate::layout::RendererPlacement],
    compiled: &[super::CompiledRenderer],
    instrumented: bool,
) -> Result<String, String> {
    if placements.len() != compiled.len() {
        return Err("pixels::glue: placement/program count differs".to_string());
    }
    let mut output = String::from(
        "module __image_pixels\n\n\
         # These declarations are emitted mechanically from core.render_program,\n\
         # the single canonical Wrela wire-view schema. Keeping them local lets\n\
         # ordinary @layout nesting type the exact generated placed roots.\n",
    );
    output.push_str(&canonical_wire_view_source()?);
    writeln!(
        output,
        "\npub const N_RENDERERS: usize = {}",
        placements.len()
    )
    .expect("String writes cannot fail");
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let capacities = &renderer.structural.program().capacities;
        let projective = &renderer.projective.program().capacities;
        output.push('\n');
        write_renderer_constants(
            &mut output,
            index,
            &[
                ("FRAMEPROG_BASE", placement.frameprog_base),
                ("FRAMEPROG_BYTES", placement.frameprog_size),
                ("STATE_BASE", placement.state_base),
                ("STATE_BYTES", placement.state_size),
                ("WIDTH", u64::from(renderer.config.width)),
                ("HEIGHT", u64::from(renderer.config.height)),
                (
                    "DISPLAY_INDEX",
                    u64::try_from(renderer.config.display_index)
                        .map_err(|_| "P025: display index exceeds u64".to_string())?,
                ),
                ("REFRESH_HZ", u64::from(renderer.config.refresh_hz)),
                ("SHADE_HZ", u64::from(renderer.config.shade_hz)),
                ("LIGHT_CAPACITY", u64::from(renderer.config.light_capacity)),
                (
                    "PROBE_INITIALIZATION_WORST_CASE_MS",
                    u64::from(renderer.config.probe_initialization_worst_case_ms),
                ),
                (
                    "INITIALIZATION_DEADLINE_MS",
                    u64::from(renderer.config.initialization_deadline_ms),
                ),
                ("TILE_W", u64::from(TILE_WIDTH_V1)),
                ("TILE_H", u64::from(TILE_HEIGHT_V1)),
                ("WORKERS", u64::from(capacities.worker_count)),
                ("OBJECTS", u64::from(capacities.object_count)),
                (
                    "FEATURE_TEMPLATES",
                    u64::from(capacities.feature_template_count),
                ),
                ("FEATURES", u64::from(capacities.feature_count)),
                (
                    "REPEATED_INSTANCES",
                    u64::from(capacities.repeated_instance_count),
                ),
                (
                    "SCALAR_PROGRAM_SLOTS",
                    u64::from(capacities.scalar_program_slots),
                ),
                (
                    "DERIVATIVE_PROGRAM_SLOTS",
                    u64::from(capacities.derivative_program_slots),
                ),
                ("PARAMETER_SLOTS", u64::from(capacities.parameter_slots)),
                ("CSG_STACK", u64::from(capacities.max_csg_stack)),
                (
                    "MAX_PROJECTED_FEATURES_ROW",
                    u64::from(capacities.max_projected_features_per_row),
                ),
                (
                    "MAX_PROJECTED_FEATURES_TILE",
                    u64::from(capacities.max_projected_features_per_tile),
                ),
                (
                    "MAX_OBJECT_ROOTS_ROW_START",
                    u64::from(capacities.max_object_roots_per_row_start),
                ),
                (
                    "MAX_ACTIVE_SHEETS_ROW",
                    u64::from(capacities.max_active_sheet_records_per_row),
                ),
                (
                    "STRUCTURAL_EVENT_GENERATORS",
                    u64::from(capacities.event_generator_count),
                ),
                (
                    "MAX_EVENT_SUBDIVISIONS",
                    u64::from(capacities.max_event_subdivisions),
                ),
                ("MAX_EVENT_RECORDS", u64::from(capacities.max_event_records)),
                (
                    "MAX_RUN_RECORDS_TILE_ROW",
                    u64::from(capacities.max_run_records_per_tile_row),
                ),
                (
                    "MAX_CSG_EVENTS_ROW",
                    u64::from(capacities.max_csg_events_per_row),
                ),
                (
                    "MAX_TRANSPARENT_LAYERS",
                    u64::from(capacities.max_transparent_layers),
                ),
                (
                    "MAX_LOCAL_REBUILD_QUEUE",
                    u64::from(capacities.max_local_rebuild_queue),
                ),
                ("CANDIDATE_STORAGE_BYTES", capacities.candidate_bytes),
                ("ROOT_STORAGE_BYTES", capacities.root_bytes),
                ("SHEET_STORAGE_BYTES", capacities.sheet_bytes),
                ("EVENT_STORAGE_BYTES", capacities.event_bytes),
                ("RUN_STORAGE_BYTES", capacities.run_bytes),
                ("CORRIDOR_STORAGE_BYTES", capacities.corridor_bytes),
                ("FIXED_Q_STORAGE_BYTES", capacities.fixed_q_bytes),
                ("SHADING_STORAGE_BYTES", capacities.shading_bytes),
                ("TRANSPARENCY_STORAGE_BYTES", capacities.transparency_bytes),
                (
                    "STRUCTURAL_PER_WORKER_SCRATCH_BYTES",
                    capacities.per_worker_scratch_bytes,
                ),
                (
                    "STRUCTURAL_ALL_WORKER_SCRATCH_BYTES",
                    capacities.all_worker_scratch_bytes,
                ),
                (
                    "TELEMETRY_PRODUCTION_BYTES",
                    capacities.telemetry_bytes_production,
                ),
                (
                    "TELEMETRY_INSTRUMENTED_BYTES",
                    capacities.telemetry_bytes_instrumented,
                ),
                ("OUTPUT_TILE_BYTES", capacities.output_tile_bytes),
                (
                    "OUTPUT_DOUBLE_BUFFER_BYTES",
                    capacities.output_double_buffer_bytes,
                ),
                ("PROBE_STATE_BYTES", capacities.probe_bytes),
                (
                    "KINETIC_CERTIFICATE_BYTES",
                    capacities.kinetic_certificate_bytes,
                ),
                ("STATE_HEADER_CAPACITY_BYTES", capacities.state_header_bytes),
                (
                    "COEFFICIENT_SNAPSHOT_CAPACITY_BYTES",
                    capacities.coefficient_snapshot_bytes,
                ),
                (
                    "FRAME_SNAPSHOT_CAPACITY_BYTES",
                    capacities.frame_dependency_snapshot_bytes,
                ),
                (
                    "FRAME_COMPLEX_CAPACITY_BYTES",
                    capacities.frame_complex_double_buffer_bytes,
                ),
                (
                    "TILE_DESCRIPTOR_CAPACITY_BYTES",
                    capacities.tile_descriptor_bytes,
                ),
                (
                    "TILE_OWNERSHIP_CAPACITY_BYTES",
                    capacities.tile_ownership_bytes,
                ),
                (
                    "FAILURE_RECORD_CAPACITY_BYTES",
                    capacities.failure_record_bytes,
                ),
                (
                    "PRODUCTION_STATE_BYTES",
                    projective.total_renderer_state_bytes,
                ),
                (
                    "INSTRUMENTED_STATE_BYTES",
                    projective.total_renderer_state_bytes_instrumented,
                ),
                (
                    "CANDIDATE_FEATURES_TILE",
                    u64::from(projective.candidate_features_per_tile),
                ),
                ("ROW_START_ROOTS", u64::from(projective.row_start_roots)),
                (
                    "ACTIVE_SHEETS_ROW",
                    u64::from(projective.active_sheets_per_row),
                ),
                ("EVENT_GENERATORS", u64::from(projective.event_generators)),
                (
                    "COMPETITION_PAIRS_TILE",
                    u64::from(projective.competition_pairs_per_tile),
                ),
                (
                    "ROW_EVENT_INTERVALS",
                    u64::from(projective.row_event_intervals),
                ),
                ("ROOT_STACK_NODES", u64::from(projective.root_stack_nodes)),
                ("EVENT_STACK_NODES", u64::from(projective.event_stack_nodes)),
                ("RUNS_PER_ROW", u64::from(projective.runs_per_row)),
                ("CORRIDORS_PER_ROW", u64::from(projective.corridors_per_row)),
                ("MAX_INDEX_SLICE", u64::from(projective.max_index_slice)),
                (
                    "POLYNOMIAL_PROGRAMS",
                    u64::from(projective.polynomial_programs),
                ),
                ("RATIONAL_PROGRAMS", u64::from(projective.rational_programs)),
                (
                    "POLYNOMIAL_TERMS_PROGRAM",
                    u64::from(projective.polynomial_terms_per_program),
                ),
                ("COEFFICIENT_NODES", u64::from(projective.coefficient_nodes)),
                (
                    "DERIVATIVE_BUNDLES",
                    u64::from(projective.derivative_bundles),
                ),
                (
                    "DERIVATIVE_CLUSTERS",
                    u64::from(projective.derivative_clusters),
                ),
                ("INDEX_BYTES", projective.index_bytes),
                (
                    "PROJECTIVE_PER_WORKER_SCRATCH_BYTES",
                    projective.final_per_worker_scratch_bytes,
                ),
                (
                    "PROJECTIVE_ALL_WORKER_SCRATCH_BYTES",
                    projective.final_all_worker_scratch_bytes,
                ),
                (
                    "CERT_TELEMETRY_BYTES",
                    if instrumented {
                        capacities.telemetry_bytes_instrumented
                    } else {
                        0
                    },
                ),
            ],
        );
        let state = &renderer.mutable_layout;
        for region in [
            state_region_constants(
                placement.state_base,
                "STATE_HEADER_BASE",
                "STATE_HEADER_BYTES",
                state.header,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "COEFFICIENT_SNAPSHOTS_BASE",
                "COEFFICIENT_SNAPSHOTS_BYTES",
                state.coefficient_snapshots,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "FRAME_SNAPSHOTS_BASE",
                "FRAME_SNAPSHOTS_BYTES",
                state.frame_snapshots,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "FRAME_COMPLEXES_BASE",
                "FRAME_COMPLEXES_BYTES",
                state.frame_complexes,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "WORKER_SCRATCH_BASE",
                "WORKER_SCRATCH_BYTES",
                state.worker_scratch,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "FRAMEBUFFERS_BASE",
                "FRAMEBUFFERS_BYTES",
                state.framebuffers,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "PROBES_BASE",
                "PROBES_BYTES",
                state.probes,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "KINETIC_STATE_BASE",
                "KINETIC_STATE_BYTES",
                state.kinetic,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "TILE_DESCRIPTORS_BASE",
                "TILE_DESCRIPTORS_BYTES",
                state.tile_descriptors,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "TILE_OWNERSHIP_BASE",
                "TILE_OWNERSHIP_BYTES",
                state.tile_ownership,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "FAILURE_BASE",
                "FAILURE_BYTES",
                state.failure,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "TELEMETRY_BASE",
                "TELEMETRY_BYTES",
                state.telemetry,
                Some(if instrumented {
                    state.telemetry.bytes
                } else {
                    0
                }),
            )?,
        ] {
            write_renderer_constants(&mut output, index, &region);
        }
        write_renderer_constants(
            &mut output,
            index,
            &[
                ("FRAMEBUFFER_BASE", placement.framebuffer_base),
                ("FRAMEBUFFER_BYTES", placement.framebuffer_bytes),
                ("PROBE_RESERVATION_BASE", placement.probe_base),
                ("PROBE_RESERVATION_BYTES", placement.probe_bytes),
            ],
        );
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            writeln!(
                output,
                "const R{index}_WORKER_{worker_index}_CORE: usize = {}\n\
                 const R{index}_WORKER_{worker_index}_TILES_START: usize = {}\n\
                 const R{index}_WORKER_{worker_index}_TILES_END: usize = {}\n\
                 const R{index}_WORKER_{worker_index}_WORKSPACE_BASE: usize = {}\n\
                 const R{index}_WORKER_{worker_index}_WORKSPACE_BYTES: usize = {}",
                worker.core,
                worker.tiles_start,
                worker.tiles_end,
                worker.workspace_base,
                worker.workspace_bytes,
            )
            .expect("String writes cannot fail");
        }
        let tables = super::binary_verify::verify_envelope(&renderer.encoded).map_err(|error| {
            format!("pixels::glue: encoded program failed verification: {error}")
        })?;
        for table in tables {
            let upper = table
                .kind
                .stable_name()
                .replace('-', "_")
                .to_ascii_uppercase();
            writeln!(
                output,
                "const R{index}_{upper}_BASE: usize = {:#x}\n\
                 const R{index}_{upper}_COUNT: usize = {}\n\
                 const R{index}_{upper}_BYTES: usize = {}",
                if table.count == 0 {
                    0
                } else {
                    placement
                        .frameprog_base
                        .checked_add(u64::from(table.offset))
                        .ok_or_else(|| {
                            "P025: generated frame-program table address overflow".to_string()
                        })?
                },
                table.count,
                table.byte_len,
            )
            .expect("String writes cannot fail");
            if table.count != 0 {
                let view_name = table
                    .kind
                    .stable_name()
                    .split('-')
                    .map(|part| {
                        let mut chars = part.chars();
                        chars
                            .next()
                            .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                            .unwrap_or_default()
                    })
                    .collect::<String>();
                let table_base = placement
                    .frameprog_base
                    .checked_add(u64::from(table.offset))
                    .ok_or_else(|| "P025: generated placed table address overflow".to_string())?;
                let record_type =
                    if table.kind == wrela_machine::pixels::FrameProgramTableKindV1::Immediate {
                        "FrameProgramImmediateV1"
                    } else {
                        "FrameProgramRecordV1"
                    };
                writeln!(
                    output,
                    "@layout(runtime, endian=little)\n\
                     struct R{index}{view_name}TableView:\n\
                     \x20   records: [{record_type}; R{index}_{upper}_COUNT]\n\
                     @placed({table_base:#x})\n\
                     static R{index}_{upper}_TABLE: R{index}{view_name}TableView",
                )
                .expect("String writes cannot fail");
            }
        }
        writeln!(
            output,
            "\n@layout(runtime, endian=little)\n\
             struct R{index}FrameProgramRootView:\n\
             \x20   header: FrameProgramHeaderV1\n\
             \x20   directory: [FrameProgramTableV1; {}]\n\
             @placed({:#x})\n\
             static R{index}_FRAME_PROGRAM: R{index}FrameProgramRootView\n\
             const R{index}_DIRECTORY_COUNT: usize = {}",
            wrela_machine::pixels::FrameProgramTableKindV1::REQUIRED_COUNT,
            placement.frameprog_base,
            wrela_machine::pixels::FrameProgramTableKindV1::REQUIRED_COUNT,
        )
        .expect("String writes cannot fail");
    }
    Ok(output)
}

pub fn parse_configuration_source(source: &str) -> Result<crate::syntax::ast::Module, String> {
    let tokens = crate::syntax::lexer::lex(source)
        .map_err(|error| format!("pixels::glue: generated module lex: {}", error.message))?;
    crate::syntax::parser::parse(tokens)
        .map_err(|error| format!("pixels::glue: generated module parse: {}", error.message))
}

fn rewrite_renderer_refs(
    value: &mut crate::eval::value::Value,
    coordinators: &[usize],
) -> Result<(), String> {
    use crate::eval::image::ImageDeclRef;
    use crate::eval::value::Value;
    match value {
        Value::ImageDecl(ImageDeclRef::Renderer(index)) => {
            let actor = coordinators.get(*index).copied().ok_or_else(|| {
                format!("pixels::glue: renderer handle {index} has no coordinator")
            })?;
            *value = Value::ImageDecl(ImageDeclRef::Actor(actor));
        }
        Value::Tuple(values)
        | Value::Array(values)
        | Value::Struct(values)
        | Value::Enum(_, values) => {
            for value in values {
                rewrite_renderer_refs(value, coordinators)?;
            }
        }
        Value::Closure { env, .. } => {
            for scope in env {
                for value in scope.values_mut() {
                    rewrite_renderer_refs(value, coordinators)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn integer_arg(label: &str, value: u64) -> crate::eval::image::DeclArg {
    crate::eval::image::DeclArg {
        label: label.to_string(),
        ty: crate::sema::types::Type::Usize,
        value: crate::eval::value::Value::Usize(value),
        span: crate::syntax::ast::Span::default(),
    }
}

fn u32_arg(label: &str, value: u32) -> crate::eval::image::DeclArg {
    crate::eval::image::DeclArg {
        label: label.to_string(),
        ty: crate::sema::types::Type::U32,
        value: crate::eval::value::Value::U32(value),
        span: crate::syntax::ast::Span::default(),
    }
}

fn handle_arg(
    label: &str,
    ty: crate::sema::types::Type,
    value: crate::eval::image::ImageDeclRef,
) -> crate::eval::image::DeclArg {
    crate::eval::image::DeclArg {
        label: label.to_string(),
        ty,
        value: crate::eval::value::Value::ImageDecl(value),
        span: crate::syntax::ast::Span::default(),
    }
}

fn renderer_worker_type() -> crate::sema::types::Type {
    crate::sema::types::Type::Named("RendererWorker".to_string(), Vec::new())
}

fn padded_worker_handles(
    first_worker: usize,
    worker_count: usize,
) -> Result<Vec<crate::eval::value::Value>, String> {
    if worker_count == 0 || worker_count > wrela_machine::CORE_SLOTS {
        return Err("pixels::glue: generated worker count is outside machine slots".to_string());
    }
    (0..wrela_machine::CORE_SLOTS)
        .map(|worker| {
            let declared = worker.min(worker_count - 1);
            first_worker
                .checked_add(declared)
                .map(crate::eval::image::ImageDeclRef::Actor)
                .map(crate::eval::value::Value::ImageDecl)
                .ok_or_else(|| "P015: generated worker edge index overflow".to_string())
        })
        .collect()
}

pub fn synthesize_image_graph(
    source: &crate::eval::image::ImageGraph,
    renderers: &[GeneratedRenderer],
) -> Result<crate::eval::image::ImageGraph, String> {
    if source.renderers.len() != renderers.len() {
        return Err("pixels::glue: renderer graph/generated count differs".to_string());
    }
    let mut graph = source.clone();
    let original_actor_count = graph.actors.len();
    let mut coordinators = Vec::with_capacity(renderers.len());
    let mut next_actor = original_actor_count;
    for renderer in renderers {
        coordinators.push(next_actor);
        next_actor = next_actor
            .checked_add(1 + renderer.workers.len())
            .ok_or_else(|| "P015: generated actor count overflow".to_string())?;
    }
    if next_actor > crate::rtconfig::MB_POOL_COUNT {
        return Err(format!(
            "P015: renderer-generated actors need {next_actor} mailbox slots, ceiling {}",
            crate::rtconfig::MB_POOL_COUNT
        ));
    }
    for actor in &mut graph.actors {
        for argument in &mut actor.args {
            rewrite_renderer_refs(&mut argument.value, &coordinators)?;
        }
    }
    for (renderer_index, generated) in renderers.iter().enumerate() {
        let renderer_decl = source
            .renderers
            .get(renderer_index)
            .ok_or_else(|| "pixels::glue: renderer declaration missing".to_string())?;
        let mailbox = u64::try_from(generated.workers.len() + 2)
            .map_err(|_| "P015: coordinator mailbox capacity overflow".to_string())?;
        let first_worker = coordinators[renderer_index]
            .checked_add(1)
            .ok_or_else(|| "P015: first generated worker index overflow".to_string())?;
        let worker_handles = padded_worker_handles(first_worker, generated.workers.len())?;
        let mut frame_bounds = generated
            .exposure_range
            .into_iter()
            .chain(generated.environment_min)
            .chain(generated.environment_max)
            .map(crate::eval::value::Value::F32)
            .collect::<Vec<_>>();
        for bounds in generated.camera_bounds {
            frame_bounds.extend(bounds.map(crate::eval::value::Value::F32));
        }
        frame_bounds.push(crate::eval::value::Value::Usize(
            u64::try_from(generated.light_capacity)
                .map_err(|_| "P015: generated light capacity exceeds u64".to_string())?,
        ));
        for kind in generated.light_kinds {
            frame_bounds.push(crate::eval::value::Value::Usize(
                u64::try_from(kind)
                    .map_err(|_| "P015: generated light kind tag exceeds u64".to_string())?,
            ));
        }
        if frame_bounds.len() != RENDERER_FRAME_BOUNDS_WORDS {
            return Err(format!(
                "pixels::glue: generated frame bounds have {} values, expected {}",
                frame_bounds.len(),
                RENDERER_FRAME_BOUNDS_WORDS,
            ));
        }
        let coordinator_args = vec![
            integer_arg("core", 0),
            integer_arg("mailbox", mailbox),
            crate::eval::image::DeclArg {
                label: "workers".to_string(),
                ty: crate::sema::types::Type::Named("RendererWorkers".to_string(), Vec::new()),
                value: crate::eval::value::Value::Struct(worker_handles),
                span: crate::syntax::ast::Span::default(),
            },
            u32_arg(
                "worker_count",
                u32::try_from(generated.workers.len())
                    .map_err(|_| "P015: generated worker count exceeds u32".to_string())?,
            ),
            handle_arg(
                "display",
                crate::sema::types::Type::Usize,
                crate::eval::image::ImageDeclRef::Driver(generated.display_index),
            ),
            u32_arg(
                "renderer_index",
                u32::try_from(renderer_index)
                    .map_err(|_| "P015: renderer index exceeds u32".to_string())?,
            ),
            integer_arg("frameprog_base", 0),
            integer_arg("state_base", 0),
            integer_arg("state_bytes", 0),
            crate::eval::image::DeclArg {
                label: "bounds".to_string(),
                ty: crate::sema::types::Type::Named("RendererFrameBounds".to_string(), Vec::new()),
                value: crate::eval::value::Value::Struct(frame_bounds),
                span: crate::syntax::ast::Span::default(),
            },
        ];
        graph.actors.push(crate::eval::image::ActorDecl {
            actor_type: renderer_decl.actor_type.clone(),
            args: coordinator_args,
        });
        for worker in &generated.workers {
            graph.actors.push(crate::eval::image::ActorDecl {
                actor_type: renderer_worker_type(),
                args: vec![
                    integer_arg(
                        "core",
                        u64::try_from(worker.core)
                            .map_err(|_| "pixels::glue: worker core exceeds u64")?,
                    ),
                    integer_arg("mailbox", 1),
                    u32_arg(
                        "renderer_index",
                        u32::try_from(renderer_index)
                            .map_err(|_| "P015: renderer index exceeds u32".to_string())?,
                    ),
                    integer_arg("frameprog_base", 0),
                    integer_arg("workspace_base", 0),
                    integer_arg("workspace_bytes", 0),
                    u32_arg("tiles_start", worker.tiles_start),
                    u32_arg("tiles_end", worker.tiles_end),
                ],
            });
        }
    }
    for actor in &graph.actors {
        for argument in &actor.args {
            fn has_renderer(value: &crate::eval::value::Value) -> bool {
                use crate::eval::image::ImageDeclRef;
                use crate::eval::value::Value;
                match value {
                    Value::ImageDecl(ImageDeclRef::Renderer(_)) => true,
                    Value::Tuple(values)
                    | Value::Array(values)
                    | Value::Struct(values)
                    | Value::Enum(_, values) => values.iter().any(has_renderer),
                    Value::Closure { env, .. } => {
                        env.iter().any(|scope| scope.values().any(has_renderer))
                    }
                    _ => false,
                }
            }
            if has_renderer(&argument.value) {
                return Err(
                    "pixels::glue: unresolved renderer declaration reference after synthesis"
                        .to_string(),
                );
            }
        }
    }
    Ok(graph)
}

fn set_generated_arg(
    actor: &mut crate::eval::image::ActorDecl,
    label: &str,
    value: crate::eval::value::Value,
) -> Result<(), String> {
    let argument = actor
        .args
        .iter_mut()
        .find(|argument| argument.label == label)
        .ok_or_else(|| format!("pixels::glue: generated actor has no `{label}` argument"))?;
    argument.value = value;
    Ok(())
}

pub fn bind_image_graph_placements(
    graph: &mut crate::eval::image::ImageGraph,
    renderers: &[GeneratedRenderer],
    placements: &[crate::layout::RendererPlacement],
) -> Result<(), String> {
    if renderers.len() != placements.len() {
        return Err("pixels::glue: renderer/placement count differs".to_string());
    }
    let generated_actor_count = renderers.iter().try_fold(0_usize, |count, renderer| {
        count
            .checked_add(1 + renderer.workers.len())
            .ok_or_else(|| "P015: generated actor count overflow".to_string())
    })?;
    let mut actor_index = graph
        .actors
        .len()
        .checked_sub(generated_actor_count)
        .ok_or_else(|| "pixels::glue: generated actor suffix is truncated".to_string())?;
    for (renderer, placement) in renderers.iter().zip(placements) {
        if renderer.renderer_index != placement.index
            || renderer.workers.len() != placement.per_core.len()
        {
            return Err("pixels::glue: generated renderer placement identity differs".to_string());
        }
        let coordinator = graph
            .actors
            .get_mut(actor_index)
            .ok_or_else(|| "pixels::glue: coordinator actor is missing".to_string())?;
        set_generated_arg(
            coordinator,
            "frameprog_base",
            crate::eval::value::Value::Usize(placement.frameprog_base),
        )?;
        set_generated_arg(
            coordinator,
            "state_base",
            crate::eval::value::Value::Usize(placement.state_base),
        )?;
        set_generated_arg(
            coordinator,
            "state_bytes",
            crate::eval::value::Value::Usize(placement.state_size),
        )?;
        actor_index += 1;
        for worker in &placement.per_core {
            let actor = graph
                .actors
                .get_mut(actor_index)
                .ok_or_else(|| "pixels::glue: worker actor is missing".to_string())?;
            set_generated_arg(
                actor,
                "frameprog_base",
                crate::eval::value::Value::Usize(placement.frameprog_base),
            )?;
            set_generated_arg(
                actor,
                "workspace_base",
                crate::eval::value::Value::Usize(worker.workspace_base),
            )?;
            set_generated_arg(
                actor,
                "workspace_bytes",
                crate::eval::value::Value::Usize(worker.workspace_bytes),
            )?;
            actor_index += 1;
        }
    }
    if actor_index != graph.actors.len() {
        return Err("pixels::glue: generated actor suffix has trailing actors".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn canonical_wrela_wire_views_match_machine_sizes_and_offsets() {
        let (_, loaded) = crate::loader::load_render_program_module()
            .unwrap_or_else(|_| panic!("load render_program"));
        let module = crate::sema::specialize::specialize(&loaded.module).expect("specialize");
        let layouts = crate::sema::types::check_layouts(&module).expect("layout views");
        let fields = |name: &str| {
            let layout = layouts
                .iter()
                .find(|layout| layout.name == name)
                .unwrap_or_else(|| panic!("missing layout {name}"));
            let fields = layout
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    crate::sema::types::LayoutEntry::Field(field) => {
                        Some((field.name.as_str(), field.offset, field.size))
                    }
                    crate::sema::types::LayoutEntry::Padding { .. } => None,
                })
                .collect::<Vec<_>>();
            (layout.size.expect("fixed layout size"), fields)
        };
        assert_eq!(
            fields("FrameProgramHeaderV1"),
            (
                u64::from(wrela_machine::pixels::FRAME_PROGRAM_HEADER_BYTES_V1),
                vec![
                    ("magic", 0, 8),
                    ("version", 8, 2),
                    ("header_bytes", 10, 2),
                    ("flags", 12, 4),
                    ("total_bytes", 16, 4),
                    ("renderer_index", 20, 2),
                    ("reserved0", 22, 2),
                    ("numeric_revision", 24, 4),
                    ("formal_revision", 28, 4),
                    ("table_count", 32, 2),
                    ("reserved1", 34, 14),
                    ("digest", 48, 32),
                ],
            )
        );
        assert_eq!(
            fields("FrameProgramTableV1").0,
            u64::from(wrela_machine::pixels::FRAME_PROGRAM_TABLE_BYTES_V1)
        );
        assert_eq!(
            fields("FrameProgramRecordV1").0,
            u64::from(wrela_machine::pixels::FRAME_PROGRAM_RECORD_BYTES_V1)
        );
        assert_eq!(
            fields("FrameProgramImmediateV1").0,
            u64::from(wrela_machine::pixels::FRAME_PROGRAM_IMMEDIATE_BYTES_V1)
        );
    }

    #[test]
    fn bootstrap_census_comes_from_populated_verified_record_tables() {
        let program = super::super::program::minimal_verified_frame_program();
        assert_eq!(
            super::bootstrap_families(&program)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![
                "camera-light-post",
                "csg",
                "feature",
                "field",
                "fixed-domain",
                "object",
                "scalar"
            ]
        );
    }

    use super::*;

    #[test]
    fn tile_partition_is_half_open_complete_and_disjoint() {
        let tile_count = 17_u32;
        let workers = 4_u32;
        let ranges = (0..workers)
            .map(|worker| {
                (
                    u64::from(tile_count) * u64::from(worker) / u64::from(workers),
                    u64::from(tile_count) * u64::from(worker + 1) / u64::from(workers),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, u64::from(tile_count));
        assert!(ranges.windows(2).all(|pair| pair[0].1 == pair[1].0));
    }

    #[test]
    fn worker_handle_padding_is_uniform_and_never_names_an_absent_actor() {
        for worker_count in [1, 2, wrela_machine::CORE_SLOTS] {
            let first = 17;
            let handles = padded_worker_handles(first, worker_count).unwrap();
            assert_eq!(handles.len(), wrela_machine::CORE_SLOTS);
            for (slot, handle) in handles.iter().enumerate() {
                let crate::eval::value::Value::ImageDecl(crate::eval::image::ImageDeclRef::Actor(
                    actor,
                )) = handle
                else {
                    panic!("worker handle is not an actor")
                };
                assert_eq!(*actor, first + slot.min(worker_count - 1));
            }
        }
        assert_eq!(
            renderer_worker_type(),
            crate::sema::types::Type::Named("RendererWorker".to_string(), Vec::new())
        );
    }

    #[test]
    fn placement_binding_gives_each_generated_actor_exact_addresses() {
        let mut graph = crate::eval::image::ImageGraph::default();
        let actor = |ty: crate::sema::types::Type, labels: &[&str]| crate::eval::image::ActorDecl {
            actor_type: ty,
            args: labels.iter().map(|label| integer_arg(label, 0)).collect(),
        };
        graph.actors.push(actor(
            crate::sema::types::Type::Named("Renderer".to_string(), Vec::new()),
            &["frameprog_base", "state_base", "state_bytes"],
        ));
        graph.actors.push(actor(
            renderer_worker_type(),
            &["frameprog_base", "workspace_base", "workspace_bytes"],
        ));
        let generated = GeneratedRenderer {
            renderer_index: 0,
            coordinator: "coordinator".to_string(),
            display_index: 0,
            workers: vec![GeneratedWorker {
                actor: "worker".to_string(),
                core: 0,
                tiles_start: 0,
                tiles_end: 8,
            }],
            exposure_range: [-1.0, 1.0],
            environment_min: [0.0; 3],
            environment_max: [1.0; 3],
            camera_bounds: [[-1.0, 1.0]; 12],
            light_capacity: 0,
            light_kinds: [0; 8],
            rooted_functions: Vec::new(),
            bootstrap_families: Vec::new(),
        };
        let placement = crate::layout::RendererPlacement {
            index: 0,
            frameprog_base: 0x4055_0000,
            frameprog_size: 4096,
            state_base: 0x4056_0000,
            state_size: 8192,
            coordinator_actor: "coordinator".to_string(),
            coordinator_core: 0,
            per_core: vec![crate::layout::RendererCorePlacement {
                worker_index: 0,
                core: 0,
                actor: "worker".to_string(),
                tiles_start: 0,
                tiles_end: 8,
                workspace_base: 0x4056_1000,
                workspace_bytes: 1024,
            }],
            framebuffer_base: 0x4056_2000,
            framebuffer_bytes: 4096,
            probe_base: 0,
            probe_bytes: 0,
        };
        bind_image_graph_placements(&mut graph, &[generated], &[placement]).unwrap();
        let value = |actor: usize, label: &str| {
            graph.actors[actor]
                .args
                .iter()
                .find(|argument| argument.label == label)
                .map(|argument| argument.value.clone())
                .unwrap()
        };
        assert_eq!(
            value(0, "frameprog_base"),
            crate::eval::value::Value::Usize(0x4055_0000)
        );
        assert_eq!(
            value(0, "state_base"),
            crate::eval::value::Value::Usize(0x4056_0000)
        );
        assert_eq!(
            value(1, "workspace_base"),
            crate::eval::value::Value::Usize(0x4056_1000)
        );
    }
}
