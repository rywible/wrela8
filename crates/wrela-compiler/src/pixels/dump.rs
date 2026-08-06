//! Stable textual dump boundaries for the Pixels compiler stages.

use super::PlaneSkeleton;
use super::config::{RendererConfig, RendererConfigs};
use super::graph::{FieldKind, Primitive, TransformProgram};
use super::material_graph::{MaterialKind, NormalModel};
use super::scalar::{ProofObligation, ScalarOp};
use super::symbolic::SymbolicGraph;
use super::verify::VerifiedStructuralProgram;

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
    out.push_str("Analysis status=symbolic-only\n");
    out
}

pub fn dump_structural_graphs(
    graphs: &[(
        usize,
        SymbolicGraph,
        VerifiedStructuralProgram,
        super::verify::VerifiedProjectiveProgram,
    )],
    configs: &RendererConfigs,
) -> String {
    let symbolic = graphs
        .iter()
        .map(|(index, graph, _, _)| (*index, graph.clone()))
        .collect::<Vec<_>>();
    let mut out = dump_symbolic_graphs(&symbolic, configs);
    out = out.replace("    Obligation kind=", "    DischargedObligation kind=");
    let suffix = "Analysis status=symbolic-only\n";
    debug_assert!(out.ends_with(suffix));
    out.truncate(out.len() - suffix.len());
    for (index, graph, verified, projective) in graphs {
        let program = verified.program();
        out.push_str(&format!(
            "  StructuralRenderer index={} field_key={} material_key={}\n",
            index, graph.field_key, graph.material_key,
        ));
        out.push_str(&format!(
            "    ParameterLayout slots={} packed_bytes={} dependency_schema={}\n",
            program.params.slots.len(),
            program.params.packed_bytes,
            program.params.digest_schema.schema_digest,
        ));
        let dependencies = &program.params.frame_dependencies;
        out.push_str(&format!(
            "      FrameDependencies runtime_bytes={} camera_contract={:?} lights={}:[{}] environment={:?}:{:?} exposure={:?} post={} ao_version={} probe_version={} output={} phase={}:{}\n",
            dependencies.runtime_bytes,
            dependencies.camera_contract,
            dependencies.light_capacity,
            dependencies.light_kinds.join(","),
            dependencies.environment_min,
            dependencies.environment_max,
            dependencies.exposure,
            dependencies.post_id,
            dependencies.ao_version,
            dependencies.probe_version,
            dependencies.output_mode,
            dependencies.deterministic_frame_phase[0],
            dependencies.deterministic_frame_phase[1],
        ));
        for field in &dependencies.fields {
            out.push_str(&format!(
                "        FrameInput path={} use={:?} type={:?} count={} offset={} runtime={}\n",
                field.path,
                field.use_kind,
                field.scalar_ty,
                field.element_count,
                field.packed_offset,
                field.runtime,
            ));
        }
        for slot in &program.params.slots {
            out.push_str(&format!(
                "      Slot id={} path={} component={} type={:?} range=[{},{}] rate={} immutable={} uses=[{}] offset={}\n",
                slot.id,
                format_path(&slot.path),
                slot.component
                    .map(|component| component.to_string())
                    .unwrap_or_else(|| "scalar".to_string()),
                slot.scalar_ty,
                format_f64(slot.range.min),
                format_f64(slot.range.max),
                slot.rate
                    .map(|rate| format!(
                        "{},{}",
                        format_f64(rate.max_delta),
                        format_f64(rate.max_second_delta)
                    ))
                    .unwrap_or_else(|| "none".to_string()),
                slot.immutable,
                slot.uses
                    .iter()
                    .map(|use_kind| format!("{use_kind:?}"))
                    .collect::<Vec<_>>()
                    .join(","),
                slot.packed_offset,
            ));
        }
        for (id, bound) in &program.values.scalar {
            let derivative = &program.derivatives.scalar[id];
            out.push_str(&format!(
                "    ScalarProof id={} value=[{},{}] value_rule={} world_derivative=[{},{},{}] gradient_norm={} frame_delta={} frame_second_delta={} hessian_norm={} third_norm={} smooth={} derivative_rule={}\n",
                id,
                format_f64(bound.value.lo),
                format_f64(bound.value.hi),
                bound.rule,
                format_f64(derivative.world_components[0]),
                format_f64(derivative.world_components[1]),
                format_f64(derivative.world_components[2]),
                format_f64(derivative.world_gradient_norm),
                derivative
                    .frame_delta
                    .map(format_f64)
                    .unwrap_or_else(|| "unbounded-rate".to_string()),
                derivative
                    .frame_second_delta
                    .map(format_f64)
                    .unwrap_or_else(|| "unbounded-rate".to_string()),
                format_f64(derivative.hessian_norm),
                format_f64(derivative.third_derivative_norm),
                !derivative.nonsmooth,
                derivative.rule,
            ));
            for (param, value) in &derivative.parameter {
                out.push_str(&format!(
                    "      ParameterDerivative param={} upper={}\n",
                    param,
                    format_f64(*value),
                ));
            }
        }
        for (id, bound) in &program.world_bounds.fields {
            out.push_str(&format!(
                "    WorldBound field={} bounds={} rule={} contributors=[{}] pruned={}\n",
                id,
                bound
                    .bounds
                    .map(|bounds| format!(
                        "[{},{},{}]-[{},{},{}]",
                        format_f64(bounds.min[0]),
                        format_f64(bounds.min[1]),
                        format_f64(bounds.min[2]),
                        format_f64(bounds.max[0]),
                        format_f64(bounds.max[1]),
                        format_f64(bounds.max[2]),
                    ))
                    .unwrap_or_else(|| "empty".to_string()),
                bound.rule,
                bound.contributors.join(","),
                bound.pruned_reason.unwrap_or("none"),
            ));
        }
        for (id, support) in &program.support.fields {
            out.push_str(&format!(
                "    Support field={} max=[{},{}] leaves={} path=[{}]\n",
                id,
                format_f64(support.max_budget.lo),
                format_f64(support.max_budget.hi),
                support
                    .leaf_supports
                    .iter()
                    .map(|leaf| format!(
                        "{}:smooth=[{},{}]:deform=[{},{}]:value_to_distance=[{},{}]",
                        leaf.path
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(">"),
                        format_f64(leaf.smooth_budget.lo),
                        format_f64(leaf.smooth_budget.hi),
                        format_f64(leaf.deformation_expand.lo),
                        format_f64(leaf.deformation_expand.hi),
                        format_f64(leaf.value_to_distance.lo),
                        format_f64(leaf.value_to_distance.hi),
                    ))
                    .collect::<Vec<_>>()
                    .join(","),
                support
                    .maximum_path
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ));
            if let Some(gap) = &support.gap_sensitive {
                out.push_str(&format!(
                    "      GapBudget left={} left_negated={} right={} right_negated={} gap_lower={} bulge_upper={} k=[{},{}]\n",
                    gap.left,
                    gap.left_negated,
                    gap.right,
                    gap.right_negated,
                    format_f64(gap.gap_lower),
                    format_f64(gap.bulge_upper),
                    format_f64(gap.k.lo),
                    format_f64(gap.k.hi),
                ));
            }
        }
        for identity in &program.objects.identities {
            out.push_str(&format!(
                "    IdentitySet id={} pairs=[{}]\n",
                identity.id,
                identity
                    .pairs
                    .iter()
                    .map(|pair| format!(
                        "{}::{}=>{}::{}",
                        pair.object.enum_key,
                        pair.object.variant,
                        pair.material.enum_key,
                        pair.material.variant,
                    ))
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        for object in &program.objects.objects {
            out.push_str(&format!(
                "    SmoothObject id={} root={} scalar={} bounds=[{},{},{}]-[{},{},{}] leaves=[{}] support=[{},{}] identity_set={} repeats=[{}]\n",
                object.id,
                object.source_root,
                object.scalar_root,
                format_f64(object.bounds.min[0]),
                format_f64(object.bounds.min[1]),
                format_f64(object.bounds.min[2]),
                format_f64(object.bounds.max[0]),
                format_f64(object.bounds.max[1]),
                format_f64(object.bounds.max[2]),
                object
                    .primitive_occurrences
                    .iter()
                    .map(|path| path
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(">"))
                    .collect::<Vec<_>>()
                    .join(","),
                format_f64(object.support_max.lo),
                format_f64(object.support_max.hi),
                object.identity_set,
                object
                    .repeat_instances
                    .iter()
                    .map(|instance| format!(
                        "{}[{}]:{:?}:{}:[{},{}]",
                        instance.repeat_field,
                        instance
                            .equivalent_fields
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                        instance.axis,
                        instance.index,
                        format_f64(instance.period.lo),
                        format_f64(instance.period.hi),
                    ))
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        out.push_str(&format!(
            "    Csg constant={} instructions=[{}] max_stack={}\n",
            program
                .csg
                .constant
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            format_bounded_debug_list(&program.csg.instructions),
            program.csg.max_stack,
        ));
        for influence in &program.csg.influence {
            out.push_str(&format!(
                "      Influence object={} false=constant:{:?}:stack:{}:instructions:[{}]:digest:{} true=constant:{:?}:stack:{}:instructions:[{}]:digest:{}\n",
                influence.object,
                influence.when_false.constant,
                influence.when_false.max_stack,
                format_bounded_debug_list(&influence.when_false.instructions),
                influence.when_false.digest,
                influence.when_true.constant,
                influence.when_true.max_stack,
                format_bounded_debug_list(&influence.when_true.instructions),
                influence.when_true.digest,
            ));
        }
        for feature in &program.features {
            out.push_str(&format!(
                "    Feature id={} template={} object={} primitive={} path=[{}] kind={:?} bounds=[{},{},{}]-[{},{},{}] support={} validity=[{}] orientation={:?} identity_set={} semantic_root={}\n",
                feature.id,
                feature.template_id,
                feature.object,
                feature.primitive,
                feature
                    .occurrence_path
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(">"),
                feature.kind,
                format_f64(feature.world_bounds.min[0]),
                format_f64(feature.world_bounds.min[1]),
                format_f64(feature.world_bounds.min[2]),
                format_f64(feature.world_bounds.max[0]),
                format_f64(feature.world_bounds.max[1]),
                format_f64(feature.world_bounds.max[2]),
                format_f64(feature.support_expand),
                format!(
                    "{:?}:shared={}",
                    feature.validity.constraints, feature.validity.shared_boundary
                ),
                feature.orientation,
                feature.identity_set,
                feature.scalar_semantic_root,
            ));
        }
        for repeat in &program.repeats {
            out.push_str(&format!(
                "    RepeatTemplate object={} root={} instances={} translations={} wrap_events={} fixed_instance={}\n",
                repeat.object,
                repeat.source_root,
                repeat.instance_count,
                repeat.affine_translation_count,
                repeat.wrap_event_families,
                repeat.certificate_must_fix_instance,
            ));
            for instance in &repeat.instances {
                out.push_str(&format!(
                    "      RepeatInstance object={} translations=[{}]\n",
                    instance.object,
                    instance
                        .translations
                        .iter()
                        .map(|translation| format!(
                            "{}:{:?}:{}:period=[{},{}]:offset=[{},{}]",
                            translation.repeat_field,
                            translation.axis,
                            translation.index,
                            format_f64(translation.period.lo),
                            format_f64(translation.period.hi),
                            format_f64(translation.translation.lo),
                            format_f64(translation.translation.hi),
                        ))
                        .collect::<Vec<_>>()
                        .join(","),
                ));
            }
            for event in &repeat.wrap_events {
                out.push_str(&format!(
                    "      WrapEvent field={} axis={:?} cells={}..{} boundary=[{},{}]\n",
                    event.repeat_field,
                    event.axis,
                    event.left_index,
                    event.right_index,
                    format_f64(event.boundary.lo),
                    format_f64(event.boundary.hi),
                ));
            }
        }
        for deformation in &program.deformations {
            out.push_str(&format!(
                "    Deformation field={} displacement={} derivation={:?} amplitude={} gradient={} hessian={} third={} coordinate_x={} frequency_scalar={} frequency=[{},{}] phase_scalar={} phase=[{},{}]\n",
                deformation.field,
                deformation.displacement,
                deformation.derivation,
                format_f64(deformation.amplitude),
                format_f64(deformation.gradient),
                format_f64(deformation.hessian),
                format_f64(deformation.third_derivative),
                deformation.coordinate_x,
                deformation.frequency_scalar,
                format_f64(deformation.frequency.lo),
                format_f64(deformation.frequency.hi),
                deformation.phase_scalar,
                format_f64(deformation.phase.lo),
                format_f64(deformation.phase.hi),
            ));
        }
        for event in &program.material_events {
            out.push_str(&format!(
                "    MaterialEvent predicate={} kind={:?} crossings={} owners=[{}] features=[{}] source={}:{}:{}\n",
                event.predicate,
                event.kind,
                event.crossing_bound,
                format_id_list(&event.owners),
                format_id_list(&event.feature_owners),
                event.origin.primary.module,
                event.origin.primary.span.line,
                event.origin.primary.span.col,
            ));
        }
        let capacity = &program.capacities;
        out.push_str(&format!(
            "    Capacities workers={} objects={} feature_templates={} feature_slots={} repeated_instances={} scalar_slots={} derivative_slots={} parameter_slots={} csg_stack={} projected_row={} projected_tile={} row_start_roots={} active_sheets={} event_generators={} event_subdivisions={} event_records={} tile_row_runs={} csg_events={} transparent_layers={} rebuild_queue={} candidate_bytes={} root_bytes={} sheet_bytes={} event_bytes={} run_bytes={} corridor_bytes={} fixed_q_bytes={} shading_bytes={} transparency_bytes={} per_worker_scratch_bytes={} all_worker_scratch_bytes={} telemetry_production_bytes={} telemetry_instrumented_bytes={} output_tile_bytes={} output_double_buffer_bytes={} probe_bytes={} kinetic_bytes={} state_header_bytes={} coefficient_snapshot_bytes={} frame_snapshot_bytes={} frame_complex_double_buffer_bytes={} tile_descriptor_bytes={} tile_ownership_bytes={} failure_record_bytes={} production_state_bytes={} instrumented_state_bytes={}\n",
            capacity.worker_count,
            capacity.object_count,
            capacity.feature_template_count,
            capacity.feature_count,
            capacity.repeated_instance_count,
            capacity.scalar_program_slots,
            capacity.derivative_program_slots,
            capacity.parameter_slots,
            capacity.max_csg_stack,
            capacity.max_projected_features_per_row,
            capacity.max_projected_features_per_tile,
            capacity.max_object_roots_per_row_start,
            capacity.max_active_sheet_records_per_row,
            capacity.event_generator_count,
            capacity.max_event_subdivisions,
            capacity.max_event_records,
            capacity.max_run_records_per_tile_row,
            capacity.max_csg_events_per_row,
            capacity.max_transparent_layers,
            capacity.max_local_rebuild_queue,
            capacity.candidate_bytes,
            capacity.root_bytes,
            capacity.sheet_bytes,
            capacity.event_bytes,
            capacity.run_bytes,
            capacity.corridor_bytes,
            capacity.fixed_q_bytes,
            capacity.shading_bytes,
            capacity.transparency_bytes,
            capacity.per_worker_scratch_bytes,
            capacity.all_worker_scratch_bytes,
            capacity.telemetry_bytes_production,
            capacity.telemetry_bytes_instrumented,
            capacity.output_tile_bytes,
            capacity.output_double_buffer_bytes,
            capacity.probe_bytes,
            capacity.kinetic_certificate_bytes,
            capacity.state_header_bytes,
            capacity.coefficient_snapshot_bytes,
            capacity.frame_dependency_snapshot_bytes,
            capacity.frame_complex_double_buffer_bytes,
            capacity.tile_descriptor_bytes,
            capacity.tile_ownership_bytes,
            capacity.failure_record_bytes,
            capacity.total_renderer_state_bytes,
            capacity.total_renderer_state_bytes_instrumented,
        ));
        for derivation in &capacity.derivations {
            out.push_str(&format!(
                "      CapacityDerivation field={} value={} why=[{}]\n",
                derivation.field,
                derivation.value,
                derivation.why.join("; "),
            ));
        }
        out.push_str(&format!(
            "    StructuralReport coefficient_bytes={} objects={} features={} production_state_bytes={} instrumented_state_bytes={} dependency_schema={}\n",
            program.report.coefficient_bytes,
            program.report.object_count,
            program.report.feature_count,
            program.report.renderer_state_bytes,
            program.report.renderer_state_bytes_instrumented,
            program.report.dependency_schema_digest,
        ));
        append_projective_dump(&mut out, projective.program());
    }
    out.push_str("Analysis status=verified-projective\n");
    out
}

fn stable_interval(interval: super::reference::interval::F64Interval) -> String {
    format!("[{},{}]", format_f64(interval.lo), format_f64(interval.hi))
}

fn stable_interval_array(intervals: &[super::reference::interval::F64Interval]) -> String {
    intervals
        .iter()
        .map(|interval| stable_interval(*interval))
        .collect::<Vec<_>>()
        .join(",")
}

fn stable_strict_sign(sign: super::projective::StrictSign) -> &'static str {
    match sign {
        super::projective::StrictSign::Negative => "negative",
        super::projective::StrictSign::Positive => "positive",
    }
}

fn stable_obligation(obligation: super::projective::StrictSignObligation) -> String {
    format!(
        "coefficient:{}:enclosure={}:sign={}",
        obligation.coefficient,
        stable_interval(obligation.enclosure),
        stable_strict_sign(obligation.sign)
    )
}

fn stable_coefficient_op(op: &super::program::CoeffOp) -> String {
    match op {
        super::program::CoeffOp::ConstF64(bits) => {
            format!("const:{}", format_f64(f64::from_bits(*bits)))
        }
        super::program::CoeffOp::Scalar(id) => format!("scalar:{id}"),
        super::program::CoeffOp::Camera(value) => {
            use super::program::CameraCoeff;
            match value {
                CameraCoeff::Eye(component) => format!("camera:eye:{component}"),
                CameraCoeff::Forward(component) => format!("camera:forward:{component}"),
                CameraCoeff::Right(component) => format!("camera:right:{component}"),
                CameraCoeff::Up(component) => format!("camera:up:{component}"),
                CameraCoeff::EyeRate(component) => format!("camera:eye_rate:{component}"),
                CameraCoeff::ForwardRate(component) => {
                    format!("camera:forward_rate:{component}")
                }
                CameraCoeff::RightRate(component) => format!("camera:right_rate:{component}"),
                CameraCoeff::UpRate(component) => format!("camera:up_rate:{component}"),
                CameraCoeff::TanHalfFovY => "camera:tan_half_fov_y".to_string(),
                CameraCoeff::Aspect => "camera:aspect".to_string(),
            }
        }
        super::program::CoeffOp::ScalarParamDerivative(scalar, parameter) => {
            format!("scalar_param_derivative:{scalar}:{parameter}")
        }
        super::program::CoeffOp::ParamRate(parameter, order) => {
            format!("param_rate:{parameter}:{order}")
        }
        super::program::CoeffOp::Add(a, b) => format!("add:{a}:{b}"),
        super::program::CoeffOp::Mul(a, b) => format!("mul:{a}:{b}"),
        super::program::CoeffOp::Neg(value) => format!("neg:{value}"),
    }
}

fn stable_predicate_sense(sense: super::program::PredicateSense) -> &'static str {
    use super::program::PredicateSense;
    match sense {
        PredicateSense::StrictNegative => "strict-negative",
        PredicateSense::NonPositive => "non-positive",
        PredicateSense::EqualZero => "equal-zero",
        PredicateSense::NonNegative => "non-negative",
        PredicateSense::StrictPositive => "strict-positive",
    }
}

fn stable_seed(seed: super::projective::SeedKind) -> String {
    match seed {
        super::projective::SeedKind::Affine { denominator } => {
            format!("affine:{}", stable_obligation(denominator))
        }
        super::projective::SeedKind::StableQuadratic {
            leading_coefficient,
            leading_enclosure,
            leading_sign,
            linear_fallback,
            generic_isolation_fallback,
        } => format!(
            "stable-quadratic:leading={}:enclosure={}:sign={}:linear_fallback={}:generic_fallback={}",
            leading_coefficient,
            stable_interval(leading_enclosure),
            leading_sign.map(stable_strict_sign).unwrap_or("unknown"),
            linear_fallback,
            generic_isolation_fallback,
        ),
        super::projective::SeedKind::GenericIsolatedRoot => "generic-isolated-root".to_string(),
    }
}

fn stable_isolation(isolation: super::projective::RootIsolationProgram) -> String {
    match isolation {
        super::projective::RootIsolationProgram::Affine => "affine".to_string(),
        super::projective::RootIsolationProgram::StableQuadratic {
            linear_fallback,
            generic_isolation_fallback,
        } => format!(
            "stable-quadratic:linear_fallback={linear_fallback}:generic_fallback={generic_isolation_fallback}"
        ),
        super::projective::RootIsolationProgram::CertifiedBernstein {
            maximum_depth,
            ambiguity_depth,
            preserve_all_positive_q_roots,
        } => format!(
            "certified-bernstein:depth={maximum_depth}:ambiguity_depth={ambiguity_depth}:preserve_all_positive_q_roots={preserve_all_positive_q_roots}"
        ),
    }
}

fn stable_orientation(orientation: super::primitive::OrientationProgram) -> &'static str {
    use super::primitive::OrientationProgram;
    match orientation {
        OrientationProgram::Outward => "outward",
        OrientationProgram::Inward => "inward",
        OrientationProgram::DeformedOutward => "deformed-outward",
        OrientationProgram::DeformedInward => "deformed-inward",
    }
}

fn stable_event_kind(kind: super::event_kinds::EventKind) -> &'static str {
    use super::event_kinds::EventKind;
    match kind {
        EventKind::ProjectedBoundEnter => "projected-bound-enter",
        EventKind::ProjectedBoundExit => "projected-bound-exit",
        EventKind::Silhouette => "silhouette",
        EventKind::FeatureBoundary => "feature-boundary",
        EventKind::RepeatBoundary => "repeat-boundary",
        EventKind::SmoothBandEnter => "smooth-band-enter",
        EventKind::SmoothCenterTie => "smooth-center-tie",
        EventKind::MaterialBoundary => "material-boundary",
        EventKind::NearClip => "near-clip",
        EventKind::FarClip => "far-clip",
        EventKind::FixedPointResetOnly => "fixed-point-reset-only",
        EventKind::DepthSwap => "depth-swap",
    }
}

fn stable_event_side(side: super::event_kinds::EventSide) -> &'static str {
    use super::event_kinds::EventSide;
    match side {
        EventSide::Inactive => "inactive",
        EventSide::Active => "active",
        EventSide::OutsideValidity => "outside-validity",
        EventSide::InsideValidity => "inside-validity",
        EventSide::RepeatLeft => "repeat-left",
        EventSide::RepeatRight => "repeat-right",
        EventSide::SmoothLeft => "smooth-left",
        EventSide::SmoothRight => "smooth-right",
        EventSide::IdentityLeft => "identity-left",
        EventSide::IdentityRight => "identity-right",
        EventSide::MaterialLeft => "material-left",
        EventSide::MaterialRight => "material-right",
        EventSide::OutsideClip => "outside-clip",
        EventSide::InsideClip => "inside-clip",
        EventSide::ResetOnly => "reset-only",
        EventSide::DepthAFront => "depth-a-front",
        EventSide::DepthBFront => "depth-b-front",
        EventSide::RecomputeRootSet => "recompute-root-set",
        EventSide::Ambiguous => "ambiguous",
    }
}

fn stable_event_sides(sides: super::event_kinds::EventSideMeaning) -> String {
    format!(
        "negative={}:zero={}:positive={}",
        stable_event_side(sides.negative),
        stable_event_side(sides.zero),
        stable_event_side(sides.positive),
    )
}

fn stable_participant(participant: super::events::Participant) -> String {
    match participant {
        super::events::Participant::Feature(id) => format!("feature:{id}"),
        super::events::Participant::Object(id) => format!("object:{id}"),
        super::events::Participant::Field(id) => format!("field:{id}"),
        super::events::Participant::MaterialEvent(id) => format!("material-event:{id}"),
    }
}

fn stable_axis(axis: super::graph::Axis) -> &'static str {
    match axis {
        super::graph::Axis::X => "x",
        super::graph::Axis::Y => "y",
        super::graph::Axis::Z => "z",
    }
}

