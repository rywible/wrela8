//! Stable P3 report facts shared by dumps and later image reports.

use super::capacities::StructuralCapacities;
use super::params::ParameterLayout;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuralReport {
    pub coefficient_bytes: u32,
    pub object_count: u32,
    pub feature_count: u32,
    pub renderer_state_bytes: u64,
    pub renderer_state_bytes_instrumented: u64,
    pub dependency_schema_digest: String,
}

pub fn build(params: &ParameterLayout, capacities: &StructuralCapacities) -> StructuralReport {
    StructuralReport {
        coefficient_bytes: params.packed_bytes,
        object_count: capacities.object_count,
        feature_count: capacities.feature_count,
        renderer_state_bytes: capacities.total_renderer_state_bytes,
        renderer_state_bytes_instrumented: capacities.total_renderer_state_bytes_instrumented,
        dependency_schema_digest: params.digest_schema.schema_digest.clone(),
    }
}

pub fn append_program_set(output: &mut String, program_set: &super::PixelsProgramSet) {
    output.push_str("PixelsStructuralReport v1\n");
    output.push_str(&format!(
        "  Renderers count={}\n",
        program_set.structural_programs.len()
    ));
    for (index, verified) in program_set.structural_programs.iter().enumerate() {
        let report = &verified.program().report;
        let capacities = &verified.program().capacities;
        output.push_str(&format!(
            "  Renderer index={} coefficient_bytes={} objects={} features={} production_state_bytes={} instrumented_state_bytes={} dependency_schema={}\n",
            index,
            report.coefficient_bytes,
            report.object_count,
            report.feature_count,
            report.renderer_state_bytes,
            report.renderer_state_bytes_instrumented,
            report.dependency_schema_digest,
        ));
        output.push_str(&format!(
            "    Capacities workers={} objects={} feature_templates={} feature_slots={} repeated_instances={} scalar_slots={} derivative_slots={} parameter_slots={} csg_stack={} projected_row={} projected_tile={} row_start_roots={} active_sheets={} event_generators={} event_subdivisions={} event_records={} tile_row_runs={} csg_events={} transparent_layers={} rebuild_queue={}\n",
            capacities.worker_count,
            capacities.object_count,
            capacities.feature_template_count,
            capacities.feature_count,
            capacities.repeated_instance_count,
            capacities.scalar_program_slots,
            capacities.derivative_program_slots,
            capacities.parameter_slots,
            capacities.max_csg_stack,
            capacities.max_projected_features_per_row,
            capacities.max_projected_features_per_tile,
            capacities.max_object_roots_per_row_start,
            capacities.max_active_sheet_records_per_row,
            capacities.event_generator_count,
            capacities.max_event_subdivisions,
            capacities.max_event_records,
            capacities.max_run_records_per_tile_row,
            capacities.max_csg_events_per_row,
            capacities.max_transparent_layers,
            capacities.max_local_rebuild_queue,
        ));
        output.push_str(&format!(
            "    Storage candidate_bytes={} root_bytes={} sheet_bytes={} event_bytes={} run_bytes={} corridor_bytes={} fixed_q_bytes={} shading_bytes={} transparency_bytes={} per_worker_scratch_bytes={} all_worker_scratch_bytes={} output_tile_bytes={} output_double_buffer_bytes={} probe_bytes={} kinetic_bytes={} state_header_bytes={} coefficient_snapshot_bytes={} frame_snapshot_bytes={} frame_complex_double_buffer_bytes={} tile_descriptor_bytes={} tile_ownership_bytes={} failure_record_bytes={} telemetry_production_bytes={} telemetry_instrumented_bytes={}\n",
            capacities.candidate_bytes,
            capacities.root_bytes,
            capacities.sheet_bytes,
            capacities.event_bytes,
            capacities.run_bytes,
            capacities.corridor_bytes,
            capacities.fixed_q_bytes,
            capacities.shading_bytes,
            capacities.transparency_bytes,
            capacities.per_worker_scratch_bytes,
            capacities.all_worker_scratch_bytes,
            capacities.output_tile_bytes,
            capacities.output_double_buffer_bytes,
            capacities.probe_bytes,
            capacities.kinetic_certificate_bytes,
            capacities.state_header_bytes,
            capacities.coefficient_snapshot_bytes,
            capacities.frame_dependency_snapshot_bytes,
            capacities.frame_complex_double_buffer_bytes,
            capacities.tile_descriptor_bytes,
            capacities.tile_ownership_bytes,
            capacities.failure_record_bytes,
            capacities.telemetry_bytes_production,
            capacities.telemetry_bytes_instrumented,
        ));
        for derivation in &capacities.derivations {
            output.push_str(&format!(
                "    Capacity field={} value={} why=[{}]\n",
                derivation.field,
                derivation.value,
                derivation.why.join("; "),
            ));
        }
    }
}
