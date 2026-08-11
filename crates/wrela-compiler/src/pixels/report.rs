//! Stable P3 report facts shared by dumps and later image reports.

use std::fmt::Write as _;

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

fn feature_source_label(
    graph: &super::symbolic::SymbolicGraph,
    structural: &super::verify::StructuralProgram,
    feature: super::ids::FeatureId,
) -> Result<(super::ids::FieldId, String), String> {
    let feature = structural
        .features
        .get(feature.index())
        .ok_or_else(|| format!("pixels::report: competition names missing feature {feature}"))?;
    let origin = graph.fields.origin(feature.primitive)?;
    let site = &origin.primary;
    Ok((
        feature.primitive,
        format!(
            "{}:{}:{}@bytes={}..{}",
            site.module, site.span.line, site.span.col, site.span.byte_start, site.span.byte_end,
        ),
    ))
}

pub fn append_program_set(
    output: &mut String,
    program_set: &super::PixelsProgramSet,
) -> Result<(), String> {
    if program_set.symbolic_graphs.len() != program_set.structural_programs.len()
        || program_set.projective_programs.len() != program_set.structural_programs.len()
    {
        return Err(format!(
            "pixels::report: renderer table counts differ: symbolic={} structural={} projective={}",
            program_set.symbolic_graphs.len(),
            program_set.structural_programs.len(),
            program_set.projective_programs.len(),
        ));
    }
    if !program_set.compiled_renderers.is_empty()
        && program_set.compiled_renderers.len() != program_set.structural_programs.len()
    {
        return Err(format!(
            "pixels::report: compiled renderer count {} differs from structural count {}",
            program_set.compiled_renderers.len(),
            program_set.structural_programs.len()
        ));
    }
    output.push_str("PixelsCompilerReport v3\n");
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
        let projective = program_set.projective_programs[index].program();
        // Whether any analytic coverage tier admits itself by comparing the
        // *runtime* camera against a fixed pose. Such a tier is fail-closed —
        // a scene that leaves the pose falls back to the arrangement walk
        // rather than producing a wrong byte — but it falls back per frame,
        // and until this line existed nothing at build time said so, which is
        // exactly what made the cost cliff invisible. `pose_conditional=yes`
        // means: this renderer has a tier whose availability the compile-time
        // cost model cannot see. Proving the pose at seal time needs a pinned
        // camera in the renderer declaration, which is a contract change and
        // is tracked to P12 rather than assumed here.
        let pinned_pose = program_set
            .compiled_renderers
            .get(index)
            .is_some_and(|renderer| renderer.config.camera_pose.is_some());
        let pose_conditional_tiers = if pinned_pose {
            // The declaration pins the camera and frame validation enforces
            // it, so the runtime pose test is proved rather than gambled on:
            // no tier here is conditional.
            0
        } else {
            projective
                .events
                .generators
                .iter()
                .filter(|event| {
                    program_set
                        .compiled_renderers
                        .get(index)
                        .is_some_and(|renderer| {
                            super::glue::is_standard_torus_event(renderer, event).unwrap_or(false)
                        })
                })
                .count()
        };
        output.push_str(&format!(
            "    AnalyticTiers pose_conditional={} camera_pinned={}\n",
            pose_conditional_tiers,
            u8::from(pinned_pose),
        ));
        let capacities = &projective.capacities;
        output.push_str(&format!(
            "    Projective features={} polynomials={} coefficients={} derivative_bundles={} derivative_clusters={} events={} competition_pairs={} exclusions={} index_bytes={}\n",
            projective.equations.features.len(),
            capacities.polynomial_programs,
            capacities.coefficient_nodes,
            capacities.derivative_bundles,
            capacities.derivative_clusters,
            capacities.event_generators,
            projective.competitions.pairs.len(),
            projective.exclusions.records.len(),
            capacities.index_bytes,
        ));
        output.push_str(&format!(
            "    ProjectiveCapacities candidate_features_per_tile={} row_start_roots={} active_sheets_per_row={} competition_pairs_per_tile={} row_event_intervals={} root_stack_nodes={} event_stack_nodes={} runs_per_row={} corridors_per_row={} max_index_slice={} polynomial_terms_per_program={} refined_per_worker_scratch_bytes={} refined_all_worker_scratch_bytes={} final_per_worker_scratch_bytes={} final_all_worker_scratch_bytes={} final_state_bytes={} final_instrumented_state_bytes={}\n",
            capacities.candidate_features_per_tile,
            capacities.row_start_roots,
            capacities.active_sheets_per_row,
            capacities.competition_pairs_per_tile,
            capacities.row_event_intervals,
            capacities.root_stack_nodes,
            capacities.event_stack_nodes,
            capacities.runs_per_row,
            capacities.corridors_per_row,
            capacities.max_index_slice,
            capacities.polynomial_terms_per_program,
            capacities.per_worker_scratch_bytes,
            capacities.all_worker_scratch_bytes,
            capacities.final_per_worker_scratch_bytes,
            capacities.final_all_worker_scratch_bytes,
            capacities.total_renderer_state_bytes,
            capacities.total_renderer_state_bytes_instrumented,
        ));
        output.push_str(&format!(
            "    CompetitionPruning projected={} q={} csg_global={} csg_pair={} strict_order={} same_feature={} material_only={}\n",
            projective.competitions.pruned_projected,
            projective.competitions.pruned_q,
            projective.competitions.pruned_csg_global,
            projective.competitions.pruned_csg_pair,
            projective.competitions.pruned_strict_order,
            projective.competitions.suppressed_same_feature,
            projective.competitions.suppressed_material_only,
        ));
        let graph = &program_set.symbolic_graphs[index];
        let structural = verified.program();
        for exclusion in projective.exclusions.records.iter().filter(|record| {
            matches!(
                record.subject,
                super::exclusions::ExclusionSubject::Competition(_)
            )
        }) {
            let super::exclusions::ExclusionSubject::Competition(subject) = exclusion.subject
            else {
                unreachable!()
            };
            let (a_primitive, a_source) = feature_source_label(graph, structural, subject.a)?;
            let (b_primitive, b_source) = feature_source_label(graph, structural, subject.b)?;
            output.push_str(&format!(
                "    CompetitionOmission a={} a_primitive={} a_source={} b={} b_primitive={} b_source={} exclusion={} reason={} proof={} domain={} dependencies=[{}] margin=[{},{}]\n",
                subject.a,
                a_primitive,
                a_source,
                subject.b,
                b_primitive,
                b_source,
                exclusion.id,
                exclusion.reason.stable_name(),
                exclusion.proof,
                exclusion.domain,
                exclusion
                    .dependencies
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                exclusion.margin.lo,
                exclusion.margin.hi,
            ));
        }
        for derivation in &capacities.derivations {
            output.push_str(&format!(
                "    ProjectiveCapacity field={} value={} why=[{}]\n",
                derivation.field,
                derivation.value,
                derivation.why.join("; "),
            ));
        }
        if let Some(compiled) = program_set.compiled_renderers.get(index) {
            let program = compiled.program.program();
            let wire = super::binary_verify::verify_envelope(&compiled.encoded)
                .map_err(|error| format!("pixels::report: invalid encoded program: {error}"))?;
            let digest = compiled.encoded[super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
                ..super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
                    + super::version::FRAME_PROGRAM_DIGEST_BYTES_V1]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            output.push_str(&format!(
                "    FrameProgram version={} profile_revision={} numeric_revision={} \
                 formal_revision={} formal_name={} wire_bytes={} digest={} rich_records={} \
                 rich_operands={}\n",
                super::version::FRAME_PROGRAM_VERSION_V1,
                super::version::FRAME_PROGRAM_PROFILE_REVISION_V1,
                program.numeric_revision,
                program.formal_revision,
                super::version::FRAME_PROGRAM_FORMAL_REVISION_STR_V1,
                compiled.encoded.len(),
                digest,
                program
                    .tables
                    .iter()
                    .map(|table| table.records.len())
                    .sum::<usize>(),
                program
                    .tables
                    .iter()
                    .flat_map(|table| &table.records)
                    .map(|record| record.operands.len())
                    .sum::<usize>(),
            ));
            for table in wire {
                output.push_str(&format!(
                    "    WireTable kind={} count={} record_bytes={} offset={:#x} bytes={}\n",
                    table.kind.stable_name(),
                    table.count,
                    table.record_bytes,
                    table.offset,
                    table.byte_len,
                ));
            }
            output.push_str(&format!(
                "    MutableState production_bytes={} instrumented_bytes={} header={} \
                 coefficients={} frame_inputs={} frame_complex={} worker_scratch={} \
                 framebuffers={} probes={} tile_metadata={} failure={} telemetry_production={} \
                 telemetry_instrumented={}\n",
                compiled.mutable_layout.total_bytes,
                compiled.mutable_layout.instrumented_total_bytes,
                compiled.mutable_layout.header.bytes,
                compiled.mutable_layout.coefficient_snapshots.bytes,
                compiled.mutable_layout.frame_snapshots.bytes,
                compiled.mutable_layout.frame_complexes.bytes,
                compiled.mutable_layout.worker_scratch.bytes,
                compiled.mutable_layout.framebuffers.bytes,
                compiled.mutable_layout.probes.bytes,
                compiled
                    .mutable_layout
                    .tile_descriptors
                    .bytes
                    .checked_add(compiled.mutable_layout.tile_ownership.bytes)
                    .ok_or_else(|| "pixels::report: tile metadata byte overflow".to_string())?,
                compiled.mutable_layout.failure.bytes,
                compiled
                    .structural
                    .program()
                    .capacities
                    .telemetry_bytes_production,
                compiled.mutable_layout.telemetry.bytes,
            ));
            output.push_str(&format!(
                "    Generated coordinator={} workers={} mailbox_capacity={} execution={} presentation=none palette=debug-identity-q families=[{}]\n",
                compiled.generated.coordinator,
                compiled.generated.workers.len(),
                compiled.generated.workers.len() + 2,
                super::DEBUG_VISIBILITY_PATH,
                compiled.generated.bootstrap_families.join(","),
            ));
            for worker in &compiled.generated.workers {
                output.push_str(&format!(
                    "    GeneratedWorker actor={} core={} tiles=[{},{}) mailbox_capacity=1\n",
                    worker.actor, worker.core, worker.tiles_start, worker.tiles_end,
                ));
            }
            output.push_str(&format!(
                "    ForceRoots keys=[{}]\n",
                compiled.generated.rooted_functions.join(","),
            ));
        }
    }
    Ok(())
}