fn stable_scalar_derivatives(program: &super::events::ScalarDerivativeProgram) -> String {
    format!(
        "sources=[{}]:first=[{}]:second={}:third={}:params=[{}]:frame_delta={}:frame_second={}",
        program
            .sources
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        program
            .first_world_abs
            .iter()
            .map(|value| format_f64(*value))
            .collect::<Vec<_>>()
            .join(","),
        format_f64(program.second_world_abs),
        format_f64(program.third_world_abs),
        program
            .parameter_abs
            .iter()
            .map(|(parameter, value)| format!("{parameter}:{}", format_f64(*value)))
            .collect::<Vec<_>>()
            .join(","),
        program
            .frame_delta_abs
            .map(format_f64)
            .unwrap_or_else(|| "-".to_string()),
        program
            .frame_second_delta_abs
            .map(format_f64)
            .unwrap_or_else(|| "-".to_string()),
    )
}

fn stable_compare(compare: super::scalar::CompareOp) -> &'static str {
    use super::scalar::CompareOp;
    match compare {
        CompareOp::Lt => "lt",
        CompareOp::Le => "le",
        CompareOp::Gt => "gt",
        CompareOp::Ge => "ge",
        CompareOp::Eq => "eq",
        CompareOp::Ne => "ne",
    }
}

fn stable_event_representation(representation: &super::events::EventRepresentation) -> String {
    use super::events::EventRepresentation;
    match representation {
        EventRepresentation::LinearLeadingCoefficient { coefficient, root } => {
            format!("linear-leading-coefficient:predicate={coefficient}:root={root}")
        }
        EventRepresentation::QuadraticDiscriminant { discriminant, root } => {
            format!("quadratic-discriminant:predicate={discriminant}:root={root}")
        }
        EventRepresentation::SparsePredicate { predicate } => {
            format!("sparse-predicate:{predicate}")
        }
        EventRepresentation::DeformationTaylorPredicate {
            predictor,
            predictor_derivatives,
            displacement,
            scalar_derivatives,
            phase_recurrence,
            taylor_order,
            world_delta_abs_bound,
            third_derivative_abs_bound,
            remainder,
        } => format!(
            "deformation-taylor:predictor={predictor}:derivatives={predictor_derivatives}:displacement={displacement}:scalar={}:coordinate={}:frequency_scalar={}:frequency={}:phase_scalar={}:phase={}:sin=[{}]:cos=[{}]:order={taylor_order}:world_delta={}:third_derivative={}:remainder={}",
            stable_scalar_derivatives(scalar_derivatives),
            phase_recurrence.coordinate_x,
            phase_recurrence.frequency_scalar,
            stable_interval(phase_recurrence.frequency),
            phase_recurrence.phase_scalar,
            stable_interval(phase_recurrence.phase),
            phase_recurrence
                .sine_coefficients
                .iter()
                .map(|bits| format!("0x{bits:016x}"))
                .collect::<Vec<_>>()
                .join(","),
            phase_recurrence
                .cosine_coefficients
                .iter()
                .map(|bits| format!("0x{bits:016x}"))
                .collect::<Vec<_>>()
                .join(","),
            format_f64(*world_delta_abs_bound),
            format_f64(*third_derivative_abs_bound),
            format_f64(*remainder),
        ),
        EventRepresentation::TorusLocalOracle {
            root,
            derivative_u,
            derivative_q,
            derivative_uq,
            derivative_qq,
            third_u,
            value_abs_bound,
            derivative_u_abs_bound,
            derivative_q_abs_bound,
            derivative_uq_abs_bound,
            derivative_qq_abs_bound,
            third_u_abs_bound,
            taylor_order,
            remainder,
        } => format!(
            "torus-local:root={root}:du={derivative_u}:dq={derivative_q}:duq={derivative_uq}:dqq={derivative_qq}:duuu={third_u}:bounds=[{},{},{},{},{},{}]:order={taylor_order}:remainder={}",
            format_f64(*value_abs_bound),
            format_f64(*derivative_u_abs_bound),
            format_f64(*derivative_q_abs_bound),
            format_f64(*derivative_uq_abs_bound),
            format_f64(*derivative_qq_abs_bound),
            format_f64(*third_u_abs_bound),
            format_f64(*remainder),
        ),
        EventRepresentation::SmoothBandTaylorPredicate {
            left,
            right,
            left_negated,
            right_negated,
            radius,
            derivatives,
            taylor_order,
            world_delta_abs_bound,
            remainder,
        } => format!(
            "smooth-band:left={left}:right={right}:negated={left_negated},{right_negated}:radius={radius}:derivatives={}:order={taylor_order}:world_delta={}:remainder={}",
            stable_scalar_derivatives(derivatives),
            format_f64(*world_delta_abs_bound),
            format_f64(*remainder),
        ),
        EventRepresentation::SmoothTieTaylorPredicate {
            left,
            right,
            left_negated,
            right_negated,
            derivatives,
            taylor_order,
            world_delta_abs_bound,
            remainder,
        } => format!(
            "smooth-tie:left={left}:right={right}:negated={left_negated},{right_negated}:derivatives={}:order={taylor_order}:world_delta={}:remainder={}",
            stable_scalar_derivatives(derivatives),
            format_f64(*world_delta_abs_bound),
            format_f64(*remainder),
        ),
        EventRepresentation::MaterialDifferenceTaylorPredicate {
            left,
            right,
            comparison,
            derivatives,
            taylor_order,
            world_delta_abs_bound,
            remainder,
        } => format!(
            "material-difference:left={left}:right={right}:compare={}:derivatives={}:order={taylor_order}:world_delta={}:remainder={}",
            stable_compare(*comparison),
            stable_scalar_derivatives(derivatives),
            format_f64(*world_delta_abs_bound),
            format_f64(*remainder),
        ),
        EventRepresentation::RepeatAffineBoundary { axis, boundary } => format!(
            "repeat-affine:axis={}:boundary={}",
            stable_axis(*axis),
            stable_interval(*boundary)
        ),
        EventRepresentation::ClipQ { q } => format!("clip-q:{}", format_f64(*q)),
        EventRepresentation::ProjectedBoundary {
            horizontal,
            coordinate,
        } => format!("projected-boundary:horizontal={horizontal}:coordinate={coordinate}"),
        EventRepresentation::FixedPointReset => "fixed-point-reset".to_string(),
        EventRepresentation::DirectDepthCrossProduct {
            numerator,
            denominator_a,
            denominator_b,
        } => format!(
            "direct-depth:numerator={numerator}:denominator_a={}:denominator_b={}",
            stable_obligation(*denominator_a),
            stable_obligation(*denominator_b)
        ),
        EventRepresentation::TaylorDepthDifference {
            a,
            b,
            taylor_order,
            remainder,
        } => format!(
            "taylor-depth:a={a}:b={b}:order={taylor_order}:next_order={}:a_third=[{},{},{},{},{},{},{},{},{},{}]:b_third=[{},{},{},{},{},{},{},{},{},{}]:x_domain={}:q_domain={}:fallback_difference={}:fallback_remainder={}:strict_gq={}:discard_taylor_on_fallback={}",
            remainder.next_derivative_order,
            remainder.a_third.uuu,
            remainder.a_third.uuv,
            remainder.a_third.uuq,
            remainder.a_third.uvv,
            remainder.a_third.uvq,
            remainder.a_third.uqq,
            remainder.a_third.vvv,
            remainder.a_third.vvq,
            remainder.a_third.vqq,
            remainder.a_third.qqq,
            remainder.b_third.uuu,
            remainder.b_third.uuv,
            remainder.b_third.uuq,
            remainder.b_third.uvv,
            remainder.b_third.uvq,
            remainder.b_third.uqq,
            remainder.b_third.vvv,
            remainder.b_third.vvq,
            remainder.b_third.vqq,
            remainder.b_third.qqq,
            stable_interval(remainder.local_x_domain),
            stable_interval(remainder.q_domain),
            stable_interval(remainder.fallback_difference),
            format_f64(remainder.fallback_remainder_abs_bound),
            remainder.requires_strict_g_q,
            remainder.discard_taylor_on_fallback,
        ),
    }
}