pub fn append_layout(
    output: &mut String,
    program_set: &super::PixelsProgramSet,
    layout: &crate::layout::ImageLayout,
    instrumented: bool,
) -> Result<(), String> {
    if layout.renderers.len() != program_set.compiled_renderers.len() {
        return Err(format!(
            "pixels::report: layout renderer count {} differs from compiled count {}",
            layout.renderers.len(),
            program_set.compiled_renderers.len()
        ));
    }
    if layout.renderers.is_empty() {
        return Ok(());
    }
    output.push_str("PixelsImageContract v1\n");
    let config_source = super::glue::configuration_source(
        &layout.renderers,
        &program_set.compiled_renderers,
        instrumented,
    )?;
    super::glue::parse_configuration_source(&config_source)?;
    output.push_str(&format!(
        "  GeneratedModule address=core.__image_pixels bytes={} digest={}\n",
        config_source.len(),
        wrela_machine::sha256::sha256_hex(config_source.as_bytes()),
    ));
    for (placement, compiled) in layout.renderers.iter().zip(&program_set.compiled_renderers) {
        let start = usize::try_from(
            placement
                .frameprog_base
                .checked_sub(wrela_machine::layout::IMAGE_BASE)
                .ok_or_else(|| "pixels::report: frame program below image base".to_string())?,
        )
        .map_err(|_| "pixels::report: frame-program offset exceeds usize".to_string())?;
        let size = usize::try_from(placement.frameprog_size)
            .map_err(|_| "pixels::report: frame-program size exceeds usize".to_string())?;
        let end = start
            .checked_add(size)
            .ok_or_else(|| "pixels::report: frame-program blob end overflow".to_string())?;
        let bytes = layout
            .blob
            .get(start..end)
            .ok_or_else(|| "pixels::report: frame-program placement outside blob".to_string())?;
        let decoded = super::decode::decode(bytes)
            .map_err(|error| format!("pixels::report: placed frame program invalid: {error}"))?;
        if decoded.program() != compiled.program.program() {
            return Err("pixels::report: placed frame program differs from rich model".to_string());
        }
        let config = &compiled.config;
        let graph = program_set
            .symbolic_graphs
            .get(placement.index)
            .ok_or_else(|| "pixels::report: renderer has no symbolic graph".to_string())?;
        let structural = compiled.structural.program();
        let projective = compiled.projective.program();
        let frame_program_digest = bytes[super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
            ..super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
                + super::version::FRAME_PROGRAM_DIGEST_BYTES_V1]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let tone_lut = super::reference::display::sealed_tone_lut(&config.tone_curve)
            .ok_or_else(|| "pixels::report: sealed tone LUT is absent".to_string())?;
        let mut tone_transfer_bytes =
            format!("pixels-tone-transfer-v1\0{}\0", config.tone_curve).into_bytes();
        for value in tone_lut
            .iter()
            .chain(super::reference::display::SRGB_TRANSFER_LUT.iter())
        {
            tone_transfer_bytes.extend(value.to_le_bytes());
        }
        let tone_transfer_digest = wrela_machine::sha256::sha256_hex(&tone_transfer_bytes);
        let mut renderer_layout_identity = format!(
            "pixels-renderer-layout-v1\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
            placement.index,
            placement.frameprog_base,
            placement.frameprog_size,
            placement.state_base,
            placement.state_size,
            placement.framebuffer_base,
            placement.framebuffer_bytes,
            placement.probe_base,
            placement.probe_bytes,
            placement.coordinator_core,
        );
        for worker in &placement.per_core {
            write!(
                renderer_layout_identity,
                "\0{}\0{}\0{}\0{}\0{}\0{}",
                worker.worker_index,
                worker.core,
                worker.tiles_start,
                worker.tiles_end,
                worker.workspace_base,
                worker.workspace_bytes,
            )
            .expect("String writes cannot fail");
        }
        let renderer_layout_digest =
            wrela_machine::sha256::sha256_hex(renderer_layout_identity.as_bytes());
        output.push_str(&format!(
            "  Renderer index={} profile={} frameprog_base={:#x} frameprog_bytes={} frameprog_blob_digest={} \
             state_base={:#x} state_reservation_bytes={} framebuffer_base={:#x} \
             framebuffer_bytes={} probe_base={:#x} probe_bytes={} coordinator={}@core{} \
             execution={} presentation=none\n",
            placement.index,
            config.profile,
            placement.frameprog_base,
            placement.frameprog_size,
            wrela_machine::sha256::sha256_hex(bytes),
            placement.state_base,
            placement.state_size,
            placement.framebuffer_base,
            placement.framebuffer_bytes,
            placement.probe_base,
            placement.probe_bytes,
            placement.coordinator_actor,
            placement.coordinator_core,
            super::DEBUG_VISIBILITY_PATH,
        ));
        output.push_str(&format!(
            "    Field key={} material_key={} display_ref=driver#{}\n\
             \x20   Mode width={} height={} refresh_hz={} shade_hz={} tone_curve={}\n\
             \x20   Capacity objects={} features={} events={} sheets_per_row={} \
             runs_per_row={} transparent_layers={}\n\
             \x20   Formal contract={} numeric_revision={} formal_revision={}\n\
             \x20   Execution from_scratch_sweep=true bounded_local_rebuild=true \
             dense_frame=false previous_state=false oracle_runtime=false \
             debug_visibility=true presentation=false\n\
             \x20   BuildIdentity frame_program_digest={} tone_transfer_digest={} \
             profile_revision={} numeric_revision={} formal_theorem_set={} \
             renderer_layout_digest={}\n",
            graph.field_key,
            graph.material_key,
            config.display_index,
            config.width,
            config.height,
            config.refresh_hz,
            config.shade_hz,
            config.tone_curve,
            structural.capacities.object_count,
            structural.capacities.feature_count,
            projective.events.generators.len(),
            projective.capacities.active_sheets_per_row,
            projective.capacities.runs_per_row,
            structural.capacities.max_transparent_layers,
            super::version::FRAME_PROGRAM_FORMAL_REVISION_STR_V1,
            super::version::FRAME_PROGRAM_NUMERIC_REVISION_V1,
            super::version::FRAME_PROGRAM_FORMAL_REVISION_V1,
            frame_program_digest,
            tone_transfer_digest,
            super::version::FRAME_PROGRAM_PROFILE_REVISION_V1,
            super::version::FRAME_PROGRAM_NUMERIC_REVISION_V1,
            super::version::FRAME_PROGRAM_FORMAL_REVISION_STR_V1,
            renderer_layout_digest,
        ));
        for worker in &placement.per_core {
            output.push_str(&format!(
                "    Worker index={} actor={} core={} tiles=[{},{}) workspace_base={:#x} \
                 workspace_bytes={}\n",
                worker.worker_index,
                worker.actor,
                worker.core,
                worker.tiles_start,
                worker.tiles_end,
                worker.workspace_base,
                worker.workspace_bytes,
            ));
        }
        output.push_str(&format!(
            "    CertificateTelemetry version={} decision_input=false merge_order=tile-id \
             counters_per_worker={} production_bytes=0 instrumented_bytes={}\n\
             \x20     Schema run_length_bins=8 root_methods=[0:bernstein-faces,1:monotone-tube,2:krawczyk] \
             composition_shapes=[0:general,1:plane,2:sphere,3:torus]\n\
             \x20     Schema expiry_causes=[0:domain-end,1:residual,2:validity,3:order,4:branch,5:numeric,6:fixed-q,7:event] \
             margin_owners=[0:root,1:feature,2:order,3:csg,4:branch,5:numeric,6:fixed-q,7:event]\n\
             \x20     Schema density_bins=8 subdivision_bins=16 \
             rebuild_reasons=[0:none,1:x-split,2:q-split,3:feature-split,4:branch-split,5:event-arrangement,6:pixel-cell,7:subpixel-integration,8:exhausted] \
             numeric_failure_bins=8 pixel_classes=[regular,corridor]\n",
            super::reference::telemetry::CERTIFICATE_TELEMETRY_VERSION,
            super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V1,
            compiled.mutable_layout.telemetry.bytes,
        ));
    }
    Ok(())
}