fn stable_projection_rule(rule: super::projection_bounds::ProjectionRule) -> &'static str {
    use super::projection_bounds::ProjectionRule;
    match rule {
        ProjectionRule::IntervalProjectiveDivision => "interval-projective-division",
        ProjectionRule::EyeOrNearPlaneFullScreen => "eye-or-near-full-screen",
        ProjectionRule::UnboundedPlaneFullScreen => "unbounded-plane-full-screen",
        ProjectionRule::OutsideNearFar => "outside-near-far",
    }
}

fn stable_proven_sign(sign: super::exclusions::ProvenSign) -> &'static str {
    match sign {
        super::exclusions::ProvenSign::Negative => "negative",
        super::exclusions::ProvenSign::Positive => "positive",
    }
}

fn stable_proof_payload(payload: &super::exclusions::ProofPayload) -> String {
    match payload {
        super::exclusions::ProofPayload::PositiveMargin { rule, facts } => {
            format!("positive-margin:rule={rule}:facts=[{}]", facts.join("|"))
        }
        super::exclusions::ProofPayload::Bernstein(payload) => format!(
            "bernstein:box=[{}]:polynomial={}:coefficient_root={}:degrees=[{}]:coefficient_order=[{}]:conversion_radius={}:tree=[{}]:sign={}:minimum_margin={}",
            payload
                .normalized_box
                .iter()
                .map(|axis| format!("[{},{}]", format_f64(axis.lo), format_f64(axis.hi)))
                .collect::<Vec<_>>()
                .join(","),
            payload
                .polynomial
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            payload
                .coefficient_program_root
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            payload
                .degrees
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            payload
                .coefficient_order
                .iter()
                .map(|row| row
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("."))
                .collect::<Vec<_>>()
                .join("|"),
            format_f64(payload.outward_conversion_radius),
            payload
                .subdivision_tree
                .iter()
                .map(|node| format!(
                    "{}:{}:{}:{}:{}",
                    node.path_bits,
                    node.depth,
                    node.split_variable
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    node.sign.map(stable_proven_sign).unwrap_or("unknown"),
                    format_f64(node.margin)
                ))
                .collect::<Vec<_>>()
                .join("|"),
            stable_proven_sign(payload.strict_sign),
            format_f64(payload.minimum_margin),
        ),
    }
}

fn stable_exclusion_reason(reason: super::exclusions::ExclusionReason) -> &'static str {
    reason.stable_name()
}

fn append_projective_dump(out: &mut String, program: &super::verify::ProjectiveProgram) {
    let camera = program.equations.camera;
    out.push_str(&format!(
        "    ProjectiveContract width={} height={} aspect={} tan_half_fov_y={} q=[{},{}] max_motion={}\n",
        camera.width,
        camera.height,
        format_f64(camera.aspect),
        format_f64(camera.tan_half_fov_y),
        format_f64(camera.q.lo),
        format_f64(camera.q.hi),
        format_f64(camera.max_frame_motion),
    ));
    out.push_str(&format!(
        "    CameraRateBounds eye=[{}] forward=[{}] right=[{}] up=[{}]\n",
        stable_interval_array(&camera.eye_rate_component),
        stable_interval_array(&camera.forward_rate_component),
        stable_interval_array(&camera.right_rate_component),
        stable_interval_array(&camera.up_rate_component),
    ));
    for node in &program.equations.coefficients.nodes {
        out.push_str(&format!(
            "    Coefficient id={} op={}\n",
            node.id,
            stable_coefficient_op(&node.op),
        ));
    }
    for polynomial in &program.equations.polynomials {
        out.push_str(&format!(
            "    Polynomial id={} degrees=u{}v{}q{}x{}t{} terms={} detail=exact\n",
            polynomial.id,
            polynomial.degree_u,
            polynomial.degree_v,
            polynomial.degree_q,
            polynomial.degree_x,
            polynomial.degree_t,
            polynomial.terms.len(),
        ));
        for term in &polynomial.terms {
            out.push_str(&format!(
                "      Term u={} v={} q={} x={} t={} params=[{}] coefficient={}\n",
                term.exponents.u,
                term.exponents.v,
                term.exponents.q,
                term.exponents.x,
                term.exponents.t,
                term.exponents
                    .param_terms
                    .iter()
                    .map(|term| format!("{}^{}", term.param, term.exponent))
                    .collect::<Vec<_>>()
                    .join(","),
                term.coefficient,
            ));
        }
    }
    for predicate in &program.equations.predicates {
        out.push_str(&format!(
            "    Predicate id={} polynomial={} sense={} boundary_family={}\n",
            predicate.id,
            predicate.polynomial,
            stable_predicate_sense(predicate.sense),
            predicate.boundary_family,
        ));
    }
    for rational in &program.equations.rationals {
        out.push_str(&format!(
            "    Rational id={} numerator={} denominator={} domain={} proof={}\n",
            rational.id,
            rational.numerator,
            rational.denominator,
            rational.domain,
            stable_obligation(rational.denominator_proof),
        ));
    }
    let mut composition_plans = Vec::<&super::polynomial::CompositionPlan>::new();
    for feature in &program.equations.features {
        if !composition_plans.contains(&&feature.quadratic_composition) {
            composition_plans.push(&feature.quadratic_composition);
        }
    }
    for (id, plan) in composition_plans.iter().enumerate() {
        match plan {
            super::polynomial::CompositionPlan::Specialized(schedule) => {
                out.push_str(&format!(
                    "    CompositionPlan id=cp{} kind=specialized source_degree_u={} source_degree_q={} source_degree_x={} q_hat_degree={} composed_degree={} source_terms={} temporaries={} coefficient_order=[{}]\n",
                    id,
                    schedule.source_degree_u,
                    schedule.source_degree_q,
                    schedule.source_degree_x,
                    schedule.q_hat_degree,
                    schedule.composed_degree,
                    schedule.source_term_count,
                    schedule.temporary_count,
                    schedule
                        .coefficient_order
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                ));
                for step in &schedule.steps {
                    out.push_str(&format!(
                        "      CompositionStep source_term={} u_power={} q_power={} lifted_power_offset={} coefficient_order={}\n",
                        step.source_term,
                        step.u_power,
                        step.q_power,
                        step.lifted_power_offset,
                        step.coefficient_order,
                    ));
                }
                for (face, correction) in schedule.correction_faces.iter().enumerate() {
                    out.push_str(&format!(
                        "      CorrectionFace id={} sign={} output_order=[{}] steps=composition-steps:{}\n",
                        face,
                        correction.correction_sign,
                        correction
                            .output_coefficient_order
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                        correction.steps.len(),
                    ));
                }
            }
            super::polynomial::CompositionPlan::IntervalTaylorFallback {
                source_degree_u,
                source_degree_q,
                source_degree_x,
                composed_degree,
                source_term_count,
            } => out.push_str(&format!(
                "    CompositionPlan id=cp{} kind=interval-taylor-fallback source_degree_u={} source_degree_q={} source_degree_x={} composed_degree={} source_terms={}\n",
                id, source_degree_u, source_degree_q, source_degree_x, composed_degree, source_term_count,
            )),
        }
    }
    for feature in &program.equations.features {
        let composition = composition_plans
            .iter()
            .position(|plan| *plan == &feature.quadratic_composition)
            .expect("projective composition plan was interned");
        out.push_str(&format!(
            "    ProjectiveFeature id={} root={} rational={} q_degree={} seed={} isolation={} composition=cp{} roots={} validity=[{}] orientation={} deformed={} params=[{}]\n",
            feature.feature,
            feature.root_equation,
            feature
                .rational_program
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            feature.q_degree,
            stable_seed(feature.q_seed_kind),
            stable_isolation(feature.root_isolation),
            composition,
            feature.max_root_count,
            feature
                .validity_predicates
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            stable_orientation(feature.orientation_program),
            feature.deformed_predictor,
            feature
                .influencing_params
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ));
    }
    for deformation in &program.deformations {
        out.push_str(&format!(
            "    ProjectiveDeformation feature={} field={} predictor={} residual={} coordinate_x={} frequency_scalar={} frequency=[{},{}] phase_scalar={} phase=[{},{}] bounds=[{},{},{},{}] roots={} taylor_order={} approximation_revision={} folded=[{},{}] remainders=[{},{}] recurrence_sin=[{}] recurrence_cos=[{}] tube={}\n",
            deformation.feature,
            deformation.deformation_field,
            deformation.predictor,
            deformation.residual,
            deformation.coordinate_x,
            deformation.phase_recurrence.frequency_scalar,
            format_f64(deformation.frequency.lo),
            format_f64(deformation.frequency.hi),
            deformation.phase_recurrence.phase_scalar,
            format_f64(deformation.phase.lo),
            format_f64(deformation.phase.hi),
            format_f64(deformation.value_bound),
            format_f64(deformation.first_derivative_bound),
            format_f64(deformation.second_derivative_bound),
            format_f64(deformation.third_derivative_bound),
            deformation.maximum_root_count,
            deformation.taylor_order,
            deformation.approximation.revision,
            format_f64(deformation.approximation.folded_domain.lo),
            format_f64(deformation.approximation.folded_domain.hi),
            format_f64(deformation.approximation.sine_remainder),
            format_f64(deformation.approximation.cosine_remainder),
            deformation
                .phase_recurrence
                .sine_coefficients
                .iter()
                .map(|bits| format!("{bits:016x}"))
                .collect::<Vec<_>>()
                .join(","),
            deformation
                .phase_recurrence
                .cosine_coefficients
                .iter()
                .map(|bits| format!("{bits:016x}"))
                .collect::<Vec<_>>()
                .join(","),
            deformation.tube_method,
        ));
    }
    for bundle in &program.derivatives.bundles {
        out.push_str(&format!(
            "    DerivativeBundle id={} feature={} g={} first=[{},{},{}] second=[{},{},{},{},{},{}] third=[{},{},{},{},{},{},{},{},{},{}] g_t={} g_tt={} params=[{}]\n",
            bundle.id,
            bundle.feature,
            bundle.g,
            bundle.first.u,
            bundle.first.v,
            bundle.first.q,
            bundle.second.uu,
            bundle.second.uv,
            bundle.second.uq,
            bundle.second.vv,
            bundle.second.vq,
            bundle.second.qq,
            bundle.third.uuu,
            bundle.third.uuv,
            bundle.third.uuq,
            bundle.third.uvv,
            bundle.third.uvq,
            bundle.third.uqq,
            bundle.third.vvv,
            bundle.third.vvq,
            bundle.third.vqq,
            bundle.third.qqq,
            bundle.g_t,
            bundle
                .g_tt
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            bundle
                .parameter
                .iter()
                .map(|derivative| format!(
                    "{}:{}:rate={}",
                    derivative.parameter,
                    derivative.polynomial,
                    derivative
                        .declared_rate
                        .map(|(first, second)| format!("{first:x}:{second:x}"))
                        .unwrap_or_else(|| "none".to_string())
                ))
                .collect::<Vec<_>>()
                .join(","),
        ));
    }
    for cluster in &program.derivatives.clusters {
        out.push_str(&format!(
            "    DerivativeCluster object={} leaf_signature=[{}] bundles=[{}] smooth_root={} scalar_derivative_sources=[{}] value_domain=[{},{}] first_world=[{},{},{}] second_world={} third_world={} params=[{}] frame_delta={} frame_second_delta={} order={} subdivision={} world_delta={} remainder={} predictor_roots={} object_roots={} requires_boundary_events={}\n",
            cluster.object,
            cluster
                .leaf_signature
                .iter()
                .map(|path| {
                    path.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .collect::<Vec<_>>()
                .join("|"),
            cluster
                .bundles
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            cluster.root_tube.scalar_root,
            cluster
                .root_tube
                .scalar_derivative_sources
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            format_f64(cluster.root_tube.value_domain.lo),
            format_f64(cluster.root_tube.value_domain.hi),
            format_f64(cluster.root_tube.first_world_abs[0]),
            format_f64(cluster.root_tube.first_world_abs[1]),
            format_f64(cluster.root_tube.first_world_abs[2]),
            format_f64(cluster.root_tube.second_world_abs),
            format_f64(cluster.root_tube.third_world_abs),
            cluster
                .root_tube
                .parameter_abs
                .iter()
                .map(|(parameter, value)| format!("{parameter}:{}", format_f64(*value)))
                .collect::<Vec<_>>()
                .join(","),
            cluster
                .root_tube
                .frame_delta_abs
                .map(format_f64)
                .unwrap_or_else(|| "none".to_string()),
            cluster
                .root_tube
                .frame_second_delta_abs
                .map(format_f64)
                .unwrap_or_else(|| "none".to_string()),
            cluster.root_tube.taylor_order,
            cluster.root_tube.subdivision_depth,
            format_f64(cluster.root_tube.world_delta_abs_bound),
            format_f64(cluster.root_tube.remainder),
            cluster.root_tube.maximum_predictor_roots,
            cluster.root_tube.maximum_object_roots,
            cluster.root_tube.requires_boundary_events,
        ));
    }
    for span in &program.spans {
        out.push_str(&format!(
            "    ProjectedSpan feature={} normalized=[{},{}]x[{},{}] pixels=[{},{}]x[{},{}] tiles=[{},{}]x[{},{}] q=[{},{}] rule={} outside_margin={} halos=event:{}:filter:{}\n",
            span.feature,
            format_f64(span.normalized.x.lo),
            format_f64(span.normalized.x.hi),
            format_f64(span.normalized.y.lo),
            format_f64(span.normalized.y.hi),
            span.pixels.x.start,
            span.pixels.x.end,
            span.pixels.y.start,
            span.pixels.y.end,
            span.tiles.x.start,
            span.tiles.x.end,
            span.tiles.y.start,
            span.tiles.y.end,
            format_f64(span.q.lo),
            format_f64(span.q.hi),
            stable_projection_rule(span.rule),
            span.outside_margin
                .map(format_f64)
                .unwrap_or_else(|| "-".to_string()),
            span.event_halo,
            span.filter_halo,
        ));
    }
    for event in &program.events.generators {
        out.push_str(&format!(
            "    Event id={} kind={} participants=[{}] pixels=[{},{}]x[{},{}] tiles=[{},{}]x[{},{}] params=[{}] roots={} subdivision={} representation={} sides={}\n",
            event.id,
            stable_event_kind(event.kind),
            event
                .participants
                .iter()
                .map(stable_participant)
                .collect::<Vec<_>>()
                .join(","),
            event.pixels.x.start,
            event.pixels.x.end,
            event.pixels.y.start,
            event.pixels.y.end,
            event.tiles.x.start,
            event.tiles.x.end,
            event.tiles.y.start,
            event.tiles.y.end,
            event
                .coefficient_dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            event.maximum_root_count,
            event.subdivision_depth,
            stable_event_representation(&event.representation),
            stable_event_sides(event.side_meaning),
        ));
    }
    for pair in &program.competitions.pairs {
        out.push_str(&format!(
            "    CompetitionPair id={} a={} b={} event={} pixels=[{},{}]x[{},{}] tiles=[{},{}]x[{},{}] q_overlap={}\n",
            pair.id,
            pair.a,
            pair.b,
            pair.event,
            pair.pixels.x.start,
            pair.pixels.x.end,
            pair.pixels.y.start,
            pair.pixels.y.end,
            pair.tiles.x.start,
            pair.tiles.x.end,
            pair.tiles.y.start,
            pair.tiles.y.end,
            stable_interval(pair.q_overlap),
        ));
    }
    out.push_str(&format!(
        "    CompetitionSummary considered={} emitted={} pruned_projected={} pruned_q={} pruned_csg_global={} pruned_csg_pair={} pruned_strict_order={} suppressed_same_feature={} suppressed_material_only={}\n",
        program.competitions.ledger.len(),
        program.competitions.pairs.len(),
        program.competitions.pruned_projected,
        program.competitions.pruned_q,
        program.competitions.pruned_csg_global,
        program.competitions.pruned_csg_pair,
        program.competitions.pruned_strict_order,
        program.competitions.suppressed_same_feature,
        program.competitions.suppressed_material_only,
    ));
    for proof in &program.exclusions.proofs {
        out.push_str(&format!(
            "    ExclusionProof id={} payload={}\n",
            proof.id,
            stable_proof_payload(&proof.payload),
        ));
    }
    for exclusion in &program.exclusions.records {
        let subject = match exclusion.subject {
            super::exclusions::ExclusionSubject::Candidate(feature) => {
                format!("candidate:{feature}")
            }
            super::exclusions::ExclusionSubject::Event(subject) => format!(
                "event:{}:{}:{}:{}",
                stable_event_kind(subject.kind),
                subject
                    .feature
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                subject
                    .owner
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                subject.ordinal,
            ),
            super::exclusions::ExclusionSubject::Competition(subject) => {
                format!("competition:{}:{}", subject.a, subject.b)
            }
        };
        out.push_str(&format!(
            "    X {} {} {} {} [{},{}] {} [{}]\n",
            exclusion.id,
            subject,
            exclusion.domain,
            stable_exclusion_reason(exclusion.reason),
            format_f64(exclusion.margin.lo),
            format_f64(exclusion.margin.hi),
            exclusion.proof,
            exclusion
                .dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ));
    }
    append_index_dump(out, "tile_features", &program.indexes.tile_features);
    append_index_dump(out, "tile_events", &program.indexes.tile_events);
    append_index_dump(out, "tile_competitions", &program.indexes.tile_competitions);
    append_index_dump(out, "row_block_repeats", &program.indexes.row_block_repeats);
    append_index_dump(out, "tile_lights", &program.indexes.tile_lights);
    append_index_dump(out, "tile_probes", &program.indexes.tile_probes);
    for range in &program.indexes.object_features {
        out.push_str(&format!(
            "    ObjectFeatureIndex object={} first={} count={}\n",
            range.object, range.first, range.count,
        ));
    }
    out.push_str(&format!(
        "    DirectIndexes feature_derivatives=[{}] material_programs=[{}] bytes={}\n",
        program
            .indexes
            .feature_derivatives
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        program
            .indexes
            .material_programs
            .iter()
            .map(|entry| format!("{}:{}", entry.identity_set, entry.material))
            .collect::<Vec<_>>()
            .join(","),
        program.indexes.bytes,
    ));
    let capacity = &program.capacities;
    out.push_str(&format!(
        "    ProjectiveCapacities candidate_features_per_tile={} row_start_roots={} active_sheets_per_row={} event_generators={} competition_pairs_per_tile={} row_event_intervals={} root_stack_nodes={} event_stack_nodes={} runs_per_row={} corridors_per_row={} max_index_slice={} polynomial_programs={} rational_programs={} polynomial_terms_per_program={} coefficient_nodes={} derivative_bundles={} derivative_clusters={} index_bytes={} refined_per_worker_scratch_bytes={} refined_all_worker_scratch_bytes={} final_per_worker_scratch_bytes={} final_all_worker_scratch_bytes={} final_state_bytes={} final_instrumented_state_bytes={}\n",
        capacity.candidate_features_per_tile,
        capacity.row_start_roots,
        capacity.active_sheets_per_row,
        capacity.event_generators,
        capacity.competition_pairs_per_tile,
        capacity.row_event_intervals,
        capacity.root_stack_nodes,
        capacity.event_stack_nodes,
        capacity.runs_per_row,
        capacity.corridors_per_row,
        capacity.max_index_slice,
        capacity.polynomial_programs,
        capacity.rational_programs,
        capacity.polynomial_terms_per_program,
        capacity.coefficient_nodes,
        capacity.derivative_bundles,
        capacity.derivative_clusters,
        capacity.index_bytes,
        capacity.per_worker_scratch_bytes,
        capacity.all_worker_scratch_bytes,
        capacity.final_per_worker_scratch_bytes,
        capacity.final_all_worker_scratch_bytes,
        capacity.total_renderer_state_bytes,
        capacity.total_renderer_state_bytes_instrumented,
    ));
    for derivation in &capacity.derivations {
        out.push_str(&format!(
            "      ProjectiveCapacityDerivation field={} value={} why=[{}]\n",
            derivation.field,
            derivation.value,
            derivation.why.join("; "),
        ));
    }
}

fn append_index_dump(out: &mut String, name: &str, index: &super::index::CompressedIndex) {
    out.push_str(&format!(
        "    Index name={} cells={} ids={}\n",
        name,
        index.cells.len(),
        index.ids.len(),
    ));
    for (cell, slice) in index.cells.iter().enumerate() {
        let start = usize::try_from(slice.offset).unwrap_or(index.ids.len());
        let count = usize::try_from(slice.count).unwrap_or(0);
        let end = start.saturating_add(count).min(index.ids.len());
        out.push_str(&format!(
            "      Cell id={} offset={} count={} values=[{}]\n",
            cell,
            slice.offset,
            slice.count,
            format_id_list(&index.ids[start..end]),
        ));
    }
}

fn format_path(path: &[usize]) -> String {
    let body = path
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("[{body}]")
}

fn format_id_list<T: ToString>(values: &[T]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn format_bounded_debug_list<T: std::fmt::Debug>(values: &[T]) -> String {
    if values.len() <= 16 {
        values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(",")
    } else {
        let text = values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "count={},sha256={}",
            values.len(),
            wrela_machine::sha256::sha256_hex(text.as_bytes())
        )
    }
}

fn format_f64(value: f64) -> String {
    if value == 0.0 && value.is_sign_negative() {
        "-0.0".to_string()
    } else {
        let shortest = value.to_string();
        if shortest.len() <= 24 {
            shortest
        } else {
            format!("{value:.17e}")
        }
    }
}

fn format_f32_bits(bits: u32) -> String {
    let value = f32::from_bits(bits);
    if value == 0.0 && value.is_sign_negative() {
        "-0.0".to_string()
    } else {
        let shortest = value.to_string();
        if shortest.len() <= 16 {
            shortest
        } else {
            format!("{value:.9e}")
        }
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
            sample.pattern.as_ref().map_or_else(
                || "none".to_string(),
                |texture| format!(
                    "immutable(asset={},id={},{}x{},filter={:?},digest={},filter_error=[{},{}])",
                    texture.asset,
                    texture.stable_id,
                    texture.width,
                    texture.height,
                    texture.filter,
                    texture.content_digest,
                    f32::from_bits(texture.filter_error_min_bits),
                    f32::from_bits(texture.filter_error_max_bits),
                ),
            )
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

pub fn dump_skeleton_frame_program(skeleton: &PlaneSkeleton) -> String {
    format!(
        "PlaneSeedMetadata P-1 renderer={} digest={}\n  Header magic=WRELAP1\\0 bytes=80\n  WalkingSkeleton semantic_seed={} storage=generated-actor\n",
        skeleton.renderer_index, skeleton.seed_metadata_digest, skeleton.semantic_digest
    )
}

pub fn dump_skeleton_render_layout(skeleton: &PlaneSkeleton) -> String {
    let renderer = crate::codegen::emit_pixels_plane_renderer(
        &skeleton.seed_metadata,
        &skeleton.semantic_seed,
    );
    let code_bytes = renderer.code.len() * 4;
    let memory_bytes = wrela_machine::pixels::FRAME_BYTES
        + wrela_machine::pixels::CONTROL_BYTES
        + wrela_machine::pixels::QUEUE_CAPACITY as usize * wrela_machine::pixels::TILE_BYTES
        + skeleton.seed_metadata.len();
    format!(
        "RenderLayout v1\n  Renderer index={}\n    PlaneSeedMetadata base={:#010x} size=80\n    GeneratedActor type=Renderer entry={} worker_count=0\n    Display ref={}\n    Mode width={} height={} refresh_hz={} shade_hz={}\n    Tile owner=renderer#0 range=[0,1)\n    Buffer base={:#010x} bytes={} format=BGRA8\n    Baseline code_bytes={} memory_bytes={} frame_cost_instructions={}\n",
        skeleton.renderer_index,
        wrela_machine::pixels::PLANE_SEED_METADATA_BASE,
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

fn stable_exclusion_subject(subject: super::exclusions::ExclusionSubject) -> String {
    match subject {
        super::exclusions::ExclusionSubject::Candidate(feature) => {
            format!("candidate:{feature}")
        }
        super::exclusions::ExclusionSubject::Event(event) => format!(
            "event:{}:feature={}:owner={}:ordinal={}",
            stable_event_kind(event.kind),
            event
                .feature
                .map_or_else(|| "-".to_string(), |value| value.to_string()),
            event
                .owner
                .map_or_else(|| "-".to_string(), |value| value.to_string()),
            event.ordinal,
        ),
        super::exclusions::ExclusionSubject::Competition(pair) => {
            format!("competition:{}:{}", pair.a, pair.b)
        }
    }
}

pub fn dump_frame_program(
    renderer: &super::CompiledRenderer,
    placement: &crate::layout::RendererPlacement,
    generated_source: &str,
) -> Result<String, String> {
    let decoded = super::decode::decode(&renderer.encoded)
        .map_err(|error| format!("pixels::dump: encoded program failed decode: {error}"))?;
    if decoded.program() != renderer.program.program() {
        return Err("pixels::dump: decoded program differs from compiler model".to_string());
    }
    let wire = super::binary_verify::verify_envelope(&renderer.encoded)
        .map_err(|error| format!("pixels::dump: invalid byte envelope: {error}"))?;
    let digest = renderer.encoded[super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
        ..super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
            + super::version::FRAME_PROGRAM_DIGEST_BYTES_V1]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let program = decoded.program();
    let mut out = format!(
        "FrameProgram v1 renderer={} digest={digest}\n\
         \x20 Header bytes={} flags=0x{:x} total_bytes={} profile_revision={} numeric_revision={} formal_revision={} formal_name={}\n\
         \x20 Directory count={} offset={}\n",
        program.renderer_index,
        super::version::FRAME_PROGRAM_HEADER_BYTES_V1,
        program.flags,
        renderer.encoded.len(),
        super::version::FRAME_PROGRAM_PROFILE_REVISION_V1,
        program.numeric_revision,
        program.formal_revision,
        super::version::FRAME_PROGRAM_FORMAL_REVISION_STR_V1,
        wire.len(),
        super::version::FRAME_PROGRAM_HEADER_BYTES_V1,
    );
    for table in &wire {
        out.push_str(&format!(
            "  Table kind={} code={} record_bytes={} count={} offset={} byte_len={}\n",
            table.kind.stable_name(),
            table.kind.code(),
            table.record_bytes,
            table.count,
            table.offset,
            table.byte_len,
        ));
    }
    let structural = renderer.structural.program();
    let projective = renderer.projective.program();
    out.push_str(&format!(
        "  Capacity objects={} features={} candidate_features_per_tile={} row_start_roots={} \
         active_sheets_per_row={} event_intervals_per_row={} root_stack_nodes={} \
         event_stack_nodes={} runs_per_row={} corridors_per_row={} transparent_layers={} \
         state_bytes={}\n",
        structural.capacities.object_count,
        structural.capacities.feature_count,
        projective.capacities.candidate_features_per_tile,
        projective.capacities.row_start_roots,
        projective.capacities.active_sheets_per_row,
        projective.capacities.row_event_intervals,
        projective.capacities.root_stack_nodes,
        projective.capacities.event_stack_nodes,
        projective.capacities.runs_per_row,
        projective.capacities.corridors_per_row,
        structural.capacities.max_transparent_layers,
        renderer.mutable_layout.total_bytes,
    ));
    let fixed_q = program
        .table(super::version::FrameProgramTableKindV1::FixedDomain)
        .and_then(|table| table.records.iter().find(|record| record.tag == 5))
        .ok_or_else(|| "pixels::dump: fixed-q domain record is missing".to_string())?;
    out.push_str(&format!(
        "  FixedQDomain exponent={} maximum_raw={} reset_width={} error_radius={}\n",
        fixed_q.operands[0] as i64, fixed_q.operands[1], fixed_q.operands[2], fixed_q.operands[3],
    ));
    for event in &projective.events.generators {
        out.push_str(&format!(
            "  Event id={} kind={} repr={} roots={} subdivision_depth={} pixels=[{},{};{},{}] \
             tiles=[{},{};{},{}]\n",
            event.id,
            stable_event_kind(event.kind),
            stable_event_representation(&event.representation),
            event.maximum_root_count,
            event.subdivision_depth,
            event.pixels.x.start,
            event.pixels.x.end,
            event.pixels.y.start,
            event.pixels.y.end,
            event.tiles.x.start,
            event.tiles.x.end,
            event.tiles.y.start,
            event.tiles.y.end,
        ));
    }
    for exclusion in &projective.exclusions.records {
        out.push_str(&format!(
            "  Exclusion id={} subject={} reason={} domain={} proof={} margin={}\n",
            exclusion.id,
            stable_exclusion_subject(exclusion.subject),
            exclusion.reason.stable_name(),
            exclusion.domain,
            exclusion.proof,
            stable_interval(exclusion.margin),
        ));
    }
    for proof in &projective.exclusions.proofs {
        out.push_str(&format!(
            "  ExclusionProof id={} payload={}\n",
            proof.id,
            stable_proof_payload(&proof.payload),
        ));
    }
    out.push_str(&format!(
        "  GeneratedConfig module=core.__image_pixels frameprog_base={:#x} \
         frameprog_bytes={} state_base={:#x} state_bytes={}\n",
        placement.frameprog_base,
        placement.frameprog_size,
        placement.state_base,
        placement.state_size,
    ));
    super::glue::parse_configuration_source(generated_source)?;
    out.push_str("  GeneratedModule begin\n");
    for line in generated_source.lines() {
        if !line.is_empty() {
            out.push_str("    ");
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str("  GeneratedModule end\n");
    out.push_str(&format!(
        "  GeneratedActors coordinator={} workers={} palette=bootstrap families=[{}]\n",
        renderer.generated.coordinator,
        renderer.generated.workers.len(),
        renderer.generated.bootstrap_families.join(","),
    ));
    out.push_str(
        "  Fallback renderer_unavailable=FrameContractMismatch presentation=false \
         bounded_local_rebuild=false dense_frame=false\n",
    );
    Ok(out)
}

pub fn dump_render_layout(
    renderer: &super::CompiledRenderer,
    placement: &crate::layout::RendererPlacement,
) -> String {
    let framebuffer_half = placement.framebuffer_bytes / 2;
    let framebuffer_back = placement
        .framebuffer_base
        .checked_add(framebuffer_half)
        .expect("verified renderer placement framebuffer address");
    let mut out = format!(
        "RenderLayout v1\n  Renderer index={}\n\
         \x20   FrameProgram base={:#x} size={}\n\
         \x20   State base={:#x} size={}\n\
         \x20   Coordinator core={} actor={}\n",
        placement.index,
        placement.frameprog_base,
        placement.frameprog_size,
        placement.state_base,
        placement.state_size,
        placement.coordinator_core,
        placement.coordinator_actor,
    );
    for worker in &placement.per_core {
        out.push_str(&format!(
            "    Worker index={} core={} actor={} tiles=[{},{}) workspace_base={:#x} \
             workspace_bytes={}\n",
            worker.worker_index,
            worker.core,
            worker.actor,
            worker.tiles_start,
            worker.tiles_end,
            worker.workspace_base,
            worker.workspace_bytes,
        ));
    }
    out.push_str(&format!(
        "    Buffer front={:#x} back={:#x} bytes_each={}\n\
         \x20   Probe base={:#x} bytes={}\n\
         \x20   Telemetry offset={} production_bytes=0 instrumented_bytes={}\n\
         \x20   Failure presentation=false error={}\n",
        placement.framebuffer_base,
        framebuffer_back,
        framebuffer_half,
        placement.probe_base,
        placement.probe_bytes,
        renderer.mutable_layout.telemetry.offset,
        renderer.mutable_layout.telemetry.bytes,
        super::RENDERER_UNAVAILABLE_FALLBACK,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projective_dump_schema_does_not_use_derived_debug_formatting() {
        let source = include_str!("dump.rs");
        let projective = source
            .split("fn append_projective_dump")
            .nth(1)
            .and_then(|tail| tail.split("fn append_index_dump").next())
            .expect("projective dump function");
        assert!(!projective.contains("{:?}"));
        assert!(!projective.contains(":?}"));
    }

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
                worker_count: 1,
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

    #[test]
    fn large_index_cells_dump_exact_reconstructable_membership() {
        let index = super::super::index::CompressedIndex {
            cells: vec![super::super::index::IndexSlice {
                offset: 0,
                count: 20,
            }],
            ids: (0..20).collect(),
        };
        let mut dump = String::new();
        append_index_dump(&mut dump, "large", &index);
        assert!(dump.contains(
            "Cell id=0 offset=0 count=20 values=[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19]"
        ));
        assert!(!dump.contains("sha256="));
    }
}
