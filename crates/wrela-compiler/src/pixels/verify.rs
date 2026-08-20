//! Deterministic structural-program verifier and verified type boundary.

use std::collections::{BTreeMap, BTreeSet};

use super::bounds::ValueBounds;
use super::capacities::StructuralCapacities;
use super::csg::{CsgInst, CsgProgram};
use super::deform::DeformationTemplate;
use super::derivative_bounds::DerivativeBounds;
use super::features::FeatureRecord;
use super::graph::FieldKind;
#[cfg(test)]
use super::ids::ObjectId;
use super::ids::{FieldId, MaterialId};
use super::material::MaterialEvent;
use super::material_graph::MaterialKind;
use super::objects::ObjectPartition;
use super::params::ParameterLayout;
use super::repeat::RepeatTemplate;
use super::report::StructuralReport;
use super::support::SupportTable;
use super::symbolic::SymbolicGraph;
use super::world_bounds::WorldBounds;

#[derive(Clone, Debug, PartialEq)]
pub struct StructuralProgram {
    pub params: ParameterLayout,
    pub values: ValueBounds,
    pub derivatives: DerivativeBounds,
    pub world_bounds: WorldBounds,
    pub support: SupportTable,
    pub objects: ObjectPartition,
    pub csg: CsgProgram,
    pub features: Vec<FeatureRecord>,
    pub repeats: Vec<RepeatTemplate>,
    pub deformations: Vec<DeformationTemplate>,
    pub material_events: Vec<MaterialEvent>,
    pub capacities: StructuralCapacities,
    pub report: StructuralReport,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedStructuralProgram(StructuralProgram);

impl VerifiedStructuralProgram {
    pub fn program(&self) -> &StructuralProgram {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectiveProgram {
    pub equations: super::projective::ProjectiveEquations,
    pub deformations: Vec<super::deform::ProjectiveDeformationProgram>,
    pub derivatives: super::derivatives::DerivativePrograms,
    pub spans: Vec<super::projection_bounds::ProjectedFeatureSpan>,
    pub events: super::events::EventPrograms,
    pub competitions: super::competition::CompetitionPrograms,
    pub exclusions: super::exclusions::ExclusionPrograms,
    pub indexes: super::index::LocalIndexes,
    pub capacities: super::capacities::ProjectiveCapacities,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedProjectiveProgram(ProjectiveProgram);

impl VerifiedProjectiveProgram {
    pub fn program(&self) -> &ProjectiveProgram {
        &self.0
    }
}

pub fn check_program(
    program: super::program::FrameProgram,
) -> Result<super::program::VerifiedFrameProgram, String> {
    wrela_machine::pixels::verify_frame_program_model_v1(&program)?;
    Ok(super::program::VerifiedFrameProgram::new(program))
}
fn field_children(kind: &FieldKind) -> Vec<FieldId> {
    match kind {
        FieldKind::Primitive(_) => Vec::new(),
        FieldKind::HardUnion { a, b }
        | FieldKind::HardIntersection { a, b }
        | FieldKind::HardSubtract { a, b }
        | FieldKind::SmoothUnion { a, b, .. }
        | FieldKind::SmoothIntersection { a, b, .. }
        | FieldKind::SmoothSubtract { a, b, .. } => vec![*a, *b],
        FieldKind::Neg { child }
        | FieldKind::Transform { child, .. }
        | FieldKind::FiniteRepeat { child, .. }
        | FieldKind::Mark { child, .. } => vec![*child],
        FieldKind::BoundedDisplace { base, .. } => vec![*base],
    }
}

pub(crate) fn check_input_depth(graph: &SymbolicGraph) -> Result<(), String> {
    let ceiling = super::capacities::PixelsCeilings::MACHINE_V1.structural_depth;
    let mut scalar_depth = BTreeMap::<super::ids::ScalarId, u32>::new();
    for (id, node) in graph.scalar.iter() {
        let depth = super::params::scalar_children(&node.op)
            .into_iter()
            .try_fold(1_u32, |depth, child| {
                let child_depth = scalar_depth.get(&child).copied().ok_or_else(|| {
                    format!("pixels::verify: scalar {id} names non-predecessor {child}")
                })?;
                Ok::<u32, String>(depth.max(child_depth.saturating_add(1)))
            })?;
        if depth > ceiling {
            return Err(format!(
                "P015: renderer capacity `structural_depth` needs {depth} levels, which exceeds the machine-v1 ceiling of {ceiling}"
            ));
        }
        scalar_depth.insert(id, depth);
    }
    let mut field_depth = BTreeMap::<FieldId, u32>::new();
    for (id, node) in graph.fields.iter() {
        let depth = field_children(&node.kind)
            .into_iter()
            .try_fold(1_u32, |depth, child| {
                let child_depth = field_depth.get(&child).copied().ok_or_else(|| {
                    format!("pixels::verify: field {id} names non-predecessor {child}")
                })?;
                Ok::<u32, String>(depth.max(child_depth.saturating_add(1)))
            })?;
        if depth > ceiling {
            return Err(format!(
                "P015: renderer capacity `structural_depth` needs {depth} levels, which exceeds the machine-v1 ceiling of {ceiling}"
            ));
        }
        field_depth.insert(id, depth);
    }
    let mut material_depth = BTreeMap::<MaterialId, u32>::new();
    for (id, node) in graph.materials.iter() {
        let children = match &node.kind {
            MaterialKind::Sample(_) => Vec::new(),
            MaterialKind::Select { a, b, .. } => vec![*a, *b],
            MaterialKind::IdentityTable { cases, .. } => {
                cases.iter().map(|(_, child)| *child).collect()
            }
        };
        let depth = children.into_iter().try_fold(1_u32, |depth, child| {
            let child_depth = material_depth.get(&child).copied().ok_or_else(|| {
                format!("pixels::verify: material {id} names non-predecessor {child}")
            })?;
            Ok::<u32, String>(depth.max(child_depth.saturating_add(1)))
        })?;
        if depth > ceiling {
            return Err(format!(
                "P015: renderer capacity `structural_depth` needs {depth} levels, which exceeds the machine-v1 ceiling of {ceiling}"
            ));
        }
        material_depth.insert(id, depth);
    }
    Ok(())
}

fn reachable_fields(graph: &SymbolicGraph) -> Result<BTreeSet<FieldId>, String> {
    let mut reached = BTreeSet::new();
    let mut stack = vec![graph.field_root];
    while let Some(id) = stack.pop() {
        if !reached.insert(id) {
            continue;
        }
        stack.extend(field_children(&graph.fields.get(id)?.kind));
    }
    Ok(reached)
}

fn reachable_materials(graph: &SymbolicGraph) -> Result<BTreeSet<MaterialId>, String> {
    let mut reached = BTreeSet::new();
    let mut stack = vec![graph.material_root];
    while let Some(id) = stack.pop() {
        if !reached.insert(id) {
            continue;
        }
        match &graph.materials.get(id)?.kind {
            MaterialKind::Sample(_) => {}
            MaterialKind::Select { a, b, .. } => stack.extend([*a, *b]),
            MaterialKind::IdentityTable { cases, .. } => {
                stack.extend(cases.iter().map(|(_, material)| *material));
            }
        }
    }
    Ok(reached)
}

fn verify_scalar_obligations(graph: &SymbolicGraph, values: &ValueBounds) -> Result<(), String> {
    use super::scalar::{CompareOp, ProofObligation, ScalarOp};
    use super::symbolic::PendingObligation;

    for obligation in &graph.obligations {
        let PendingObligation::Scalar(obligation) = obligation else {
            continue;
        };
        match obligation {
            ProofObligation::DenominatorNonZero { denominator } => {
                let interval = values.get(*denominator)?;
                if interval.contains_zero() {
                    return Err(format!(
                        "P004: field operation `division` is not available in `AaaByteExact`: denominator {denominator} may reach zero over {interval:?}"
                    ));
                }
            }
            ProofObligation::GuardedDenominatorNonZero {
                denominator,
                predicate,
            } => {
                let ScalarOp::Compare {
                    op: CompareOp::Gt,
                    a,
                    b,
                } = graph.scalar.get(*predicate)?.op
                else {
                    return Err(format!(
                        "pixels::verify: guarded denominator {denominator} has a noncanonical predicate {predicate}"
                    ));
                };
                if a != *denominator
                    || !matches!(
                        graph.scalar.get(b)?.op,
                        ScalarOp::ConstF32(bits) if f32::from_bits(bits) == 0.0
                    )
                {
                    return Err(format!(
                        "pixels::verify: guarded denominator {denominator} is not protected by the canonical positive test"
                    ));
                }
            }
            ProofObligation::RestrictedTrigDomain { argument } => {
                let interval = values.get(*argument)?;
                if !interval.lo.is_finite() || !interval.hi.is_finite() {
                    return Err(format!(
                        "P013: deformation `sinusoidal_displace` lacks a finite folded-polynomial domain for {argument}"
                    ));
                }
            }
            ProofObligation::DynamicIndexInBounds { index, extent } => {
                let interval = values.get(*index)?;
                if *extent == 0 || interval.lo < 0.0 || interval.hi >= f64::from(*extent) {
                    return Err(format!(
                        "P004: field operation `dynamic_index` is not available in `AaaByteExact`: index {index} range {interval:?} is outside [0,{extent})"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn verify_graph(graph: &SymbolicGraph) -> Result<(), String> {
    check_input_depth(graph)?;
    let fields = reachable_fields(graph)?;
    if fields.len() != graph.fields.len() {
        let lowest = graph
            .fields
            .iter()
            .map(|(id, _)| id)
            .find(|id| !fields.contains(id))
            .ok_or_else(|| {
                "pixels::verify: field reachability cardinality mismatch without a stable missing ID"
                    .to_string()
            })?;
        return Err(format!(
            "pixels::verify: unreachable field node with lowest stable ID {lowest}"
        ));
    }
    let materials = reachable_materials(graph)?;
    if materials.len() != graph.materials.len() {
        let lowest = graph
            .materials
            .iter()
            .map(|(id, _)| id)
            .find(|id| !materials.contains(id))
            .ok_or_else(|| {
                "pixels::verify: material reachability cardinality mismatch without a stable missing ID"
                    .to_string()
            })?;
        return Err(format!(
            "pixels::verify: unreachable material node with lowest stable ID {lowest}"
        ));
    }
    Ok(())
}

fn verify_analysis_coverage(
    graph: &SymbolicGraph,
    program: &StructuralProgram,
) -> Result<(), String> {
    if program.values.scalar.len() != graph.scalar.len() {
        return Err(format!(
            "pixels::verify: value-bound count {} differs from scalar count {}",
            program.values.scalar.len(),
            graph.scalar.len()
        ));
    }
    if program.derivatives.scalar.len() != graph.scalar.len() {
        return Err(format!(
            "pixels::verify: derivative-bound count {} differs from scalar count {}",
            program.derivatives.scalar.len(),
            graph.scalar.len()
        ));
    }
    if program.world_bounds.fields.len() != graph.fields.len() {
        return Err(format!(
            "pixels::verify: world-bound count {} differs from field count {}",
            program.world_bounds.fields.len(),
            graph.fields.len()
        ));
    }
    if program.support.fields.len() != graph.fields.len() {
        return Err(format!(
            "pixels::verify: support count {} differs from field count {}",
            program.support.fields.len(),
            graph.fields.len()
        ));
    }
    for (id, _) in graph.scalar.iter() {
        if !program.values.scalar.contains_key(&id) {
            return Err(format!("pixels::verify: scalar {id} has no value bound"));
        }
        if !program.derivatives.scalar.contains_key(&id) {
            return Err(format!(
                "pixels::verify: scalar {id} has no derivative bound"
            ));
        }
    }
    for (id, _) in graph.fields.iter() {
        if !program.world_bounds.fields.contains_key(&id) {
            return Err(format!("pixels::verify: field {id} has no world bound"));
        }
        if !program.support.fields.contains_key(&id) {
            return Err(format!("pixels::verify: field {id} has no support record"));
        }
    }
    Ok(())
}

fn verify_analysis_rules(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
    program: &StructuralProgram,
) -> Result<(), String> {
    let values = super::bounds::propagate(graph, config)?;
    for (id, expected) in &values.scalar {
        if program.values.scalar.get(id) != Some(expected) {
            return Err(format!(
                "pixels::verify: scalar {id} value bound differs from exact rule"
            ));
        }
    }
    let derivatives = super::derivative_bounds::propagate(graph, &values)?;
    for (id, expected) in &derivatives.scalar {
        if program.derivatives.scalar.get(id) != Some(expected) {
            return Err(format!(
                "pixels::verify: scalar {id} derivative bound differs from exact rule"
            ));
        }
    }
    let support = super::support::propagate(graph, &values)?;
    for (id, expected) in &support.fields {
        if program.support.fields.get(id) != Some(expected) {
            return Err(format!(
                "pixels::verify: field {id} support paths or budgets differ from exact rule"
            ));
        }
    }
    let world = super::world_bounds::derive(graph, config, &values, &support)?;
    for (id, expected) in &world.fields {
        if program.world_bounds.fields.get(id) != Some(expected) {
            return Err(format!(
                "pixels::verify: field {id} world bound differs from exact rule"
            ));
        }
    }
    Ok(())
}

fn verify_parameters(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
    program: &StructuralProgram,
) -> Result<(), String> {
    let expected = super::params::derive_layout(graph, config)?;
    if program.params != expected {
        return Err(
            "pixels::verify: parameter layout differs from exact graph dependency derivation"
                .to_string(),
        );
    }
    let records = graph
        .params
        .iter()
        .map(|param| (param.id, param))
        .collect::<BTreeMap<_, _>>();
    let mut prior_end = 0_u32;
    for slot in &program.params.slots {
        let record = records
            .get(&slot.id)
            .ok_or_else(|| format!("pixels::verify: slot references missing {}", slot.id))?;
        if slot.range.min != record.range_min || slot.range.max != record.range_max {
            return Err(format!(
                "pixels::verify: parameter {} range differs from source contract",
                slot.id
            ));
        }
        if !slot.range.min.is_finite() || !slot.range.max.is_finite() {
            return Err(format!(
                "pixels::verify: parameter {} range is non-finite",
                slot.id
            ));
        }
        if let Some(rate) = slot.rate {
            if !rate.max_delta.is_finite()
                || !rate.max_second_delta.is_finite()
                || rate.max_delta < 0.0
                || rate.max_second_delta < 0.0
            {
                return Err(format!(
                    "pixels::verify: parameter {} rate is invalid",
                    slot.id
                ));
            }
        }
        if slot.packed_offset < prior_end {
            return Err(format!(
                "pixels::verify: parameter {} overlaps its predecessor",
                slot.id
            ));
        }
        prior_end = slot
            .packed_offset
            .checked_add(slot.scalar_ty.size())
            .ok_or_else(|| "pixels::verify: parameter end overflow".to_string())?;
    }
    let snapshot_alignment = program
        .params
        .slots
        .iter()
        .map(|slot| slot.scalar_ty.size())
        .max()
        .unwrap_or(1);
    let expected_packed_bytes = prior_end
        .checked_add(snapshot_alignment - 1)
        .map(|value| value / snapshot_alignment * snapshot_alignment)
        .ok_or_else(|| "P015: packed parameter alignment overflow".to_string())?;
    if expected_packed_bytes != program.params.packed_bytes {
        return Err(format!(
            "pixels::verify: packed byte count {} does not equal aligned final slot end {expected_packed_bytes}",
            program.params.packed_bytes,
        ));
    }
    Ok(())
}

fn verify_objects(graph: &SymbolicGraph, program: &StructuralProgram) -> Result<(), String> {
    for (index, identity) in program.objects.identities.iter().enumerate() {
        if identity.id as usize != index {
            return Err(format!(
                "pixels::verify: non-dense identity-set ID {}",
                identity.id
            ));
        }
        if identity.pairs.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(format!(
                "pixels::verify: identity set {} is not strictly ordered",
                identity.id
            ));
        }
    }
    let identity_ids = program
        .objects
        .identities
        .iter()
        .map(|identity| identity.id)
        .collect::<BTreeSet<_>>();
    for (index, object) in program.objects.objects.iter().enumerate() {
        if object.id.0 as usize != index {
            return Err(format!("pixels::verify: non-dense object ID {}", object.id));
        }
        graph.fields.get(object.source_root)?;
        graph.scalar.get(object.scalar_root)?;
        if !identity_ids.contains(&object.identity_set) {
            return Err(format!(
                "pixels::verify: object {} references missing identity set {}",
                object.id, object.identity_set
            ));
        }
        let support = program.support.get(object.source_root)?;
        if object.primitive_occurrences.is_empty() {
            return Err(format!(
                "pixels::verify: object {} has no primitive occurrences",
                object.id
            ));
        }
        if object
            .primitive_occurrences
            .windows(2)
            .any(|pair| pair[0] > pair[1])
        {
            return Err(format!(
                "pixels::verify: object {} primitive occurrences are not ordered",
                object.id
            ));
        }
        for path in &object.primitive_occurrences {
            let leaf = path
                .first()
                .ok_or_else(|| {
                    format!(
                        "pixels::verify: object {} has an empty primitive occurrence",
                        object.id
                    )
                })?
                .field;
            let FieldKind::Primitive(primitive) = &graph.fields.get(leaf)?.kind else {
                return Err(format!(
                    "pixels::verify: object {} leaf {leaf} is not primitive",
                    object.id
                ));
            };
            if !support
                .leaf_supports
                .iter()
                .any(|candidate| candidate.path == *path)
            {
                return Err(format!(
                    "pixels::verify: object {} occurrence {:?} has no support budget",
                    object.id, path
                ));
            }
            let feature_count = program
                .features
                .iter()
                .filter(|feature| {
                    feature.object == object.id
                        && feature.occurrence_path == *path
                        && feature.primitive == leaf
                })
                .count();
            let expected = super::features::expected_feature_count(primitive);
            if feature_count != expected {
                return Err(format!(
                    "pixels::verify: object {} leaf {leaf} has {feature_count} feature records, expected {expected}",
                    object.id,
                ));
            }
        }
        if object.repeat_instances.iter().any(|instance| {
            !instance.period.lo.is_finite()
                || !instance.period.hi.is_finite()
                || instance.period.lo <= 0.0
                || instance.equivalent_fields.is_empty()
                || !instance.equivalent_fields.contains(&instance.repeat_field)
                || instance
                    .equivalent_fields
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
        }) {
            return Err(format!(
                "pixels::verify: object {} has invalid repeat contract",
                object.id
            ));
        }
        for instance in &object.repeat_instances {
            for field in &instance.equivalent_fields {
                let FieldKind::FiniteRepeat { axis, period, .. } = &graph.fields.get(*field)?.kind
                else {
                    return Err(format!(
                        "pixels::verify: object {} repeat alias {field} is not a finite repeat",
                        object.id
                    ));
                };
                if *axis != instance.axis || program.values.get(*period)? != instance.period {
                    return Err(format!(
                        "pixels::verify: object {} repeat alias {field} differs from its canonical family",
                        object.id
                    ));
                }
            }
        }
    }
    Ok(())
}

fn verify_features(graph: &SymbolicGraph, program: &StructuralProgram) -> Result<(), String> {
    let expected = super::features::decompose(
        graph,
        &program.objects,
        &program.values,
        &program.world_bounds,
        &program.support,
    )?;
    if expected.len() != program.features.len() {
        return Err(format!(
            "pixels::verify: feature record count {} differs from exact decomposition {}",
            program.features.len(),
            expected.len()
        ));
    }
    for (index, feature) in program.features.iter().enumerate() {
        if feature.id.0 as usize != index {
            return Err(format!(
                "pixels::verify: non-dense feature ID {}",
                feature.id
            ));
        }
        let object = program
            .objects
            .objects
            .get(feature.object.0 as usize)
            .ok_or_else(|| {
                format!(
                    "pixels::verify: feature {} references missing object {}",
                    feature.id, feature.object
                )
            })?;
        graph.fields.get(feature.scalar_semantic_root)?;
        if feature.identity_set != object.identity_set {
            return Err(format!(
                "pixels::verify: feature {} identity differs from object {}",
                feature.id, feature.object
            ));
        }
        if feature.validity.constraints.is_empty() {
            return Err(format!(
                "pixels::verify: feature {} has no validity predicate",
                feature.id
            ));
        }
        for predicate in &feature.validity.constraints {
            let _ = super::primitive::contains_boundary_point(
                predicate,
                [0.0; 3],
                &|scalar| {
                    let interval = program.values.get(scalar)?;
                    Ok(interval.lo * 0.5 + interval.hi * 0.5)
                },
                0.0,
            )
            .map_err(|error| {
                format!(
                    "pixels::verify: feature {} validity predicate is not executable: {error}",
                    feature.id
                )
            })?;
        }
        if feature.scalar_semantic_root != object.source_root
            || !object
                .primitive_occurrences
                .contains(&feature.occurrence_path)
            || feature
                .occurrence_path
                .first()
                .is_none_or(|step| step.field != feature.primitive)
        {
            return Err(format!(
                "pixels::verify: feature {} is not a member of object {}",
                feature.id, feature.object
            ));
        }
        if (0..3).any(|axis| {
            feature.world_bounds.min[axis] < object.bounds.min[axis]
                || feature.world_bounds.max[axis] > object.bounds.max[axis]
        }) {
            return Err(format!(
                "pixels::verify: feature {} bounds escape object {}",
                feature.id, feature.object
            ));
        }
        if feature.support_expand < 0.0 || !feature.support_expand.is_finite() {
            return Err(format!(
                "pixels::verify: feature {} has invalid support expansion",
                feature.id
            ));
        }
        let exact = &expected[index];
        if feature.primitive != exact.primitive || feature.occurrence_path != exact.occurrence_path
        {
            return Err(format!(
                "pixels::verify: feature {} primitive occurrence differs from exact decomposition",
                feature.id
            ));
        }
        if feature.kind != exact.kind {
            return Err(format!(
                "pixels::verify: feature {} analytic kind differs from exact decomposition",
                feature.id
            ));
        }
        if feature.world_bounds != exact.world_bounds {
            return Err(format!(
                "pixels::verify: feature {} world bounds differ from exact decomposition",
                feature.id
            ));
        }
        if feature.support_expand != exact.support_expand {
            return Err(format!(
                "pixels::verify: feature {} support expansion differs from exact decomposition",
                feature.id
            ));
        }
        if feature.validity != exact.validity {
            return Err(format!(
                "pixels::verify: feature {} validity predicate differs from exact decomposition",
                feature.id
            ));
        }
        if feature.orientation != exact.orientation {
            return Err(format!(
                "pixels::verify: feature {} orientation differs from exact occurrence path",
                feature.id
            ));
        }
        if feature.scalar_semantic_root != exact.scalar_semantic_root {
            return Err(format!(
                "pixels::verify: feature {} semantic root differs from owning object",
                feature.id
            ));
        }
        if feature.identity_set != exact.identity_set {
            return Err(format!(
                "pixels::verify: feature {} identity set differs from owning object",
                feature.id
            ));
        }
        if feature.template_id != exact.template_id {
            return Err(format!(
                "pixels::verify: feature {} template ID differs from exact decomposition",
                feature.id
            ));
        }
    }
    Ok(())
}

fn verify_csg(program: &StructuralProgram) -> Result<(), String> {
    let object_count = program.objects.objects.len();
    let referenced_objects = program
        .csg
        .instructions
        .iter()
        .filter_map(|instruction| match instruction {
            CsgInst::Push(object) => Some(*object),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let expected_objects = program
        .objects
        .objects
        .iter()
        .map(|object| object.id)
        .collect::<BTreeSet<_>>();
    if referenced_objects
        .iter()
        .all(|object| (object.0 as usize) < object_count)
        && referenced_objects != expected_objects
    {
        let object = expected_objects
            .difference(&referenced_objects)
            .next()
            .copied()
            .or_else(|| {
                referenced_objects
                    .difference(&expected_objects)
                    .next()
                    .copied()
            })
            .ok_or_else(|| {
                "pixels::verify: CSG reachability mismatch without a stable object ID".to_string()
            })?;
        return Err(format!(
            "pixels::verify: object {object} is not exactly represented by the hard-CSG program"
        ));
    }
    let verify_instructions = |label: &str,
                               constant: Option<bool>,
                               instructions: &[CsgInst],
                               recorded_max: u32|
     -> Result<(), String> {
        for instruction in instructions {
            if let CsgInst::Push(object) = instruction {
                if object.0 as usize >= object_count {
                    return Err(format!(
                        "pixels::verify: {label} references missing object {object}"
                    ));
                }
            }
        }
        let mut depth = 0_u32;
        let mut maximum = 0_u32;
        for instruction in instructions {
            match instruction {
                CsgInst::Push(_) => {
                    depth = depth
                        .checked_add(1)
                        .ok_or_else(|| "pixels::verify: CSG stack depth overflow".to_string())?;
                }
                CsgInst::Not if depth == 0 => {
                    return Err(format!("pixels::verify: {label} Not underflows stack"));
                }
                CsgInst::Not => {}
                CsgInst::And | CsgInst::Or if depth < 2 => {
                    return Err(format!(
                        "pixels::verify: {label} binary instruction underflows stack"
                    ));
                }
                CsgInst::And | CsgInst::Or => depth -= 1,
            }
            maximum = maximum.max(depth);
        }
        let expected_depth = u32::from(constant.is_none());
        if depth != expected_depth {
            return Err(format!(
                "pixels::verify: {label} final stack depth is {depth}, expected {expected_depth}"
            ));
        }
        if maximum != recorded_max {
            return Err(format!(
                "pixels::verify: {label} max stack {recorded_max} differs from derived {maximum}"
            ));
        }
        Ok(())
    };
    if program.csg.influence.len() != object_count && program.csg.constant.is_none() {
        return Err(format!(
            "pixels::verify: CSG influence count {} differs from object count {object_count}",
            program.csg.influence.len()
        ));
    }
    verify_instructions(
        "CSG",
        program.csg.constant,
        &program.csg.instructions,
        program.csg.max_stack,
    )?;
    let expected = super::csg::compile(program.objects.csg.clone(), object_count)?;
    if program.csg.constant != expected.constant
        || program.csg.instructions != expected.instructions
        || program.csg.max_stack != expected.max_stack
    {
        return Err(
            "pixels::verify: root CSG instructions differ from exact occupancy expression"
                .to_string(),
        );
    }
    if program.csg.influence.len() != expected.influence.len() {
        return Err(format!(
            "pixels::verify: CSG cofactor count {} differs from exact count {}",
            program.csg.influence.len(),
            expected.influence.len()
        ));
    }
    for (actual, exact) in program.csg.influence.iter().zip(&expected.influence) {
        if actual != exact {
            return Err(format!(
                "pixels::verify: CSG cofactors differ for object {}",
                exact.object
            ));
        }
    }
    for (index, influence) in program.csg.influence.iter().enumerate() {
        if influence.object.0 as usize != index {
            return Err(format!(
                "pixels::verify: CSG influence has non-dense object {}",
                influence.object
            ));
        }
        for (forced, cofactor) in [
            ("false cofactor", &influence.when_false),
            ("true cofactor", &influence.when_true),
        ] {
            verify_instructions(
                forced,
                cofactor.constant,
                &cofactor.instructions,
                cofactor.max_stack,
            )?;
            if cofactor.digest != super::csg::cofactor_digest(cofactor) {
                return Err(format!(
                    "pixels::verify: CSG {forced} digest differs for {}",
                    influence.object
                ));
            }
            if cofactor.instructions.iter().any(
                |instruction| matches!(instruction, CsgInst::Push(id) if *id == influence.object),
            ) {
                return Err(format!(
                    "pixels::verify: CSG {forced} still references forced object {}",
                    influence.object
                ));
            }
        }
    }
    Ok(())
}

fn verify_material_events(
    graph: &SymbolicGraph,
    program: &StructuralProgram,
) -> Result<(), String> {
    for (material, node) in graph.materials.iter() {
        let MaterialKind::Sample(sample) = &node.kind else {
            continue;
        };
        let require_range = |label: &str,
                             scalar: super::ids::ScalarId,
                             minimum: f64,
                             maximum: f64|
         -> Result<(), String> {
            let range = program.values.get(scalar)?;
            if range.lo < minimum || range.hi > maximum {
                return Err(format!(
                    "P007: range for material {material} {label} is not representable by MaterialSample.standard: [{}, {}] is outside [{minimum}, {maximum}]",
                    range.lo, range.hi
                ));
            }
            Ok(())
        };
        for (channel, scalar) in sample.base_color.into_iter().enumerate() {
            require_range(&format!("base_color[{channel}]"), scalar, 0.0, 1.0)?;
        }
        require_range("metallic", sample.metallic, 0.0, 1.0)?;
        require_range("roughness", sample.roughness, 0.02, 1.0)?;
        require_range("specular", sample.specular_level, 0.0, 1.0)?;
        require_range("opacity", sample.opacity, 0.0, 1.0)?;
        for (channel, scalar) in sample.emissive.into_iter().enumerate() {
            require_range(&format!("emissive[{channel}]"), scalar, 0.0, 65_504.0)?;
        }
        if let Some(texture) = &sample.pattern {
            let exact = super::material_graph::compiler_texture(
                texture.stable_id,
                texture.filter,
                texture.uv_source,
            )?;
            if *texture != exact {
                return Err(format!(
                    "pixels::verify: material {material} texture descriptor differs from compiler-owned asset `{}`",
                    texture.asset
                ));
            }
        }
        if let super::material_graph::NormalModel::TextureSlope { texture } = &sample.normal {
            let exact = super::material_graph::compiler_texture(
                texture.stable_id,
                texture.filter,
                texture.uv_source,
            )?;
            let asset = super::texture::compiler_asset(texture.stable_id)?;
            if *texture != exact || asset.format != super::texture::TextureFormat::Rg8Snorm {
                return Err(format!(
                    "pixels::verify: material {material} normal texture descriptor differs from a compiler-owned Rg8Snorm asset `{}`",
                    texture.asset
                ));
            }
        }
    }
    let object_count = program.objects.objects.len();
    let mut seen_events = BTreeSet::new();
    for event in &program.material_events {
        if !seen_events.insert(event.predicate) {
            return Err(format!(
                "pixels::verify: duplicate material event for {}",
                event.predicate
            ));
        }
        graph.scalar.get(event.predicate)?;
        if event.owners.is_empty() && object_count != 0 {
            return Err(format!(
                "pixels::verify: material event {} has no owning object",
                event.predicate
            ));
        }
        if event.owners.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(format!(
                "pixels::verify: material event {} owners are not strictly ordered",
                event.predicate
            ));
        }
        if let Some(owner) = event
            .owners
            .iter()
            .find(|owner| owner.0 as usize >= object_count)
        {
            return Err(format!(
                "pixels::verify: material event {} references missing object {owner}",
                event.predicate
            ));
        }
        if event.crossing_bound == 0
            || event
                .feature_owners
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(format!(
                "pixels::verify: material event {} has an invalid crossing/feature-owner set",
                event.predicate
            ));
        }
        let owner_set = event.owners.iter().copied().collect::<BTreeSet<_>>();
        for feature in &event.feature_owners {
            let feature = program.features.get(feature.0 as usize).ok_or_else(|| {
                format!(
                    "pixels::verify: material event {} references missing feature {feature}",
                    event.predicate
                )
            })?;
            if !owner_set.contains(&feature.object) {
                return Err(format!(
                    "pixels::verify: material event {} feature {} is not owned by its object set",
                    event.predicate, feature.id
                ));
            }
        }
    }
    let expected =
        super::material::compile(graph, &program.values, &program.objects, &program.features)?;
    if program.material_events.len() != expected.len() {
        return Err(format!(
            "pixels::verify: material event count {} differs from exact count {}",
            program.material_events.len(),
            expected.len()
        ));
    }
    for (actual, exact) in program.material_events.iter().zip(&expected) {
        if actual != exact {
            return Err(format!(
                "pixels::verify: material event {} owner, crossing, kind, or origin differs from exact classification",
                exact.predicate
            ));
        }
    }
    Ok(())
}

fn verify_repeats(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
    program: &StructuralProgram,
) -> Result<(), String> {
    let expected = super::repeat::compile(graph, config, &program.objects)?;
    if program.repeats.len() != expected.len() {
        return Err(format!(
            "pixels::verify: repeat template count {} differs from exact count {}",
            program.repeats.len(),
            expected.len()
        ));
    }
    for (actual, exact) in program.repeats.iter().zip(&expected) {
        if actual != exact {
            return Err(format!(
                "pixels::verify: repeat template {} translations or wrap events differ from exact derivation",
                exact.source_root
            ));
        }
    }
    Ok(())
}

fn verify_capacities(graph: &SymbolicGraph, program: &StructuralProgram) -> Result<(), String> {
    let capacities = &program.capacities;
    let expected_repeated_objects = program
        .objects
        .objects
        .iter()
        .filter(|object| !object.repeat_instances.is_empty())
        .map(|object| object.id)
        .collect::<BTreeSet<_>>();
    let recorded_repeated_objects = program
        .repeats
        .iter()
        .flat_map(|template| template.instances.iter().map(|instance| instance.object))
        .collect::<BTreeSet<_>>();
    if recorded_repeated_objects != expected_repeated_objects {
        let missing = expected_repeated_objects
            .difference(&recorded_repeated_objects)
            .next()
            .copied()
            .or_else(|| {
                recorded_repeated_objects
                    .difference(&expected_repeated_objects)
                    .next()
                    .copied()
            })
            .ok_or_else(|| {
                "pixels::verify: repeat template set mismatch without a stable object ID"
                    .to_string()
            })?;
        return Err(format!(
            "pixels::verify: repeated object {missing} has no exact repeat template instance"
        ));
    }
    let exact_template_count = program
        .features
        .iter()
        .map(|feature| feature.template_id)
        .collect::<BTreeSet<_>>()
        .len();
    if (capacities.object_count as usize) != program.objects.objects.len()
        || (capacities.feature_template_count as usize) != exact_template_count
        || (capacities.feature_count as usize) != program.features.len()
        || (capacities.parameter_slots as usize) != program.params.slots.len()
        || (capacities.scalar_program_slots as usize) != program.values.scalar.len()
        || (capacities.derivative_program_slots as usize) != program.derivatives.scalar.len()
        || capacities.max_csg_stack != program.csg.max_stack
    {
        return Err(
            "pixels::verify: a capacity does not dominate its exact table size".to_string(),
        );
    }
    if capacities.telemetry_bytes_production != 0 {
        return Err(
            "pixels::verify: uninstrumented production telemetry must reserve zero bytes"
                .to_string(),
        );
    }
    let exact_feature_count = program.features.len();
    if capacities.feature_count as usize != exact_feature_count {
        return Err(format!(
            "pixels::verify: feature capacity {} differs from exact instantiated count {exact_feature_count}",
            capacities.feature_count
        ));
    }
    if capacities.worker_count == 0 {
        return Err("pixels::verify: structural capacity has zero workers".to_string());
    }
    let expected_transparent_layers = if capacities.object_count == 0 {
        0
    } else if super::capacities::material_may_transmit(graph, &program.values)? {
        capacities.max_object_roots_per_row_start
    } else {
        1
    };
    if capacities.max_projected_features_per_row != capacities.feature_count
        || capacities.max_projected_features_per_tile != capacities.feature_count
        || capacities.max_active_sheet_records_per_row != capacities.max_object_roots_per_row_start
        || capacities.max_csg_events_per_row != capacities.max_object_roots_per_row_start
        || capacities.max_local_rebuild_queue != capacities.max_run_records_per_tile_row
        || capacities.max_transparent_layers != expected_transparent_layers
    {
        return Err("pixels::verify: structural count derivation is inconsistent".to_string());
    }
    let checked_mul = |a: u64, b: u64, name: &str| {
        a.checked_mul(b)
            .ok_or_else(|| format!("pixels::verify: {name} arithmetic overflow"))
    };
    let checked_sum = |values: &[u64], name: &str| {
        values.iter().try_fold(0_u64, |sum, value| {
            sum.checked_add(*value)
                .ok_or_else(|| format!("pixels::verify: {name} arithmetic overflow"))
        })
    };
    let repeat_wrap_generators =
        program.repeats.iter().try_fold(0_u64, |count, template| {
            count
                .checked_add(u64::try_from(template.wrap_events.len()).map_err(|_| {
                    "pixels::verify: repeat wrap generator count overflow".to_string()
                })?)
                .ok_or_else(|| "pixels::verify: repeat wrap generator count overflow".to_string())
        })?;
    let feature_boundary_generators =
        program.features.iter().try_fold(0_u64, |count, feature| {
            count
                .checked_add(u64::from(feature.validity.boundary_generator_count()?))
                .ok_or_else(|| {
                    "pixels::verify: feature boundary generator count overflow".to_string()
                })
        })?;
    let material_generators = program
        .material_events
        .iter()
        .try_fold(0_u64, |count, event| {
            count
                .checked_add(
                    u64::try_from(event.feature_owners.len())
                        .map_err(|_| {
                            "pixels::verify: material event owner count overflow".to_string()
                        })?
                        .checked_mul(u64::from(event.crossing_bound))
                        .ok_or_else(|| {
                            "pixels::verify: material crossing count overflow".to_string()
                        })?,
                )
                .ok_or_else(|| "pixels::verify: material generator count overflow".to_string())
        })?;
    let expected_generators = checked_sum(
        &[
            u64::from(capacities.feature_count),
            feature_boundary_generators,
            material_generators,
            repeat_wrap_generators,
        ],
        "event generators",
    )?;
    if u64::from(capacities.event_generator_count) != expected_generators {
        return Err(format!(
            "pixels::verify: event generator count {} differs from exact structural count {expected_generators}",
            capacities.event_generator_count
        ));
    }
    let expected_events = checked_mul(
        expected_generators,
        u64::from(capacities.max_event_subdivisions),
        "event records",
    )?;
    let expected_runs = expected_events
        .checked_add(1)
        .ok_or_else(|| "pixels::verify: run record arithmetic overflow".to_string())?
        .max(u64::from(super::projection_bounds::TILE_WIDTH_V1));
    if u64::from(capacities.max_event_records) != expected_events
        || u64::from(capacities.max_run_records_per_tile_row) != expected_runs
    {
        return Err("pixels::verify: event/run capacities are inconsistent".to_string());
    }
    let expected_storage = [
        (
            "candidate",
            capacities.candidate_bytes,
            checked_mul(
                u64::from(capacities.max_projected_features_per_tile),
                super::capacities::CANDIDATE_RECORD_BYTES_V1,
                "candidate bytes",
            )?,
        ),
        (
            "root",
            capacities.root_bytes,
            checked_mul(
                u64::from(capacities.max_object_roots_per_row_start),
                super::capacities::ROOT_RECORD_BYTES_V1,
                "root bytes",
            )?,
        ),
        (
            "sheet",
            capacities.sheet_bytes,
            checked_mul(
                u64::from(capacities.max_active_sheet_records_per_row),
                super::capacities::SHEET_RECORD_BYTES_V1,
                "sheet bytes",
            )?,
        ),
        (
            "event",
            capacities.event_bytes,
            checked_mul(
                expected_events,
                super::capacities::EVENT_RECORD_BYTES_V1,
                "event bytes",
            )?,
        ),
        (
            "run",
            capacities.run_bytes,
            checked_mul(
                u64::from(capacities.max_run_records_per_tile_row),
                super::capacities::RUN_RECORD_BYTES_V1,
                "run bytes",
            )?,
        ),
        (
            "corridor",
            capacities.corridor_bytes,
            checked_mul(
                u64::from(capacities.max_run_records_per_tile_row),
                super::capacities::CORRIDOR_RECORD_BYTES_V1,
                "corridor bytes",
            )?,
        ),
        (
            "fixed-q",
            capacities.fixed_q_bytes,
            checked_mul(
                u64::from(capacities.max_run_records_per_tile_row),
                super::capacities::FIXED_Q_RECORD_BYTES_V1,
                "fixed-q bytes",
            )?,
        ),
        (
            "shading",
            capacities.shading_bytes,
            checked_mul(
                u64::from(capacities.max_run_records_per_tile_row),
                super::capacities::SHADING_RECORD_BYTES_V1,
                "shading bytes",
            )?,
        ),
        (
            "transparency",
            capacities.transparency_bytes,
            checked_mul(
                checked_mul(
                    u64::from(capacities.max_run_records_per_tile_row),
                    u64::from(capacities.max_transparent_layers),
                    "transparency records",
                )?,
                super::capacities::TRANSPARENCY_LAYER_BYTES_V1,
                "transparency bytes",
            )?,
        ),
        (
            "kinetic",
            capacities.kinetic_certificate_bytes,
            checked_mul(
                u64::from(capacities.feature_count),
                super::capacities::KINETIC_CERTIFICATE_BYTES_V1,
                "kinetic bytes",
            )?,
        ),
    ];
    if let Some((name, actual, expected)) = expected_storage
        .into_iter()
        .find(|(_, actual, expected)| actual != expected)
    {
        return Err(format!(
            "pixels::verify: {name} storage bytes {actual} differ from derived {expected}"
        ));
    }
    let expected_per_worker = checked_sum(
        &[
            capacities.candidate_bytes,
            capacities.root_bytes,
            capacities.sheet_bytes,
            capacities.event_bytes,
            capacities.run_bytes,
            capacities.corridor_bytes,
            capacities.fixed_q_bytes,
            capacities.shading_bytes,
            capacities.transparency_bytes,
        ],
        "per-worker scratch",
    )?;
    if capacities.per_worker_scratch_bytes != expected_per_worker
        || capacities.all_worker_scratch_bytes
            != checked_mul(
                expected_per_worker,
                u64::from(capacities.worker_count),
                "all-worker scratch",
            )?
    {
        return Err("pixels::verify: worker scratch byte derivation is inconsistent".to_string());
    }
    if capacities.output_double_buffer_bytes
        != checked_mul(capacities.output_tile_bytes, 2, "output double buffer")?
    {
        return Err("pixels::verify: output double-buffer derivation is inconsistent".to_string());
    }
    let scanout_tile_bytes = wrela_machine::pixels::TILE_ALLOCATION_BYTES as u64;
    if capacities.output_tile_bytes % scanout_tile_bytes != 0 {
        return Err("pixels::verify: scanout generation is not a whole tile list".to_string());
    }
    let scanout_tiles = capacities.output_tile_bytes / scanout_tile_bytes;
    let expected_output_bytes = checked_mul(
        scanout_tiles,
        scanout_tile_bytes,
        "scanout generation bytes",
    )?;
    let expected_descriptor_bytes = if scanout_tiles == 0 {
        0
    } else {
        checked_mul(
            (wrela_machine::pixels::CONTROL_BYTES as u64)
                .checked_add(checked_mul(
                    scanout_tiles,
                    wrela_machine::pixels::DISPLAY_TILE_DESC_BYTES_V1 as u64,
                    "scanout descriptors",
                )?)
                .ok_or_else(|| "pixels::verify: scanout descriptor bytes overflow".to_string())?,
            2,
            "scanout descriptor generations",
        )?
    };
    if capacities.output_tile_bytes != expected_output_bytes
        || capacities.tile_descriptor_bytes != expected_descriptor_bytes
        || capacities.tile_ownership_bytes
            != checked_mul(scanout_tiles, 2, "scanout ownership bytes")?
    {
        return Err(
            "pixels::verify: scanout tile/list/ownership derivation is inconsistent".to_string(),
        );
    }
    let pre_framebuffer = checked_sum(
        &[
            capacities.state_header_bytes,
            capacities.coefficient_snapshot_bytes,
            capacities.frame_dependency_snapshot_bytes,
            capacities.frame_complex_double_buffer_bytes,
            capacities.all_worker_scratch_bytes,
        ],
        "pre-framebuffer renderer state",
    )?;
    let page = wrela_machine::layout::PIXELS_STATE_PAGE_ALIGNMENT;
    let framebuffer_offset = pre_framebuffer
        .checked_add(page - 1)
        .map(|value| value & !(page - 1))
        .ok_or_else(|| "pixels::verify: framebuffer alignment overflow".to_string())?;
    let after_framebuffer = framebuffer_offset
        .checked_add(capacities.output_double_buffer_bytes)
        .ok_or_else(|| "pixels::verify: framebuffer state overflow".to_string())?;
    let probe_offset = if capacities.probe_bytes == 0 {
        after_framebuffer
    } else {
        after_framebuffer
            .checked_add(page - 1)
            .map(|value| value & !(page - 1))
            .ok_or_else(|| "pixels::verify: probe alignment overflow".to_string())?
    };
    let expected_state = checked_sum(
        &[
            probe_offset,
            capacities.probe_bytes,
            capacities.kinetic_certificate_bytes,
            capacities.tile_descriptor_bytes,
            capacities.tile_ownership_bytes,
            capacities.failure_record_bytes,
        ],
        "renderer state",
    )?;
    if capacities.state_header_bytes != super::capacities::RENDERER_STATE_HEADER_BYTES_V1
        || capacities.coefficient_snapshot_bytes
            != checked_mul(
                u64::from(program.params.packed_bytes),
                2,
                "coefficient snapshots",
            )?
        || capacities.frame_dependency_snapshot_bytes
            != checked_mul(
                u64::from(program.params.frame_dependencies.runtime_bytes)
                    .max(super::capacities::P7_CANONICAL_FRAME_SNAPSHOT_BYTES),
                2,
                "frame dependency snapshots",
            )?
        || capacities.failure_record_bytes != super::capacities::FAILURE_RECORD_BYTES_V1
    {
        return Err(
            "pixels::verify: renderer snapshot/header/failure storage is inconsistent".to_string(),
        );
    }
    if capacities.total_renderer_state_bytes != expected_state {
        return Err("pixels::verify: total renderer-state derivation is inconsistent".to_string());
    }
    if capacities.total_renderer_state_bytes_instrumented
        != expected_state
            .checked_add(7)
            .map(|value| value & !7)
            .ok_or_else(|| "pixels::verify: telemetry alignment overflow".to_string())?
            .checked_add(capacities.telemetry_bytes_instrumented)
            .ok_or_else(|| "pixels::verify: instrumented state arithmetic overflow".to_string())?
    {
        return Err(
            "pixels::verify: instrumented renderer-state derivation is inconsistent".to_string(),
        );
    }
    let repeated = program
        .objects
        .objects
        .iter()
        .filter(|object| !object.repeat_instances.is_empty())
        .count();
    if (capacities.repeated_instance_count as usize) != repeated {
        return Err(
            "pixels::verify: repeated-instance capacity does not dominate object instances"
                .to_string(),
        );
    }
    let mut repeated_objects = BTreeSet::new();
    let mut repeated_roots = BTreeSet::new();
    for template in &program.repeats {
        if template.object.0 as usize >= program.objects.objects.len()
            || !template.certificate_must_fix_instance
        {
            return Err(format!(
                "pixels::verify: repeat template for {} is incomplete",
                template.object
            ));
        }
        if template.instance_count as usize != template.instances.len()
            || template.wrap_event_families as usize != template.wrap_events.len()
        {
            return Err(format!(
                "pixels::verify: repeat template for {} has inconsistent table counts",
                template.object
            ));
        }
        if !repeated_roots.insert(template.source_root) {
            return Err(format!(
                "pixels::verify: duplicate repeat template for {}",
                template.source_root
            ));
        }
        let translation_count = template
            .instances
            .iter()
            .try_fold(0_u32, |count, instance| {
                if instance.object.0 as usize >= program.objects.objects.len() {
                    return Err(format!(
                        "pixels::verify: repeat template references missing object {}",
                        instance.object
                    ));
                }
                if !repeated_objects.insert(instance.object) {
                    return Err(format!(
                        "pixels::verify: repeated object {} appears in multiple template instances",
                        instance.object
                    ));
                }
                let object = &program.objects.objects[instance.object.0 as usize];
                if object.source_root != template.source_root || object.repeat_instances.is_empty()
                {
                    return Err(format!(
                        "pixels::verify: repeat instance {} does not match template root {}",
                        instance.object, template.source_root
                    ));
                }
                count
                    .checked_add(u32::try_from(instance.translations.len()).map_err(|_| {
                        "pixels::verify: repeat translation count overflow".to_string()
                    })?)
                    .ok_or_else(|| "pixels::verify: repeat translation count overflow".to_string())
            })?;
        if translation_count != template.affine_translation_count {
            return Err(format!(
                "pixels::verify: repeat template for {} has inconsistent translation count",
                template.object
            ));
        }
        if template.instances.iter().any(|instance| {
            instance.translations.iter().any(|translation| {
                translation.period.lo <= 0.0
                    || !translation.period.lo.is_finite()
                    || !translation.period.hi.is_finite()
                    || !translation.translation.lo.is_finite()
                    || !translation.translation.hi.is_finite()
            })
        }) || template.wrap_events.iter().any(|event| {
            !event.boundary.lo.is_finite()
                || !event.boundary.hi.is_finite()
                || event.right_index != event.left_index.saturating_add(1)
        }) {
            return Err(format!(
                "pixels::verify: repeat template for {} has an invalid affine or wrap program",
                template.object
            ));
        }
    }
    let expected_repeated_objects = program
        .objects
        .objects
        .iter()
        .filter(|object| !object.repeat_instances.is_empty())
        .map(|object| object.id)
        .collect::<BTreeSet<_>>();
    if repeated_objects != expected_repeated_objects {
        let missing = expected_repeated_objects
            .difference(&repeated_objects)
            .next()
            .copied()
            .or_else(|| {
                repeated_objects
                    .difference(&expected_repeated_objects)
                    .next()
                    .copied()
            })
            .ok_or_else(|| {
                "pixels::verify: repeat coverage mismatch without a stable object ID".to_string()
            })?;
        return Err(format!(
            "pixels::verify: repeated object {missing} has no exact repeat template instance"
        ));
    }
    Ok(())
}

fn verify_deformations(graph: &SymbolicGraph, program: &StructuralProgram) -> Result<(), String> {
    let mut expected = BTreeMap::new();
    for (field, node) in graph.fields.iter() {
        if let FieldKind::BoundedDisplace {
            displacement,
            contract,
            ..
        } = &node.kind
        {
            expected.insert(field, (*displacement, contract.derivation));
        }
    }
    let mut seen = BTreeSet::new();
    for deformation in &program.deformations {
        if !seen.insert(deformation.field) {
            return Err(format!(
                "pixels::verify: duplicate deformation template for {}",
                deformation.field
            ));
        }
        let Some((displacement, derivation)) = expected.get(&deformation.field) else {
            return Err(format!(
                "pixels::verify: deformation template references unsupported field {}",
                deformation.field
            ));
        };
        if deformation.displacement != *displacement || deformation.derivation != *derivation {
            return Err(format!(
                "pixels::verify: deformation {} differs from its closed source derivation",
                deformation.field
            ));
        }
        if [
            deformation.amplitude,
            deformation.gradient,
            deformation.hessian,
            deformation.third_derivative,
        ]
        .iter()
        .any(|bound| !bound.is_finite() || *bound < 0.0)
        {
            return Err(format!(
                "pixels::verify: deformation {} has invalid derived bounds",
                deformation.field
            ));
        }
    }
    if seen.len() != expected.len() {
        let missing = expected
            .keys()
            .find(|field| !seen.contains(field))
            .ok_or_else(|| {
                "pixels::verify: deformation coverage mismatch without a stable field ID"
                    .to_string()
            })?;
        return Err(format!(
            "pixels::verify: deformation field {missing} has no derived template"
        ));
    }
    let exact = super::deform::compile(graph, &program.values)?;
    for (actual, expected) in program.deformations.iter().zip(&exact) {
        if actual != expected {
            return Err(format!(
                "pixels::verify: deformation {} numeric contract differs from exact closed derivation",
                expected.field
            ));
        }
    }
    Ok(())
}

fn verify_report(program: &StructuralProgram) -> Result<(), String> {
    if program.report.coefficient_bytes != program.params.packed_bytes
        || program.report.object_count != program.capacities.object_count
        || program.report.feature_count != program.capacities.feature_count
        || program.report.renderer_state_bytes != program.capacities.total_renderer_state_bytes
        || program.report.renderer_state_bytes_instrumented
            != program.capacities.total_renderer_state_bytes_instrumented
        || program.report.dependency_schema_digest != program.params.digest_schema.schema_digest
    {
        return Err("pixels::verify: structural report differs from verified inputs".to_string());
    }
    Ok(())
}

/// The exact re-derivation ratchets (`verify_exact_derivation`,
/// `verify_exact_projective_derivation`) re-run the entire structural (P3)
/// and projective (P4) pipelines and deep-compare every table, doubling the
/// cost of each renderer compile. The per-table verifiers stay on
/// unconditionally; the full re-derivation is opt-in via
/// `WRELA_PIXELS_EXACT_VERIFY=1`, which the deep verification lanes set.
/// Environment switch that turns the exact re-derivation ratchets on.
///
/// The deep lane names this constant rather than repeating the string, so the
/// ratchet cannot be silently disabled by a rename on one side: a drift becomes
/// a compile error instead of a lane that quietly stops verifying anything.
pub const EXACT_VERIFY_ENV: &str = "WRELA_PIXELS_EXACT_VERIFY";

/// The switch is opt-in and fails closed: only an exact `1` enables it, so a
/// stray empty or misspelled value cannot half-enable the ratchet.
fn exact_verification_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

fn exact_derivation_verification_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED
        .get_or_init(|| exact_verification_requested(std::env::var_os(EXACT_VERIFY_ENV).as_deref()))
}

fn verify_exact_derivation(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
    program: &StructuralProgram,
) -> Result<(), String> {
    let params = super::params::derive_layout(graph, config)?;
    if program.params != params {
        return Err(
            "pixels::verify: parameter dependencies differ from exact graph/config derivation"
                .to_string(),
        );
    }
    let values = super::bounds::propagate(graph, config)?;
    if program.values != values {
        return Err(
            "pixels::verify: scalar value bounds differ from exact rule rederivation".to_string(),
        );
    }
    let derivatives = super::derivative_bounds::propagate(graph, &values)?;
    if program.derivatives != derivatives {
        return Err(
            "pixels::verify: derivative bounds differ from exact rule rederivation".to_string(),
        );
    }
    let support = super::support::propagate(graph, &values)?;
    if program.support != support {
        return Err(
            "pixels::verify: support table differs from exact rule rederivation".to_string(),
        );
    }
    let world_bounds = super::world_bounds::derive(graph, config, &values, &support)?;
    if program.world_bounds != world_bounds {
        return Err("pixels::verify: world bounds differ from exact rule rederivation".to_string());
    }
    let objects = super::objects::partition(graph, &values, &world_bounds, &support)?;
    if program.objects != objects {
        return Err(
            "pixels::verify: object partition differs from exact rule rederivation".to_string(),
        );
    }
    let csg = super::csg::compile(objects.csg.clone(), objects.objects.len())?;
    if program.csg != csg {
        return Err(
            "pixels::verify: CSG program or cofactors differ from exact source-tree compilation"
                .to_string(),
        );
    }
    let features = super::features::decompose(graph, &objects, &values, &world_bounds, &support)?;
    if program.features != features {
        return Err(
            "pixels::verify: feature records differ from exact occurrence decomposition"
                .to_string(),
        );
    }
    let repeats = super::repeat::compile(graph, config, &objects)?;
    if program.repeats != repeats {
        return Err(
            "pixels::verify: repeat templates differ from exact instance derivation".to_string(),
        );
    }
    let deformations = super::deform::compile(graph, &values)?;
    if program.deformations != deformations {
        return Err(
            "pixels::verify: deformation contracts differ from exact closed derivation".to_string(),
        );
    }
    let material_events = super::material::compile(graph, &values, &objects, &features)?;
    if program.material_events != material_events {
        return Err(
            "pixels::verify: material events differ from exact ownership derivation".to_string(),
        );
    }
    let capacities = super::capacities::derive(
        graph,
        config,
        &program.params,
        &values,
        &objects,
        &csg,
        &features,
        &repeats,
        &deformations,
        &material_events,
    )?;
    if program.capacities != capacities {
        return Err("pixels::verify: capacities differ from exact checked derivation".to_string());
    }
    if program.report != super::report::build(&program.params, &capacities) {
        return Err("pixels::verify: report differs from exact checked derivation".to_string());
    }
    Ok(())
}

pub fn check(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
    program: StructuralProgram,
) -> Result<VerifiedStructuralProgram, String> {
    verify_graph(graph)?;
    verify_scalar_obligations(graph, &program.values)?;
    verify_analysis_coverage(graph, &program)?;
    verify_analysis_rules(graph, config, &program)?;
    verify_parameters(graph, config, &program)?;
    verify_objects(graph, &program)?;
    verify_features(graph, &program)?;
    verify_csg(&program)?;
    verify_material_events(graph, &program)?;
    verify_repeats(graph, config, &program)?;
    verify_capacities(graph, &program)?;
    verify_report(&program)?;
    verify_deformations(graph, &program)?;
    if exact_derivation_verification_enabled() {
        verify_exact_derivation(graph, config, &program)?;
    }
    Ok(VerifiedStructuralProgram(program))
}

fn verify_projective_features(
    graph: &SymbolicGraph,
    structural: &StructuralProgram,
    program: &ProjectiveProgram,
) -> Result<(), String> {
    if program.equations.features.len() != structural.features.len() {
        return Err(format!(
            "pixels::projective_verify: projective feature count {} differs from structural count {}",
            program.equations.features.len(),
            structural.features.len()
        ));
    }
    if program.derivatives.bundles.len() != structural.features.len() {
        return Err(format!(
            "pixels::projective_verify: derivative bundle count {} differs from feature count {}",
            program.derivatives.bundles.len(),
            structural.features.len()
        ));
    }
    if program.spans.len() != structural.features.len() {
        return Err(format!(
            "pixels::projective_verify: projected span count {} differs from feature count {}",
            program.spans.len(),
            structural.features.len()
        ));
    }
    let projection_camera =
        super::projection_bounds::CameraProjectionBox::from_contract(program.equations.camera)?;
    for (index, ((structural, feature), bundle)) in structural
        .features
        .iter()
        .zip(&program.equations.features)
        .zip(&program.derivatives.bundles)
        .enumerate()
    {
        if structural.id.index() != index
            || feature.feature != structural.id
            || bundle.feature != structural.id
            || bundle.id.index() != index
        {
            return Err(format!(
                "pixels::projective_verify: feature/bundle order differs at {}",
                structural.id
            ));
        }
        let root = program
            .equations
            .polynomials
            .get(feature.root_equation.index())
            .ok_or_else(|| {
                format!(
                    "pixels::projective_verify: feature {} root program is missing",
                    feature.feature
                )
            })?;
        if root.degree_q != feature.q_degree || feature.q_degree > 4 {
            return Err(format!(
                "pixels::projective_verify: feature {} q degree metadata is invalid",
                feature.feature
            ));
        }
        match feature.q_seed_kind {
            super::projective::SeedKind::Affine { denominator } => {
                let strict = match denominator.sign {
                    super::projective::StrictSign::Positive => denominator.enclosure.lo > 0.0,
                    super::projective::StrictSign::Negative => denominator.enclosure.hi < 0.0,
                };
                if feature.q_degree != 1 || !strict {
                    return Err(format!(
                        "pixels::projective_verify: feature {} affine denominator lacks strict sign",
                        feature.feature
                    ));
                }
            }
            super::projective::SeedKind::StableQuadratic {
                leading_enclosure,
                leading_sign,
                linear_fallback,
                generic_isolation_fallback,
                ..
            } => {
                let strict = leading_sign.is_some_and(|sign| match sign {
                    super::projective::StrictSign::Positive => leading_enclosure.lo > 0.0,
                    super::projective::StrictSign::Negative => leading_enclosure.hi < 0.0,
                });
                if feature.q_degree != 2
                    || (!strict && !linear_fallback)
                    || !generic_isolation_fallback
                {
                    return Err(format!(
                        "pixels::projective_verify: feature {} quadratic seed lacks its required linear/generic fallback",
                        feature.feature
                    ));
                }
            }
            super::projective::SeedKind::GenericIsolatedRoot => {}
        }
        let exact_root_isolation = match feature.q_seed_kind {
            super::projective::SeedKind::Affine { .. } => {
                super::projective::RootIsolationProgram::Affine
            }
            super::projective::SeedKind::StableQuadratic {
                linear_fallback,
                generic_isolation_fallback,
                ..
            } => super::projective::RootIsolationProgram::StableQuadratic {
                linear_fallback,
                generic_isolation_fallback,
            },
            super::projective::SeedKind::GenericIsolatedRoot => {
                super::projective::RootIsolationProgram::CertifiedBernstein {
                    maximum_depth: super::projective::BERNSTEIN_ISOLATION_DEPTH_V1,
                    ambiguity_depth: super::projective::BERNSTEIN_AMBIGUITY_DEPTH_V1,
                    preserve_all_positive_q_roots: true,
                }
            }
        };
        if feature.root_isolation != exact_root_isolation
            || feature.quadratic_composition != super::polynomial::plan_quadratic_composition(root)?
        {
            return Err(format!(
                "pixels::projective_verify: feature {} root/composition plan differs from canonical lowering",
                feature.feature
            ));
        }
        if bundle.g != feature.root_equation {
            return Err(format!(
                "pixels::projective_verify: derivative bundle {} names a different root",
                bundle.id
            ));
        }
        if program.spans[index].feature != structural.id || program.spans[index].q.lo <= 0.0 {
            return Err(format!(
                "pixels::projective_verify: feature {} has invalid projected/q span",
                structural.id
            ));
        }
        super::projection_bounds::verify_independent_corner_samples(
            structural.world_bounds,
            projection_camera,
            program.equations.camera,
            &program.spans[index],
        )?;
        let deformations = program
            .deformations
            .iter()
            .filter(|deformation| deformation.feature == structural.id)
            .count();
        if feature.deformed_predictor != (deformations == 1) {
            return Err(format!(
                "pixels::projective_verify: feature {} deformation predictor/program mismatch",
                structural.id
            ));
        }
    }
    if program.derivatives.clusters.len() != structural.objects.objects.len() {
        return Err(format!(
            "pixels::projective_verify: smooth root cluster count {} differs from object count {}",
            program.derivatives.clusters.len(),
            structural.objects.objects.len()
        ));
    }
    for (index, (object, cluster)) in structural
        .objects
        .objects
        .iter()
        .zip(&program.derivatives.clusters)
        .enumerate()
    {
        let mut expected_signature = object.primitive_occurrences.clone();
        expected_signature.sort();
        expected_signature.dedup();
        let expected_bundles = structural
            .features
            .iter()
            .filter(|feature| feature.object == object.id)
            .map(|feature| {
                program
                    .derivatives
                    .bundles
                    .get(feature.id.index())
                    .map(|bundle| bundle.id)
                    .ok_or_else(|| {
                        format!(
                            "pixels::projective_verify: smooth object {} lacks predictor bundle for {}",
                            object.id, feature.id
                        )
                    })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let derivative = structural.derivatives.get(object.scalar_root)?;
        let (world_delta, remainder) = super::events::quadratic_taylor_remainder(
            derivative.third_derivative_norm,
            object.bounds,
        )?;
        let maximum_predictor_roots =
            expected_bundles.iter().try_fold(0_u32, |total, bundle| {
                total
                    .checked_add(u32::from(
                        program.equations.features[bundle.index()].max_root_count,
                    ))
                    .ok_or_else(|| {
                        "P015: smooth verifier predictor-root count overflow".to_string()
                    })
            })?;
        let requires_boundary_events =
            super::derivatives::contains_smooth_operation(graph, object.source_root)?;
        let maximum_object_roots = if requires_boundary_events {
            maximum_predictor_roots
                .checked_mul(
                    1_u32
                        .checked_shl(
                            super::capacities::PixelsCeilings::MACHINE_V1.event_isolation_depth,
                        )
                        .ok_or_else(|| {
                            "P015: smooth verifier subdivision shift overflow".to_string()
                        })?,
                )
                .ok_or_else(|| "P015: smooth verifier root count overflow".to_string())?
        } else {
            maximum_predictor_roots
        };
        let root = &cluster.root_tube;
        if cluster.object.index() != index
            || cluster.object != object.id
            || cluster.leaf_signature != expected_signature
            || cluster.bundles != expected_bundles
            || root.scalar_root != object.scalar_root
            || root.scalar_derivative_sources != [object.scalar_root]
            || root.value_domain != structural.values.get(object.scalar_root)?
            || root.first_world_abs != derivative.world_components.map(f64::abs)
            || root.second_world_abs != derivative.hessian_norm
            || root.third_world_abs != derivative.third_derivative_norm
            || root.parameter_abs
                != derivative
                    .parameter
                    .iter()
                    .map(|(parameter, value)| (*parameter, value.abs()))
                    .collect::<Vec<_>>()
            || root.frame_delta_abs != derivative.frame_delta.map(f64::abs)
            || root.frame_second_delta_abs != derivative.frame_second_delta.map(f64::abs)
            || root.taylor_order != 2
            || u32::from(root.subdivision_depth)
                != super::capacities::PixelsCeilings::MACHINE_V1.event_isolation_depth
            || root.world_delta_abs_bound.to_bits() != world_delta.to_bits()
            || root.remainder.to_bits() != remainder.to_bits()
            || root.maximum_predictor_roots != maximum_predictor_roots
            || root.maximum_object_roots != maximum_object_roots
            || root.requires_boundary_events != requires_boundary_events
            || maximum_predictor_roots == 0
        {
            return Err(format!(
                "pixels::projective_verify: smooth object {} lacks its exact composed-scalar root tube/derivative cluster",
                object.id
            ));
        }
    }
    for (index, node) in program.equations.coefficients.nodes.iter().enumerate() {
        if node.id.index() != index {
            return Err(format!(
                "pixels::projective_verify: coefficient {} is out of dense order {index}",
                node.id
            ));
        }
    }
    let mut canonical_equations = program.equations.clone();
    super::projective::canonicalize_coefficient_ids(&mut canonical_equations)?;
    if canonical_equations.coefficients != program.equations.coefficients
        || canonical_equations.polynomials != program.equations.polynomials
        || canonical_equations
            .features
            .iter()
            .map(|feature| feature.q_seed_kind)
            .ne(program
                .equations
                .features
                .iter()
                .map(|feature| feature.q_seed_kind))
    {
        return Err(
            "pixels::projective_verify: coefficient IDs are not in canonical semantic order"
                .to_string(),
        );
    }
    for (index, polynomial) in program.equations.polynomials.iter().enumerate() {
        if polynomial.id.index() != index {
            return Err(format!(
                "pixels::projective_verify: polynomial {} is out of dense order {index}",
                polynomial.id
            ));
        }
        if polynomial.terms.len() > super::polynomial::MAX_TERMS_PER_PROGRAM_V1 {
            return Err(format!(
                "pixels::projective_verify: polynomial {} exceeds the term ceiling",
                polynomial.id
            ));
        }
        let mut sorted = polynomial.terms.clone();
        sorted.sort_by(|a, b| {
            (
                std::cmp::Reverse(a.exponents.q),
                std::cmp::Reverse(a.exponents.u),
                std::cmp::Reverse(a.exponents.v),
                std::cmp::Reverse(a.exponents.x),
                std::cmp::Reverse(a.exponents.t),
                a.exponents.param_terms,
            )
                .cmp(&(
                    std::cmp::Reverse(b.exponents.q),
                    std::cmp::Reverse(b.exponents.u),
                    std::cmp::Reverse(b.exponents.v),
                    std::cmp::Reverse(b.exponents.x),
                    std::cmp::Reverse(b.exponents.t),
                    b.exponents.param_terms,
                ))
        });
        if sorted != polynomial.terms {
            return Err(format!(
                "pixels::projective_verify: polynomial {} terms are noncanonical",
                polynomial.id
            ));
        }
        if polynomial
            .terms
            .iter()
            .any(|term| term.coefficient.index() >= program.equations.coefficients.nodes.len())
        {
            return Err(format!(
                "pixels::projective_verify: polynomial {} names a missing coefficient",
                polynomial.id
            ));
        }
    }
    for (index, predicate) in program.equations.predicates.iter().enumerate() {
        if predicate.id.index() != index
            || predicate.polynomial.index() >= program.equations.polynomials.len()
        {
            return Err(format!(
                "pixels::projective_verify: predicate {} has invalid dense ID or polynomial",
                predicate.id
            ));
        }
    }
    for (index, rational) in program.equations.rationals.iter().enumerate() {
        let denominator = program
            .equations
            .polynomials
            .get(rational.denominator.index())
            .ok_or_else(|| {
                format!(
                    "pixels::projective_verify: rational {} names missing denominator {}",
                    rational.id, rational.denominator
                )
            })?;
        let numerator_exists = program
            .equations
            .polynomials
            .get(rational.numerator.index())
            .is_some();
        let proof_strict = match rational.denominator_proof.sign {
            super::projective::StrictSign::Positive => {
                rational.denominator_proof.enclosure.lo > 0.0
            }
            super::projective::StrictSign::Negative => {
                rational.denominator_proof.enclosure.hi < 0.0
            }
        };
        if rational.id.index() != index
            || rational.domain != super::ids::DomainId(0)
            || !numerator_exists
            || denominator.terms.len() != 1
            || denominator.terms[0].exponents != super::polynomial::Exponents::default()
            || denominator.terms[0].coefficient != rational.denominator_proof.coefficient
            || !proof_strict
        {
            return Err(format!(
                "pixels::projective_verify: rational {} has an invalid denominator proof contract",
                rational.id
            ));
        }
    }
    for feature in &program.equations.features {
        match (feature.q_seed_kind, feature.rational_program) {
            (super::projective::SeedKind::Affine { denominator }, Some(rational_id)) => {
                let rational = program
                    .equations
                    .rationals
                    .get(rational_id.index())
                    .filter(|rational| {
                        rational.id == rational_id && rational.denominator_proof == denominator
                    })
                    .ok_or_else(|| {
                        format!(
                            "pixels::projective_verify: affine feature {} lacks its rational program",
                            feature.feature
                        )
                    })?;
                if rational.numerator == rational.denominator {
                    return Err(format!(
                        "pixels::projective_verify: feature {} has aliased rational parts",
                        feature.feature
                    ));
                }
                let root = &program.equations.polynomials[feature.root_equation.index()];
                let mut coefficients = super::program::CoeffBuilder::from_program(
                    program.equations.coefficients.clone(),
                )?;
                let (expected_numerator, expected_denominator) =
                    super::projective::affine_rational_parts(root, &mut coefficients)?;
                let expected_numerator = expected_numerator
                    .finish(rational.numerator, super::polynomial::ProgramLimit::Feature)?;
                let expected_denominator = expected_denominator.finish(
                    rational.denominator,
                    super::polynomial::ProgramLimit::Feature,
                )?;
                if program.equations.polynomials[rational.numerator.index()] != expected_numerator
                    || program.equations.polynomials[rational.denominator.index()]
                        != expected_denominator
                {
                    return Err(format!(
                        "pixels::projective_verify: feature {} rational program does not equal its affine root equation",
                        feature.feature
                    ));
                }
            }
            (super::projective::SeedKind::Affine { .. }, None) => {
                return Err(format!(
                    "pixels::projective_verify: affine feature {} has no rational program",
                    feature.feature
                ));
            }
            (_, Some(_)) => {
                return Err(format!(
                    "pixels::projective_verify: non-affine feature {} has a rational program",
                    feature.feature
                ));
            }
            (_, None) => {}
        }
    }
    Ok(())
}

fn matching_events<'a>(
    program: &'a ProjectiveProgram,
    kind: super::event_kinds::EventKind,
    feature: Option<super::ids::FeatureId>,
) -> impl Iterator<Item = &'a super::events::EventGenerator> {
    program.events.generators.iter().filter(move |event| {
        event.kind == kind
            && feature.is_none_or(|feature| {
                event
                    .participants
                    .iter()
                    .any(|participant| participant == super::events::Participant::Feature(feature))
            })
    })
}

fn verify_event_families(
    graph: &SymbolicGraph,
    structural: &StructuralProgram,
    program: &ProjectiveProgram,
) -> Result<(), String> {
    let event_exclusions = program
        .exclusions
        .records
        .iter()
        .filter_map(|record| match record.subject {
            super::exclusions::ExclusionSubject::Event(subject) => Some(subject),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for feature in &program.equations.features {
        let silhouette_subject = super::events::EventSubject {
            kind: super::event_kinds::EventKind::Silhouette,
            feature: Some(feature.feature),
            owner: None,
            ordinal: 0,
        };
        let silhouette_count = matching_events(
            program,
            super::event_kinds::EventKind::Silhouette,
            Some(feature.feature),
        )
        .count();
        if silhouette_count + usize::from(event_exclusions.contains(&silhouette_subject)) != 1 {
            return Err(format!(
                "pixels::projective_verify: feature {} silhouette family is missing or duplicated",
                feature.feature
            ));
        }
        let structural_feature = structural
            .features
            .get(feature.feature.index())
            .ok_or_else(|| {
                format!(
                    "pixels::projective_verify: missing structural feature {}",
                    feature.feature
                )
            })?;
        let is_torus = matches!(
            graph.fields.get(structural_feature.primitive)?.kind,
            super::graph::FieldKind::Primitive(super::graph::Primitive::Torus { .. })
        );
        let is_deformed = program
            .deformations
            .iter()
            .any(|deformation| deformation.feature == feature.feature);
        if is_torus && !is_deformed && silhouette_count == 1 {
            let event = matching_events(
                program,
                super::event_kinds::EventKind::Silhouette,
                Some(feature.feature),
            )
            .next()
            .expect("counted torus silhouette");
            if event.maximum_root_count != super::events::TORUS_SILHOUETTE_ROOT_BOUND_V1
                || !matches!(
                    event.representation,
                    super::events::EventRepresentation::TorusLocalOracle { .. }
                )
            {
                return Err(format!(
                    "pixels::projective_verify: torus feature {} lacks the fixed local silhouette oracle",
                    feature.feature
                ));
            }
            let expected = super::event_kinds::EventSideMeaning {
                negative: super::event_kinds::EventSide::RecomputeRootSet,
                zero: super::event_kinds::EventSide::RecomputeRootSet,
                positive: super::event_kinds::EventSide::RecomputeRootSet,
            };
            if event.side_meaning != expected {
                return Err(format!(
                    "pixels::projective_verify: torus feature {} lacks the mandatory root-set recomputation contract",
                    feature.feature
                ));
            }
        } else if is_deformed && silhouette_count == 1 {
            let event = matching_events(
                program,
                super::event_kinds::EventKind::Silhouette,
                Some(feature.feature),
            )
            .next()
            .expect("counted deformed silhouette");
            let expected = super::event_kinds::EventSideMeaning {
                negative: super::event_kinds::EventSide::RecomputeRootSet,
                zero: super::event_kinds::EventSide::RecomputeRootSet,
                positive: super::event_kinds::EventSide::RecomputeRootSet,
            };
            if event.side_meaning != expected {
                return Err(format!(
                    "pixels::projective_verify: deformed feature {} lacks the mandatory root-set recomputation contract",
                    feature.feature
                ));
            }
        } else if feature.q_degree == 1 && silhouette_count == 1 {
            let event = matching_events(
                program,
                super::event_kinds::EventKind::Silhouette,
                Some(feature.feature),
            )
            .next()
            .expect("counted linear silhouette");
            let super::events::EventRepresentation::LinearLeadingCoefficient { coefficient, root } =
                event.representation
            else {
                return Err(format!(
                    "pixels::projective_verify: linear feature {} lacks a signed leading-coefficient event",
                    feature.feature
                ));
            };
            let expected_sides = super::event_kinds::EventSideMeaning {
                negative: super::event_kinds::EventSide::RecomputeRootSet,
                zero: super::event_kinds::EventSide::RecomputeRootSet,
                positive: super::event_kinds::EventSide::RecomputeRootSet,
            };
            if root != feature.root_equation
                || coefficient.index() >= program.equations.polynomials.len()
                || program.equations.polynomials[coefficient.index()].degree_q != 0
                || event.side_meaning != expected_sides
            {
                return Err(format!(
                    "pixels::projective_verify: linear feature {} has an invalid leading-coefficient/side contract",
                    feature.feature
                ));
            }
        } else if feature.q_degree == 2 && silhouette_count == 1 {
            let event = matching_events(
                program,
                super::event_kinds::EventKind::Silhouette,
                Some(feature.feature),
            )
            .next()
            .expect("counted quadratic silhouette");
            let super::events::EventRepresentation::QuadraticDiscriminant { discriminant, root } =
                event.representation
            else {
                return Err(format!(
                    "pixels::projective_verify: quadratic feature {} lacks a signed discriminant event",
                    feature.feature
                ));
            };
            if root != feature.root_equation
                || discriminant.index() >= program.equations.polynomials.len()
                || program.equations.polynomials[discriminant.index()].degree_q != 0
                || event.side_meaning
                    != super::event_kinds::EventSideMeaning::crossing(
                        super::event_kinds::EventSide::Inactive,
                        super::event_kinds::EventSide::Active,
                    )
            {
                return Err(format!(
                    "pixels::projective_verify: quadratic feature {} has an invalid discriminant/side contract",
                    feature.feature
                ));
            }
        }
        for (ordinal, predicate) in feature.validity_predicates.iter().enumerate() {
            let ordinal = u32::try_from(ordinal)
                .map_err(|_| "P015: validity verification ordinal overflow".to_string())?;
            let matching = matching_events(
                program,
                super::event_kinds::EventKind::FeatureBoundary,
                Some(feature.feature),
            )
            .filter(|event| {
                matches!(
                    event.representation,
                    super::events::EventRepresentation::SparsePredicate {
                        predicate: found
                    } if found == *predicate
                )
            })
            .collect::<Vec<_>>();
            let count = matching.len();
            let subject = super::events::EventSubject {
                kind: super::event_kinds::EventKind::FeatureBoundary,
                feature: Some(feature.feature),
                owner: None,
                ordinal,
            };
            if count + usize::from(event_exclusions.contains(&subject)) != 1 {
                return Err(format!(
                    "pixels::projective_verify: feature {} validity family {ordinal} is missing or duplicated",
                    feature.feature
                ));
            }
            if let [event] = matching.as_slice() {
                let expected = super::events::feature_boundary_root_bound(
                    &program.equations,
                    feature,
                    *predicate,
                )?;
                let predicate_program = &program.equations.predicates[predicate.index()];
                let mut dependencies = feature.influencing_params.clone();
                dependencies.extend(super::projective::polynomial_influencing_params(
                    graph,
                    &program.equations,
                    predicate_program.polynomial,
                )?);
                dependencies.sort();
                dependencies.dedup();
                if event.maximum_root_count != expected
                    || event.coefficient_dependencies != dependencies
                {
                    return Err(format!(
                        "pixels::projective_verify: feature {} validity family {ordinal} root bound/dependencies differ from reconstruction",
                        feature.feature
                    ));
                }
            }
        }
        for kind in [
            super::event_kinds::EventKind::NearClip,
            super::event_kinds::EventKind::FarClip,
            super::event_kinds::EventKind::FixedPointResetOnly,
        ] {
            let subject = super::events::EventSubject {
                kind,
                feature: Some(feature.feature),
                owner: None,
                ordinal: 0,
            };
            if matching_events(program, kind, Some(feature.feature)).count()
                + usize::from(event_exclusions.contains(&subject))
                != 1
            {
                return Err(format!(
                    "pixels::projective_verify: feature {} lacks one {kind:?} family",
                    feature.feature
                ));
            }
        }
    }
    for template in &structural.repeats {
        for instance in &template.instances {
            for feature in structural
                .features
                .iter()
                .filter(|feature| feature.object == instance.object)
            {
                for ordinal in 0..template.wrap_events.len() {
                    let ordinal = u32::try_from(ordinal)
                        .map_err(|_| "P015: repeat verification ordinal overflow".to_string())?;
                    let subject = super::events::EventSubject {
                        kind: super::event_kinds::EventKind::RepeatBoundary,
                        feature: Some(feature.id),
                        owner: Some(instance.object),
                        ordinal,
                    };
                    let emitted = program
                        .events
                        .ledger
                        .iter()
                        .filter(|entry| entry.subject == subject && entry.emitted.is_some())
                        .count();
                    if emitted + usize::from(event_exclusions.contains(&subject)) != 1 {
                        return Err(format!(
                            "pixels::projective_verify: feature {} repeat boundary {ordinal} for {} is missing or duplicated",
                            feature.id, instance.object,
                        ));
                    }
                }
            }
        }
    }
    let mut smooth_ordinal = 0_u32;
    for (field, node) in graph.fields.iter() {
        let (left_field, right_field, radius, left_negated, right_negated) = match node.kind {
            super::graph::FieldKind::SmoothUnion { a, b, k } => (a, b, k, false, false),
            super::graph::FieldKind::SmoothIntersection { a, b, k } => (a, b, k, true, true),
            super::graph::FieldKind::SmoothSubtract { a, b, k } => (a, b, k, true, false),
            _ => continue,
        };
        let left = graph.fields.get(left_field)?.scalar_value;
        let right = graph.fields.get(right_field)?.scalar_value;
        for feature in structural.features.iter().filter(|feature| {
            feature
                .occurrence_path
                .iter()
                .any(|step| step.field == field)
        }) {
            for kind in [
                super::event_kinds::EventKind::SmoothBandEnter,
                super::event_kinds::EventKind::SmoothCenterTie,
            ] {
                let subject = super::events::EventSubject {
                    kind,
                    feature: Some(feature.id),
                    owner: Some(feature.object),
                    ordinal: smooth_ordinal,
                };
                let count = program
                    .events
                    .ledger
                    .iter()
                    .filter(|entry| entry.subject == subject && entry.emitted.is_some())
                    .count();
                if count + usize::from(event_exclusions.contains(&subject)) != 1 {
                    return Err(format!(
                        "pixels::projective_verify: feature {} lacks one {kind:?} family for {field}",
                        feature.id
                    ));
                }
                if count == 1 {
                    let event = program
                        .events
                        .ledger
                        .iter()
                        .find(|entry| entry.subject == subject)
                        .and_then(|entry| entry.emitted)
                        .and_then(|id| program.events.generators.get(id.index()))
                        .ok_or_else(|| {
                            format!(
                                "pixels::projective_verify: smooth event {subject:?} has no generator"
                            )
                        })?;
                    let expected_root_bound = super::events::smooth_event_root_bound(
                        field,
                        kind,
                        feature,
                        &structural.features,
                        &program.equations.features,
                    )?;
                    let operands_match = match (&event.representation, kind) {
                        (
                            super::events::EventRepresentation::SmoothBandTaylorPredicate {
                                left: found_left,
                                right: found_right,
                                left_negated: found_left_negated,
                                right_negated: found_right_negated,
                                radius: found_radius,
                                ..
                            },
                            super::event_kinds::EventKind::SmoothBandEnter,
                        ) => {
                            (*found_left, *found_right, *found_radius) == (left, right, radius)
                                && (*found_left_negated, *found_right_negated)
                                    == (left_negated, right_negated)
                        }
                        (
                            super::events::EventRepresentation::SmoothTieTaylorPredicate {
                                left: found_left,
                                right: found_right,
                                left_negated: found_left_negated,
                                right_negated: found_right_negated,
                                ..
                            },
                            super::event_kinds::EventKind::SmoothCenterTie,
                        ) => {
                            (*found_left, *found_right) == (left, right)
                                && (*found_left_negated, *found_right_negated)
                                    == (left_negated, right_negated)
                        }
                        _ => false,
                    };
                    if !operands_match || event.maximum_root_count != expected_root_bound {
                        return Err(format!(
                            "pixels::projective_verify: smooth event {} does not match the CSG operand transformation/root bound for {field}",
                            event.id
                        ));
                    }
                }
            }
        }
        smooth_ordinal = smooth_ordinal
            .checked_add(1)
            .ok_or_else(|| "P015: smooth verification ordinal overflow".to_string())?;
    }
    for (ordinal, material) in structural.material_events.iter().enumerate() {
        let ordinal_u32 = u32::try_from(ordinal)
            .map_err(|_| "P015: material verification ordinal overflow".to_string())?;
        for feature in &material.feature_owners {
            let count = matching_events(
                program,
                super::event_kinds::EventKind::MaterialBoundary,
                Some(*feature),
            )
            .filter(|event| {
                event.participants.iter().any(|participant| {
                    participant == super::events::Participant::MaterialEvent(ordinal as u32)
                })
            })
            .count();
            let subject = super::events::EventSubject {
                kind: super::event_kinds::EventKind::MaterialBoundary,
                feature: Some(*feature),
                owner: structural
                    .features
                    .get(feature.index())
                    .map(|feature| feature.object),
                ordinal: ordinal_u32,
            };
            if count + usize::from(event_exclusions.contains(&subject)) != 1 {
                return Err(format!(
                    "pixels::projective_verify: feature {feature} material boundary {ordinal} is missing or duplicated"
                ));
            }
        }
    }
    Ok(())
}

fn verify_accounting(
    structural: &StructuralProgram,
    program: &ProjectiveProgram,
) -> Result<(), String> {
    let verify_scalar_derivatives = |event: &super::events::EventGenerator,
                                     derivatives: &super::events::ScalarDerivativeProgram|
     -> Result<(), String> {
        let expected = super::events::scalar_derivative_program(structural, &derivatives.sources)?;
        if derivatives.sources.is_empty()
            || derivatives != &expected
            || derivatives
                .sources
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || derivatives
                .first_world_abs
                .iter()
                .chain([derivatives.second_world_abs, derivatives.third_world_abs].iter())
                .chain(derivatives.parameter_abs.iter().map(|(_, value)| value))
                .any(|value| !value.is_finite() || *value < 0.0)
            || derivatives
                .parameter_abs
                .windows(2)
                .any(|pair| pair[0].0 >= pair[1].0)
            || derivatives
                .parameter_abs
                .iter()
                .any(|(parameter, _)| !event.coefficient_dependencies.contains(parameter))
        {
            return Err(format!(
                "pixels::projective_verify: event {} has an invalid scalar derivative program",
                event.id
            ));
        }
        Ok(())
    };
    let verify_taylor_remainder = |event: &super::events::EventGenerator,
                                   taylor_order: u8,
                                   world_delta_abs_bound: f64,
                                   third_derivative_abs_bound: f64,
                                   remainder: f64|
     -> Result<(), String> {
        let feature = event
            .participants
            .iter()
            .find_map(|participant| match participant {
                super::events::Participant::Feature(feature) => Some(feature),
                _ => None,
            })
            .and_then(|feature| structural.features.get(feature.index()))
            .ok_or_else(|| {
                format!(
                    "pixels::projective_verify: Taylor event {} lacks one structural feature",
                    event.id
                )
            })?;
        let (expected_delta, expected_remainder) = super::events::quadratic_taylor_remainder(
            third_derivative_abs_bound,
            feature.world_bounds,
        )?;
        if taylor_order != 2
            || world_delta_abs_bound.to_bits() != expected_delta.to_bits()
            || remainder.to_bits() != expected_remainder.to_bits()
        {
            return Err(format!(
                "pixels::projective_verify: event {} has an invalid complete-domain Taylor remainder",
                event.id
            ));
        }
        Ok(())
    };
    for event in &program.events.generators {
        match &event.representation {
            super::events::EventRepresentation::TorusLocalOracle {
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
            } => {
                let feature = event
                    .participants
                    .iter()
                    .find_map(|participant| match participant {
                        super::events::Participant::Feature(feature) => Some(feature),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        format!(
                            "pixels::projective_verify: torus event {} lacks a feature",
                            event.id
                        )
                    })?;
                let bundle = program
                    .derivatives
                    .bundles
                    .get(feature.index())
                    .filter(|bundle| bundle.feature == feature)
                    .ok_or_else(|| {
                        format!(
                            "pixels::projective_verify: torus event {} lacks its derivative bundle",
                            event.id
                        )
                    })?;
                let polynomial_ids = [
                    *root,
                    *derivative_u,
                    *derivative_q,
                    *derivative_uq,
                    *derivative_qq,
                    *third_u,
                ];
                if polynomial_ids
                    .into_iter()
                    .any(|id| id.index() >= program.equations.polynomials.len())
                {
                    return Err(format!(
                        "pixels::projective_verify: torus event {} names a missing polynomial",
                        event.id
                    ));
                }
                let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
                    &program.equations.coefficients,
                    &structural.values,
                    program.equations.camera,
                    polynomial_ids.into_iter().flat_map(|id| {
                        program.equations.polynomials[id.index()]
                            .terms
                            .iter()
                            .map(|term| term.coefficient)
                    }),
                )?;
                let expected_bounds = polynomial_ids
                    .map(|id| {
                        super::events::polynomial_abs_bound(
                            &program.equations.polynomials[id.index()],
                            &coefficient_intervals,
                            program.equations.camera,
                        )
                    })
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()?;
                let expected_remainder = super::events::torus_bivariate_fourth_remainder(
                    &program.equations.polynomials[root.index()],
                    &coefficient_intervals,
                    program.equations.camera,
                )?;
                let actual_bounds = [
                    *value_abs_bound,
                    *derivative_u_abs_bound,
                    *derivative_q_abs_bound,
                    *derivative_uq_abs_bound,
                    *derivative_qq_abs_bound,
                    *third_u_abs_bound,
                ];
                if [
                    *root,
                    *derivative_u,
                    *derivative_q,
                    *derivative_uq,
                    *derivative_qq,
                    *third_u,
                ] != [
                    bundle.g,
                    bundle.first.u,
                    bundle.first.q,
                    bundle.second.uq,
                    bundle.second.qq,
                    bundle.third.uuu,
                ] || [
                    value_abs_bound,
                    derivative_u_abs_bound,
                    derivative_q_abs_bound,
                    derivative_uq_abs_bound,
                    derivative_qq_abs_bound,
                    third_u_abs_bound,
                    remainder,
                ]
                .into_iter()
                .any(|bound| !bound.is_finite() || *bound < 0.0)
                    || actual_bounds
                        .iter()
                        .zip(&expected_bounds)
                        .any(|(actual, expected)| actual.to_bits() != expected.to_bits())
                    || remainder.to_bits() != expected_remainder.to_bits()
                    || *remainder <= 0.0
                    || *taylor_order != 3
                    || program.equations.polynomials[root.index()]
                        .terms
                        .iter()
                        .any(|term| u16::from(term.exponents.u) + u16::from(term.exponents.q) > 4)
                    || program.equations.polynomials[derivative_q.index()]
                        .terms
                        .iter()
                        .any(|term| u16::from(term.exponents.u) + u16::from(term.exponents.q) > 3)
                {
                    return Err(format!(
                        "pixels::projective_verify: torus event {} has an invalid local oracle",
                        event.id
                    ));
                }
            }
            super::events::EventRepresentation::DeformationTaylorPredicate {
                predictor,
                predictor_derivatives,
                scalar_derivatives,
                phase_recurrence,
                taylor_order,
                world_delta_abs_bound,
                third_derivative_abs_bound,
                remainder,
                ..
            } => {
                verify_scalar_derivatives(event, scalar_derivatives)?;
                let feature = event
                    .participants
                    .iter()
                    .find_map(|participant| match participant {
                        super::events::Participant::Feature(feature) => Some(feature),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        format!(
                            "pixels::projective_verify: deformation event {} lacks a feature",
                            event.id
                        )
                    })?;
                let deformation = program
                    .deformations
                    .iter()
                    .find(|deformation| deformation.feature == feature)
                    .ok_or_else(|| {
                        format!(
                            "pixels::projective_verify: deformation event {} lacks its deformation program",
                            event.id
                        )
                    })?;
                let expected_third = super::reference::interval::next_up(
                    deformation
                        .third_derivative_bound
                        .max(scalar_derivatives.third_world_abs),
                );
                if predictor.index() >= program.equations.polynomials.len()
                    || predictor_derivatives.index() >= program.derivatives.bundles.len()
                    || phase_recurrence.sine_coefficients
                        != super::deform::SIN_MINIMAX_ODD_COEFFICIENTS_V1.map(f64::to_bits)
                    || phase_recurrence.cosine_coefficients
                        != super::deform::COS_MINIMAX_EVEN_COEFFICIENTS_V1.map(f64::to_bits)
                    || third_derivative_abs_bound.to_bits() != expected_third.to_bits()
                {
                    return Err(format!(
                        "pixels::projective_verify: deformation event {} has an invalid Taylor/phase program",
                        event.id
                    ));
                }
                verify_taylor_remainder(
                    event,
                    *taylor_order,
                    *world_delta_abs_bound,
                    *third_derivative_abs_bound,
                    *remainder,
                )?;
            }
            super::events::EventRepresentation::SmoothBandTaylorPredicate {
                derivatives,
                taylor_order,
                world_delta_abs_bound,
                remainder,
                ..
            }
            | super::events::EventRepresentation::SmoothTieTaylorPredicate {
                derivatives,
                taylor_order,
                world_delta_abs_bound,
                remainder,
                ..
            }
            | super::events::EventRepresentation::MaterialDifferenceTaylorPredicate {
                derivatives,
                taylor_order,
                world_delta_abs_bound,
                remainder,
                ..
            } => {
                verify_scalar_derivatives(event, derivatives)?;
                verify_taylor_remainder(
                    event,
                    *taylor_order,
                    *world_delta_abs_bound,
                    derivatives.third_world_abs,
                    *remainder,
                )?;
            }
            _ => {}
        }
    }
    for span in &program.spans {
        let exclusions = program
            .exclusions
            .records
            .iter()
            .filter(|record| {
                record.subject == super::exclusions::ExclusionSubject::Candidate(span.feature)
            })
            .count();
        let indexed = program
            .indexes
            .tile_features
            .ids
            .iter()
            .any(|id| *id == span.feature.0);
        if span.rule == super::projection_bounds::ProjectionRule::OutsideNearFar {
            if exclusions != 1
                || indexed
                || span
                    .outside_margin
                    .is_none_or(|margin| !margin.is_finite() || margin <= 0.0)
            {
                return Err(format!(
                    "pixels::projective_verify: omitted candidate {} is not excluded exactly once",
                    span.feature
                ));
            }
        } else if exclusions != 0 || !indexed || span.outside_margin.is_some() {
            return Err(format!(
                "pixels::projective_verify: candidate {} is neither indexed nor correctly unexcluded",
                span.feature
            ));
        }
    }
    let mut event_subjects = BTreeSet::new();
    for entry in &program.events.ledger {
        if !event_subjects.insert(entry.subject) {
            return Err(format!(
                "pixels::projective_verify: event subject {:?} is accounted more than once",
                entry.subject
            ));
        }
        match (entry.emitted, entry.omission.as_ref()) {
            (Some(event), None) => {
                if program
                    .events
                    .generators
                    .get(event.index())
                    .is_none_or(|generator| generator.id != event)
                {
                    return Err(format!(
                        "pixels::projective_verify: event subject {:?} names missing {event}",
                        entry.subject
                    ));
                }
            }
            (None, Some(_)) => {
                let count = program
                    .exclusions
                    .records
                    .iter()
                    .filter(|record| {
                        record.subject == super::exclusions::ExclusionSubject::Event(entry.subject)
                    })
                    .count();
                if count != 1 {
                    return Err(format!(
                        "pixels::projective_verify: omitted event {:?} has {count} exclusion records",
                        entry.subject
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "pixels::projective_verify: event {:?} is neither emitted nor excluded exactly once",
                    entry.subject
                ));
            }
        }
    }
    let expected_pairs = program
        .equations
        .features
        .len()
        .checked_mul(program.equations.features.len().saturating_sub(1))
        .and_then(|value| value.checked_div(2))
        .ok_or_else(|| "P015: competition subject count overflow".to_string())?;
    if program.competitions.ledger.len() != expected_pairs {
        return Err(format!(
            "pixels::projective_verify: competition ledger count {} differs from complete pair count {expected_pairs}",
            program.competitions.ledger.len()
        ));
    }
    let count_omissions = |predicate: fn(&super::competition::PairOmissionHint) -> bool| {
        program
            .competitions
            .ledger
            .iter()
            .filter_map(|decision| decision.omission.as_ref())
            .filter(|omission| predicate(omission))
            .count()
    };
    let expected_projected = count_omissions(|omission| {
        matches!(
            omission,
            super::competition::PairOmissionHint::ProjectedBoundsDisjoint
        )
    });
    let expected_q = count_omissions(|omission| {
        matches!(
            omission,
            super::competition::PairOmissionHint::QRangesDisjoint
        )
    });
    let expected_csg_global = count_omissions(|omission| {
        matches!(
            omission,
            super::competition::PairOmissionHint::CsgNonInfluential
        )
    });
    let expected_csg_pair = count_omissions(|omission| {
        matches!(
            omission,
            super::competition::PairOmissionHint::CsgJointInfluenceImpossible { .. }
        )
    });
    let expected_strict_order = count_omissions(|omission| {
        matches!(
            omission,
            super::competition::PairOmissionHint::StaticStrictOrder
                | super::competition::PairOmissionHint::GlobalStrictOrder
        )
    });
    let expected_material_only =
        structural
            .material_events
            .iter()
            .try_fold(0_u32, |count, event| {
                count
                    .checked_add(u32::try_from(event.feature_owners.len()).map_err(|_| {
                        "P015: material-only verification owner count overflow".to_string()
                    })?)
                    .ok_or_else(|| "P015: material-only verification count overflow".to_string())
            })?;
    if usize::try_from(program.competitions.pruned_projected).ok() != Some(expected_projected)
        || usize::try_from(program.competitions.pruned_q).ok() != Some(expected_q)
        || usize::try_from(program.competitions.pruned_csg_global).ok() != Some(expected_csg_global)
        || usize::try_from(program.competitions.pruned_csg_pair).ok() != Some(expected_csg_pair)
        || usize::try_from(program.competitions.pruned_strict_order).ok()
            != Some(expected_strict_order)
        || usize::try_from(program.competitions.suppressed_same_feature).ok()
            != Some(program.equations.features.len())
        || program.competitions.suppressed_material_only != expected_material_only
    {
        return Err(
            "pixels::projective_verify: competition pruning/audit counts differ from the complete decision ledger"
                .to_string(),
        );
    }
    let mut pair_subjects = BTreeSet::new();
    for decision in &program.competitions.ledger {
        if !pair_subjects.insert(decision.subject) {
            return Err(format!(
                "pixels::projective_verify: competition {:?} is accounted more than once",
                decision.subject
            ));
        }
        match (decision.emitted, decision.omission) {
            (Some(pair), None) => {
                let pair = program
                    .competitions
                    .pairs
                    .get(pair.index())
                    .ok_or_else(|| {
                        format!(
                            "pixels::projective_verify: competition {:?} names missing pair",
                            decision.subject
                        )
                    })?;
                let event = program
                    .events
                    .generators
                    .get(pair.event.index())
                    .ok_or_else(|| {
                        format!(
                            "pixels::projective_verify: pair {} names missing event {}",
                            pair.id, pair.event
                        )
                    })?;
                if event.kind != super::event_kinds::EventKind::DepthSwap {
                    return Err(format!(
                        "pixels::projective_verify: pair {} event is not DepthSwap",
                        pair.id
                    ));
                }
                match &event.representation {
                    super::events::EventRepresentation::DirectDepthCrossProduct {
                        numerator,
                        denominator_a,
                        denominator_b,
                    } => {
                        if numerator.index() >= program.equations.polynomials.len()
                            || event.side_meaning
                                != super::competition::direct_depth_side_meaning(
                                    *denominator_a,
                                    *denominator_b,
                                )
                        {
                            return Err(format!(
                                "pixels::projective_verify: pair {} direct depth sign contract is invalid",
                                pair.id
                            ));
                        }
                    }
                    super::events::EventRepresentation::TaylorDepthDifference {
                        a,
                        b,
                        taylor_order,
                        remainder,
                    } => {
                        let a_bundle = &program.derivatives.bundles[pair.a.index()];
                        let b_bundle = &program.derivatives.bundles[pair.b.index()];
                        let expected = super::competition::depth_taylor_remainder_program(
                            program.equations.camera,
                            pair.pixels,
                            pair.q_overlap,
                            program.spans[pair.a.index()].q,
                            program.spans[pair.b.index()].q,
                            a_bundle,
                            b_bundle,
                        )?;
                        if *a != a_bundle.id
                            || *b != b_bundle.id
                            || *taylor_order != 2
                            || *remainder != expected
                            || !remainder.requires_strict_g_q
                            || !remainder.discard_taylor_on_fallback
                            || remainder.next_derivative_order != 3
                            || remainder.fallback_remainder_abs_bound
                                < remainder.fallback_difference.abs_upper()
                        {
                            return Err(format!(
                                "pixels::projective_verify: pair {} Taylor depth remainder/domain program is invalid",
                                pair.id
                            ));
                        }
                    }
                    _ => {
                        return Err(format!(
                            "pixels::projective_verify: pair {} depth event has a non-depth representation",
                            pair.id
                        ));
                    }
                }
            }
            (None, Some(_)) => {
                if let Some(super::competition::PairOmissionHint::CsgJointInfluenceImpossible {
                    variable_count,
                    states_checked,
                }) = decision.omission
                {
                    let a_object = structural.features[decision.subject.a.index()].object;
                    let b_object = structural.features[decision.subject.b.index()].object;
                    let expected = super::csg::pair_influence(&structural.csg, a_object, b_object)?;
                    if expected
                        != (super::csg::PairInfluence::ProvenDisjoint {
                            variable_count,
                            states_checked,
                        })
                    {
                        return Err(format!(
                            "pixels::projective_verify: competition {:?} pairwise CSG proof does not reconstruct",
                            decision.subject
                        ));
                    }
                }
                let count = program
                    .exclusions
                    .records
                    .iter()
                    .filter(|record| {
                        record.subject
                            == super::exclusions::ExclusionSubject::Competition(decision.subject)
                    })
                    .count();
                if count != 1 {
                    return Err(format!(
                        "pixels::projective_verify: omitted competition {:?} has {count} exclusion records",
                        decision.subject
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "pixels::projective_verify: competition {:?} is neither emitted nor excluded exactly once",
                    decision.subject
                ));
            }
        }
    }
    for (index, proof) in program.exclusions.proofs.iter().enumerate() {
        if proof.id.index() != index {
            return Err(format!(
                "pixels::projective_verify: proof {} is out of dense order {index}",
                proof.id
            ));
        }
        match &proof.payload {
            super::exclusions::ProofPayload::PositiveMargin { rule, facts } => {
                if rule.is_empty() || facts.is_empty() {
                    return Err(format!(
                        "pixels::projective_verify: proof {} has an empty margin justification",
                        proof.id
                    ));
                }
            }
            super::exclusions::ProofPayload::Bernstein(payload) => {
                if !payload.minimum_margin.is_finite()
                    || payload.minimum_margin <= 0.0
                    || !payload.outward_conversion_radius.is_finite()
                    || payload.outward_conversion_radius < 0.0
                    || payload.subdivision_tree.is_empty()
                    || payload.normalized_box.iter().any(|axis| {
                        !axis.lo.is_finite() || !axis.hi.is_finite() || axis.lo > axis.hi
                    })
                    || payload
                        .polynomial
                        .is_some_and(|id| id.index() >= program.equations.polynomials.len())
                    || payload
                        .coefficient_program_root
                        .is_some_and(|id| id.index() >= program.equations.coefficients.nodes.len())
                {
                    return Err(format!(
                        "pixels::projective_verify: Bernstein proof {} has invalid bounds or program references",
                        proof.id
                    ));
                }
                if payload.subdivision_tree.iter().any(|node| {
                    node.depth > super::exclusions::MAX_BERNSTEIN_SUBDIVISION_DEPTH_V1
                        || !node.margin.is_finite()
                        || node.sign.is_some() && node.margin <= 0.0
                }) {
                    return Err(format!(
                        "pixels::projective_verify: Bernstein proof {} has an invalid subdivision node",
                        proof.id
                    ));
                }
            }
        }
    }
    for (index, exclusion) in program.exclusions.records.iter().enumerate() {
        if exclusion.id.index() != index
            || exclusion.margin.lo <= 0.0
            || exclusion.margin.contains_zero()
        {
            return Err(format!(
                "pixels::projective_verify: exclusion {} has invalid ID or non-strict margin",
                exclusion.id
            ));
        }
        if exclusion.proof.index() >= program.exclusions.proofs.len() {
            return Err(format!(
                "pixels::projective_verify: exclusion {} names missing proof {}",
                exclusion.id, exclusion.proof
            ));
        }
        if let super::exclusions::ProofPayload::Bernstein(payload) =
            &program.exclusions.proofs[exclusion.proof.index()].payload
            && !exclusion.margin.contains(payload.minimum_margin)
        {
            return Err(format!(
                "pixels::projective_verify: exclusion {} margin does not enclose proof margin",
                exclusion.id
            ));
        }
        if exclusion
            .dependencies
            .iter()
            .any(|dependency| dependency.0 >= exclusion.proof.0)
        {
            return Err(format!(
                "pixels::projective_verify: exclusion {} dependencies are cyclic or forward",
                exclusion.id
            ));
        }
    }
    Ok(())
}

fn expected_cell_ids(
    spans: impl Iterator<Item = (u32, super::projection_bounds::TileSpan)>,
    x: u32,
    y: u32,
) -> Vec<u32> {
    let mut ids = spans
        .filter(|(_, span)| {
            span.x.start <= x && x < span.x.end && span.y.start <= y && y < span.y.end
        })
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

fn verify_indexes(
    structural: &StructuralProgram,
    program: &ProjectiveProgram,
) -> Result<(), String> {
    let indexes = &program.indexes;
    for y in 0..indexes.tiles_y {
        for x in 0..indexes.tiles_x {
            let cell = y
                .checked_mul(indexes.tiles_x)
                .and_then(|value| value.checked_add(x))
                .ok_or_else(|| "pixels::projective_verify: tile cell overflow".to_string())?;
            let expected_features = expected_cell_ids(
                program
                    .spans
                    .iter()
                    .map(|span| (span.feature.0, span.tiles)),
                x,
                y,
            );
            if indexes.tile_features.lookup(cell)? != expected_features {
                return Err(format!(
                    "pixels::projective_verify: feature index differs from slow filter at tile ({x},{y})"
                ));
            }
            let expected_events = expected_cell_ids(
                program
                    .events
                    .generators
                    .iter()
                    .map(|event| (event.id.0, event.tiles)),
                x,
                y,
            );
            if indexes.tile_events.lookup(cell)? != expected_events {
                return Err(format!(
                    "pixels::projective_verify: event index differs from slow filter at tile ({x},{y})"
                ));
            }
            let expected_pairs = expected_cell_ids(
                program
                    .competitions
                    .pairs
                    .iter()
                    .map(|pair| (pair.id.0, pair.tiles)),
                x,
                y,
            );
            if indexes.tile_competitions.lookup(cell)? != expected_pairs {
                return Err(format!(
                    "pixels::projective_verify: competition index differs from slow filter at tile ({x},{y})"
                ));
            }
        }
    }
    let mut repeat_rows = Vec::<(u32, u32, u32)>::new();
    let mut repeat_id = 0_u32;
    for template in &structural.repeats {
        for instance in &template.instances {
            let spans = structural
                .features
                .iter()
                .filter(|feature| feature.object == instance.object)
                .map(|feature| &program.spans[feature.id.index()])
                .collect::<Vec<_>>();
            repeat_rows.push((
                repeat_id,
                spans
                    .iter()
                    .map(|span| span.tiles.y.start)
                    .min()
                    .unwrap_or(0),
                spans.iter().map(|span| span.tiles.y.end).max().unwrap_or(0),
            ));
            repeat_id = repeat_id
                .checked_add(1)
                .ok_or_else(|| "P015: repeat verification ID overflow".to_string())?;
        }
    }
    for row in 0..indexes.tiles_y {
        let expected = repeat_rows
            .iter()
            .filter(|(_, start, end)| *start <= row && row < *end)
            .map(|(id, _, _)| *id)
            .collect::<Vec<_>>();
        if indexes.row_block_repeats.lookup(row)? != expected {
            return Err(format!(
                "pixels::projective_verify: repeat index differs from slow per-instance filter at row block {row}"
            ));
        }
    }
    Ok(())
}

fn verify_projective_capacities(
    structural: &StructuralProgram,
    program: &ProjectiveProgram,
) -> Result<(), String> {
    let expected = super::capacities::derive_projective(
        &structural.capacities,
        &program.equations,
        &program.derivatives,
        &program.spans,
        &program.events,
        &program.competitions,
        &program.indexes,
    )?;
    if expected != program.capacities {
        return Err(
            "pixels::projective_verify: final P4 capacities differ from completed tables"
                .to_string(),
        );
    }
    Ok(())
}

fn verify_exact_projective_derivation(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
    structural: &VerifiedStructuralProgram,
    program: &ProjectiveProgram,
) -> Result<(), String> {
    let expected = super::derive_projective_program(graph, config, structural)?;
    if expected != *program {
        return Err(
            "pixels::projective_verify: program differs from exact deterministic P4 derivation"
                .to_string(),
        );
    }
    Ok(())
}

pub fn check_projective(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
    structural: &VerifiedStructuralProgram,
    program: ProjectiveProgram,
) -> Result<VerifiedProjectiveProgram, String> {
    verify_projective_features(graph, structural.program(), &program)?;
    verify_event_families(graph, structural.program(), &program)?;
    verify_accounting(structural.program(), &program)?;
    verify_indexes(structural.program(), &program)?;
    verify_projective_capacities(structural.program(), &program)?;
    if exact_derivation_verification_enabled() {
        verify_exact_projective_derivation(graph, config, structural, &program)?;
    }
    Ok(VerifiedProjectiveProgram(program))
}

#[cfg(test)]
mod tests {
    use super::super::arena::NodeOrigin;
    use super::super::config::{RendererConfig, RgbRangeConfig, ScalarRangeConfig, Vec3Config};
    use super::super::graph::{
        Axis, ClosedDeformDerivation, DerivedDeformContract, FieldArena, FieldKind, FieldNode,
        Primitive,
    };
    use super::super::ids::{ParamId, ScalarId};
    use super::super::material_graph::{
        MaterialArena, MaterialKind, MaterialNode, MaterialSampleNode, NormalModel,
    };
    use super::super::scalar::{CompareOp, Dependency, ScalarArena, ScalarNode, ScalarOp};
    use super::super::symbolic::{ParamRecord, SymbolicGraph};
    use super::*;
    use crate::sema::types::Type;

    #[test]
    fn every_dynamic_frame_record_family_rejects_truncation() {
        use wrela_machine::pixels::FrameProgramTableKindV1 as Kind;

        let cases = [
            (Kind::Scalar, 29),
            (Kind::Field, 27),
            (Kind::Object, 1),
            (Kind::Feature, 1),
            (Kind::Material, 3),
            (Kind::Parameter, 1),
            (Kind::Event, 4),
            (Kind::FixedDomain, 2),
            (Kind::FixedDomain, 4),
            (Kind::FixedDomain, 20),
            (Kind::FixedDomain, 23),
            (Kind::FixedDomain, 24),
            (Kind::FixedDomain, 26),
            (Kind::FixedDomain, 28),
            (Kind::FixedDomain, 30),
        ];
        for (kind, tag) in cases {
            let record = super::super::program::FrameRecord {
                stable_id: 0,
                tag,
                flags: 0,
                operands: Vec::new(),
            };
            assert!(
                wrela_machine::pixels::verify_frame_record_shape_v1(kind, &record).is_err(),
                "{} opcode {tag} accepted a truncated record",
                kind.stable_name()
            );
        }
    }

    fn empty_graph() -> SymbolicGraph {
        SymbolicGraph {
            renderer_index: 0,
            field_key: String::new(),
            material_key: String::new(),
            params_type: crate::sema::types::Type::Unit,
            material_type: crate::sema::types::Type::Unit,
            params: Vec::new(),
            scalar: super::super::scalar::ScalarArena::new(1),
            fields: super::super::graph::FieldArena::new(2),
            materials: super::super::material_graph::MaterialArena::new(3),
            field_root: FieldId(0),
            material_root: MaterialId(0),
            obligations: Vec::new(),
            quota: Default::default(),
        }
    }

    fn empty_program() -> StructuralProgram {
        StructuralProgram {
            params: ParameterLayout {
                slots: Vec::new(),
                packed_bytes: 0,
                frame_dependencies: super::super::params::FrameDependencyTuple {
                    fields: Vec::new(),
                    runtime_bytes: 0,
                    camera_contract: [0.0; 9],
                    light_capacity: 0,
                    light_kinds: Vec::new(),
                    environment_min: [0.0; 3],
                    environment_max: [0.0; 3],
                    exposure: [0.0; 2],
                    post_id: String::new(),
                    ao_version: 0,
                    probe_version: 0,
                    output_mode: String::new(),
                    deterministic_frame_phase: [0; 2],
                },
                digest_schema: super::super::params::DependencyDigestSchema {
                    fields: Vec::new(),
                    schema_digest: String::new(),
                },
            },
            values: ValueBounds {
                scalar: BTreeMap::new(),
            },
            derivatives: DerivativeBounds {
                scalar: BTreeMap::new(),
            },
            world_bounds: WorldBounds {
                world: super::super::world_bounds::Aabb64::new([0.0; 3], [1.0; 3]).unwrap(),
                fields: BTreeMap::new(),
            },
            support: SupportTable {
                fields: BTreeMap::new(),
            },
            objects: ObjectPartition {
                objects: Vec::new(),
                identities: Vec::new(),
                csg: super::super::objects::CsgExpr::Const(false),
            },
            csg: CsgProgram {
                constant: Some(false),
                instructions: Vec::new(),
                max_stack: 0,
                influence: Vec::new(),
            },
            features: Vec::new(),
            repeats: Vec::new(),
            deformations: Vec::new(),
            material_events: Vec::new(),
            capacities: StructuralCapacities {
                worker_count: 1,
                object_count: 0,
                feature_template_count: 0,
                feature_count: 0,
                repeated_instance_count: 0,
                scalar_program_slots: 0,
                derivative_program_slots: 0,
                parameter_slots: 0,
                max_csg_stack: 0,
                max_projected_features_per_row: 0,
                max_projected_features_per_tile: 0,
                max_object_roots_per_row_start: 0,
                max_active_sheet_records_per_row: 0,
                event_generator_count: 0,
                max_event_subdivisions: 0,
                max_event_records: 0,
                max_run_records_per_tile_row: 0,
                max_csg_events_per_row: 0,
                max_transparent_layers: 0,
                max_local_rebuild_queue: 0,
                candidate_bytes: 0,
                root_bytes: 0,
                sheet_bytes: 0,
                event_bytes: 0,
                run_bytes: 0,
                corridor_bytes: 0,
                fixed_q_bytes: 0,
                shading_bytes: 0,
                transparency_bytes: 0,
                per_worker_scratch_bytes: 0,
                all_worker_scratch_bytes: 0,
                telemetry_bytes_production: 0,
                telemetry_bytes_instrumented: 0,
                output_tile_bytes: 0,
                output_double_buffer_bytes: 0,
                probe_bytes: 0,
                kinetic_certificate_bytes: 0,
                state_header_bytes: 0,
                coefficient_snapshot_bytes: 0,
                frame_dependency_snapshot_bytes: 0,
                frame_complex_double_buffer_bytes: 0,
                tile_descriptor_bytes: 0,
                tile_ownership_bytes: 0,
                failure_record_bytes: 0,
                total_renderer_state_bytes: 0,
                total_renderer_state_bytes_instrumented: 0,
                derivations: Vec::new(),
            },
            report: StructuralReport {
                coefficient_bytes: 0,
                object_count: 0,
                feature_count: 0,
                renderer_state_bytes: 0,
                renderer_state_bytes_instrumented: 0,
                dependency_schema_digest: String::new(),
            },
        }
    }

    #[test]
    fn structural_depth_ceiling_rejects_the_first_excess_level() {
        let mut graph = empty_graph();
        let mut prior = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("depth-root"),
            )
            .unwrap();
        for level in 2..=super::super::capacities::PixelsCeilings::MACHINE_V1.structural_depth + 1 {
            prior = graph
                .scalar
                .push(
                    ScalarNode {
                        op: ScalarOp::Neg(prior),
                        dependency: Dependency::Constant,
                    },
                    NodeOrigin::synthetic(format!("depth-{level}")),
                )
                .unwrap();
        }
        let error = check_input_depth(&graph).unwrap_err();
        assert!(error.contains("structural_depth"));
        assert!(error.contains("needs 1025 levels"));
    }

    #[test]
    fn scalar_obligations_are_discharged_before_structural_verification() {
        let mut graph = empty_graph();
        let denominator = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("zero-denominator"),
            )
            .unwrap();
        graph
            .obligations
            .push(super::super::symbolic::PendingObligation::Scalar(
                super::super::scalar::ProofObligation::DenominatorNonZero { denominator },
            ));
        let values = ValueBounds {
            scalar: BTreeMap::from([(
                denominator,
                super::super::bounds::ScalarBound {
                    value: super::super::reference::interval::F64Interval::point(0.0).unwrap(),
                    rule: "test",
                },
            )]),
        };
        let error = verify_scalar_obligations(&graph, &values).unwrap_err();
        assert!(error.starts_with("P004: field operation `division`"));
        assert!(error.contains("may reach zero"));
    }

    fn minimal_graph_and_config() -> (SymbolicGraph, RendererConfig) {
        let mut scalar = ScalarArena::new(1);
        let zero = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.0_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("zero"),
            )
            .unwrap();
        let one = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(1.0_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("one"),
            )
            .unwrap();
        let parameter = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Param(ParamId(0)),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("parameter"),
            )
            .unwrap();
        let geometry = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Sub(zero, parameter),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("geometry"),
            )
            .unwrap();
        let mut fields = FieldArena::new(2);
        let field_root = fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Sphere {
                        center: [zero; 3],
                        radius: parameter,
                    }),
                    scalar_value: geometry,
                },
                NodeOrigin::synthetic("sphere"),
            )
            .unwrap();
        let mut materials = MaterialArena::new(3);
        let material_root = materials
            .push(
                MaterialNode {
                    kind: MaterialKind::Sample(MaterialSampleNode {
                        base_color: [one; 3],
                        opacity: one,
                        emissive: [zero; 3],
                        roughness: parameter,
                        metallic: zero,
                        specular_level: zero,
                        ior: one,
                        normal: NormalModel::Geometric,
                        pattern: None,
                    }),
                },
                NodeOrigin::synthetic("material"),
            )
            .unwrap();
        let graph = SymbolicGraph {
            renderer_index: 0,
            field_key: "test::world".to_string(),
            material_key: "test::shade".to_string(),
            params_type: Type::Unit,
            material_type: Type::Unit,
            params: vec![
                ParamRecord {
                    id: ParamId(0),
                    path: vec![1],
                    component: None,
                    spelling: "used".to_string(),
                    ty: Type::F32,
                    range_min: 1.0,
                    range_max: 1.0,
                    exact_integer: None,
                    rate: Some((0.0, 0.0)),
                },
                ParamRecord {
                    id: ParamId(1),
                    path: vec![0],
                    component: None,
                    spelling: "unused".to_string(),
                    ty: Type::F32,
                    range_min: 0.0,
                    range_max: 1.0,
                    exact_integer: None,
                    rate: None,
                },
            ],
            scalar,
            fields,
            materials,
            field_root,
            material_root,
            obligations: Vec::new(),
            quota: Default::default(),
        };
        let config = RendererConfig {
            declaration_index: 0,
            worker_count: 1,
            params_type: Type::Unit,
            field: "test::world".to_string(),
            material: "test::shade".to_string(),
            material_type: Type::Unit,
            display_index: 0,
            display_doorbell_addr: wrela_machine::pixels::DOORBELL_ADDR,
            width: 8,
            height: 8,
            refresh_hz: 60,
            shade_hz: 60,
            profile: "AaaByteExact".to_string(),
            tone_curve: "Linear".to_string(),
            near: 0.1,
            far: 16.0,
            world_min: Vec3Config {
                x: -4.0,
                y: -4.0,
                z: -4.0,
            },
            world_max: Vec3Config {
                x: 4.0,
                y: 4.0,
                z: 4.0,
            },
            camera_pose: None,
            camera_max_motion: 0.0,
            light_capacity: 0,
            light_kinds: Vec::new(),
            light_ranges: super::super::config::default_light_ranges(),
            exposure: ScalarRangeConfig { min: 0.0, max: 0.0 },
            environment: RgbRangeConfig {
                min: [0.0; 3],
                max: [0.0; 3],
            },
            ao_enabled: false,
            ao_radius: 1.0,
            ao_strength: 1.0,
            probes_enabled: false,
            probe_initialization_worst_case_ms: 0,
            initialization_deadline_ms: 1,
            parameter_contracts: Vec::new(),
        };
        (graph, config)
    }

    fn minimal_program() -> (SymbolicGraph, StructuralProgram) {
        let (graph, config) = minimal_graph_and_config();
        let program = super::super::compile_structural_renderer(&graph, &config)
            .unwrap()
            .program()
            .clone();
        (graph, program)
    }

    #[test]
    fn p4_entry_point_requires_the_verified_structural_type() {
        let _entry: fn(
            &SymbolicGraph,
            &RendererConfig,
            &VerifiedStructuralProgram,
        ) -> Result<ProjectiveProgram, String> = super::super::derive_projective_program;
    }

    #[test]
    fn exact_parameter_layout_deduplicates_uses_and_omits_unused_fields() {
        let (_, program) = minimal_program();
        assert_eq!(program.params.slots.len(), 1);
        let slot = &program.params.slots[0];
        assert_eq!(slot.id, ParamId(0));
        assert_eq!(
            slot.uses,
            [
                super::super::params::ParamUse::Geometry,
                super::super::params::ParamUse::Material
            ]
            .into_iter()
            .collect()
        );
        assert!(slot.immutable);
        assert_eq!(program.params.packed_bytes, 4);
        assert_eq!(program.params.frame_dependencies.runtime_bytes, 72);
        let frame_uses = program
            .params
            .frame_dependencies
            .fields
            .iter()
            .map(|field| field.use_kind)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            frame_uses,
            [
                super::super::params::ParamUse::Camera,
                super::super::params::ParamUse::Light,
                super::super::params::ParamUse::Exposure,
                super::super::params::ParamUse::Post,
                super::super::params::ParamUse::Probe,
            ]
            .into_iter()
            .collect()
        );
        assert!(
            program
                .params
                .frame_dependencies
                .fields
                .iter()
                .any(|field| field.path == "frame.frame_index" && field.runtime)
        );
    }

    #[test]
    fn verifier_rejects_each_removed_structural_record_class() {
        let (graph, program) = minimal_program();

        let mut corrupt = program.clone();
        corrupt.params.slots.clear();
        assert!(
            verify_parameters(&graph, &minimal_graph_and_config().1, &corrupt)
                .unwrap_err()
                .contains("parameter layout")
        );

        let mut corrupt = program.clone();
        corrupt.values.scalar.remove(&ScalarId(0));
        assert_eq!(
            verify_analysis_coverage(&graph, &corrupt).unwrap_err(),
            "pixels::verify: value-bound count 3 differs from scalar count 4"
        );

        let mut corrupt = program.clone();
        corrupt.derivatives.scalar.remove(&ScalarId(0));
        assert_eq!(
            verify_analysis_coverage(&graph, &corrupt).unwrap_err(),
            "pixels::verify: derivative-bound count 3 differs from scalar count 4"
        );

        let mut corrupt = program.clone();
        corrupt.world_bounds.fields.remove(&FieldId(0));
        assert_eq!(
            verify_analysis_coverage(&graph, &corrupt).unwrap_err(),
            "pixels::verify: world-bound count 0 differs from field count 1"
        );

        let mut corrupt = program.clone();
        corrupt.support.fields.remove(&FieldId(0));
        assert_eq!(
            verify_analysis_coverage(&graph, &corrupt).unwrap_err(),
            "pixels::verify: support count 0 differs from field count 1"
        );

        let mut corrupt = program.clone();
        corrupt.objects.objects[0].primitive_occurrences.clear();
        assert_eq!(
            verify_objects(&graph, &corrupt).unwrap_err(),
            "pixels::verify: object o0 has no primitive occurrences"
        );

        let mut corrupt = program.clone();
        corrupt.features.clear();
        assert!(
            verify_objects(&graph, &corrupt)
                .unwrap_err()
                .contains("has 0 feature records, expected 1")
        );

        let mut corrupt = program.clone();
        corrupt.csg.instructions[0] = CsgInst::Push(ObjectId(1));
        assert_eq!(
            verify_csg(&corrupt).unwrap_err(),
            "pixels::verify: CSG references missing object o1"
        );

        let mut corrupt = program.clone();
        corrupt.capacities.feature_template_count = 0;
        assert_eq!(
            verify_capacities(&graph, &corrupt).unwrap_err(),
            "pixels::verify: a capacity does not dominate its exact table size"
        );

        let mut corrupt = program.clone();
        corrupt.report.object_count = 0;
        assert_eq!(
            verify_report(&corrupt).unwrap_err(),
            "pixels::verify: structural report differs from verified inputs"
        );

        let (_, config) = minimal_graph_and_config();
        let mut corrupt = program.clone();
        corrupt
            .values
            .scalar
            .get_mut(&ScalarId(0))
            .unwrap()
            .value
            .hi = super::super::reference::interval::next_up(
            corrupt.values.scalar[&ScalarId(0)].value.hi,
        );
        assert!(
            verify_exact_derivation(&graph, &config, &corrupt)
                .unwrap_err()
                .contains("value bounds")
        );

        let mut corrupt = program.clone();
        corrupt.csg.instructions.push(CsgInst::Not);
        assert!(
            verify_exact_derivation(&graph, &config, &corrupt)
                .unwrap_err()
                .contains("CSG program")
        );

        let mut corrupt = program;
        corrupt
            .world_bounds
            .fields
            .get_mut(&FieldId(0))
            .unwrap()
            .bounds = Some(super::super::world_bounds::Aabb64::new([-2.0; 3], [2.0; 3]).unwrap());
        assert!(
            verify_exact_derivation(&graph, &config, &corrupt)
                .unwrap_err()
                .contains("world bounds")
        );
    }

    #[test]
    fn full_domain_projection_is_honestly_audited_and_p4_overlap_cannot_exceed_p3() {
        let (graph, config) = minimal_graph_and_config();
        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();

        let full_domain = program
            .exclusions
            .records
            .iter()
            .filter(|record| {
                record.reason == super::super::exclusions::ExclusionReason::FullDomainProjection
            })
            .collect::<Vec<_>>();
        assert_eq!(full_domain.len(), 2);
        for record in full_domain {
            assert!(record.margin.contains(0.5));
            let super::super::exclusions::ProofPayload::PositiveMargin { rule, facts } =
                &program.exclusions.proofs[record.proof.index()].payload
            else {
                panic!("full-domain projection must use its geometric margin proof")
            };
            assert_eq!(*rule, "projected-boundary-outside-complete-output-domain");
            assert!(
                facts
                    .iter()
                    .any(|fact| fact.contains("nearest-center-gap=0.5"))
            );
        }

        let derive_with = |capacities: &super::super::capacities::StructuralCapacities| {
            super::super::capacities::derive_projective(
                capacities,
                &program.equations,
                &program.derivatives,
                &program.spans,
                &program.events,
                &program.competitions,
                &program.indexes,
            )
        };
        let mut undersized = structural.program().capacities.clone();
        undersized.max_projected_features_per_row = 0;
        assert_eq!(
            derive_with(&undersized).unwrap_err(),
            "P015: P4 projected row overlap 1 exceeds the sealed P3 ceiling of 0"
        );
        let mut undersized = structural.program().capacities.clone();
        undersized.max_projected_features_per_tile = 0;
        assert_eq!(
            derive_with(&undersized).unwrap_err(),
            "P015: P4 projected tile overlap 1 exceeds the sealed P3 ceiling of 0"
        );
    }

    #[test]
    fn projective_verifier_rejects_removed_event_exclusion_and_index_records() {
        let (graph, config) = minimal_graph_and_config();
        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        // This fixture's sphere sits well inside the frustum, so its sealed
        // projected q span clears both clip planes and the near/far clip
        // families are omitted as vacuous rather than emitted. Their family
        // accounting still has to be complete: dropping the exclusion that
        // stands in for the generator must fail verification exactly as
        // dropping a generator does.
        for kind in [
            super::super::event_kinds::EventKind::NearClip,
            super::super::event_kinds::EventKind::FarClip,
        ] {
            let entry = program
                .events
                .ledger
                .iter()
                .find(|entry| entry.subject.kind == kind)
                .unwrap_or_else(|| panic!("minimal projective fixture lacks {kind:?}"));
            assert!(
                matches!(
                    entry.omission,
                    Some(super::super::events::OmissionHint::ClipQOutsideFeatureQSpan { .. })
                ),
                "{kind:?} must be omitted by the clip-q separation proof, got {:?}",
                entry.omission
            );
            let subject = entry.subject;
            let mut corrupt = program.clone();
            corrupt.exclusions.records.retain(|record| {
                record.subject != super::super::exclusions::ExclusionSubject::Event(subject)
            });
            assert!(
                verify_event_families(&graph, structural.program(), &corrupt)
                    .and_then(|()| verify_accounting(structural.program(), &corrupt))
                    .is_err(),
                "removing the {kind:?} exclusion must fail verification"
            );
        }
        for kind in [
            super::super::event_kinds::EventKind::Silhouette,
            super::super::event_kinds::EventKind::FixedPointResetOnly,
        ] {
            let mut corrupt = program.clone();
            let position = corrupt
                .events
                .generators
                .iter()
                .position(|event| event.kind == kind)
                .unwrap_or_else(|| panic!("minimal projective fixture lacks {kind:?}"));
            corrupt.events.generators.remove(position);
            assert!(
                verify_event_families(&graph, structural.program(), &corrupt)
                    .and_then(|()| verify_accounting(structural.program(), &corrupt))
                    .is_err(),
                "removing {kind:?} must fail verification"
            );
        }

        let mut box_graph = graph.clone();
        box_graph.fields.get_mut(box_graph.field_root).unwrap().kind =
            FieldKind::Primitive(Primitive::Box {
                center: [ScalarId(0); 3],
                half: [ScalarId(2); 3],
            });
        let box_structural =
            super::super::compile_structural_renderer(&box_graph, &config).unwrap();
        let mut box_program =
            super::super::derive_projective_program(&box_graph, &config, &box_structural).unwrap();
        assert!(!box_program.equations.rationals.is_empty());
        let mut corrupt_rational = box_program.clone();
        corrupt_rational.equations.rationals[0]
            .denominator_proof
            .enclosure = super::super::reference::interval::F64Interval::point(0.0).unwrap();
        assert!(
            verify_projective_features(&box_graph, box_structural.program(), &corrupt_rational)
                .unwrap_err()
                .contains("rational")
        );
        let boundary = box_program
            .events
            .generators
            .iter()
            .position(|event| event.kind == super::super::event_kinds::EventKind::FeatureBoundary)
            .expect("box fixture has validity-boundary events");
        box_program.events.generators.remove(boundary);
        assert!(
            verify_event_families(&box_graph, box_structural.program(), &box_program)
                .and_then(|()| verify_accounting(box_structural.program(), &box_program))
                .is_err()
        );

        let omitted = program
            .events
            .ledger
            .iter()
            .find(|entry| entry.omission.is_some())
            .expect("minimal projective fixture has a static omission")
            .subject;
        let mut corrupt = program.clone();
        let position = corrupt
            .exclusions
            .records
            .iter()
            .position(|record| {
                record.subject == super::super::exclusions::ExclusionSubject::Event(omitted)
            })
            .expect("omitted event has an exclusion");
        corrupt.exclusions.records.remove(position);
        assert!(verify_accounting(structural.program(), &corrupt).is_err());

        let mut corrupt = program;
        let populated = corrupt
            .indexes
            .tile_events
            .cells
            .iter()
            .position(|slice| slice.count > 0)
            .expect("minimal projective fixture has indexed events");
        corrupt.indexes.tile_events.cells[populated].count -= 1;
        assert!(verify_indexes(structural.program(), &corrupt).is_err());
    }

    #[test]
    fn projective_verifier_retains_a_feature_reachable_by_an_authored_basis() {
        let (graph, mut config) = minimal_graph_and_config();
        config.far = 1.0;
        config.camera_max_motion = 1.0;
        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        assert_eq!(
            program.spans[0].rule,
            super::super::projection_bounds::ProjectionRule::EyeOrNearPlaneFullScreen
        );
        assert!(!program.events.generators.is_empty());
        assert!(!program.indexes.tile_features.ids.is_empty());
        assert!(program.spans[0].outside_margin.is_none());
        check_projective(&graph, &config, &structural, program).unwrap();
    }

    #[test]
    fn nonoverlapping_colonnade_is_pruned_before_depth_event_emission() {
        let (mut graph, mut config) = minimal_graph_and_config();
        let zero = ScalarId(0);
        let radius = ScalarId(2);
        let scalar_value = graph.fields.get(graph.field_root).unwrap().scalar_value;
        let minus_four = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32((-4.0_f32).to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("left-column-x"),
            )
            .unwrap();
        let four = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(4.0_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("column-coordinate"),
            )
            .unwrap();
        graph.fields.get_mut(graph.field_root).unwrap().kind =
            FieldKind::Primitive(Primitive::Sphere {
                center: [minus_four, zero, four],
                radius,
            });
        let right = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Sphere {
                        center: [four, zero, four],
                        radius,
                    }),
                    scalar_value,
                },
                NodeOrigin::synthetic("right-column"),
            )
            .unwrap();
        graph.field_root = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::HardUnion {
                        a: FieldId(0),
                        b: right,
                    },
                    scalar_value,
                },
                NodeOrigin::synthetic("column-union"),
            )
            .unwrap();
        config.world_min = Vec3Config {
            x: -6.0,
            y: -2.0,
            z: 0.0,
        };
        config.world_max = Vec3Config {
            x: 6.0,
            y: 2.0,
            z: 6.0,
        };
        config.width = 64;
        config.height = 32;
        config.camera_max_motion = 0.0;

        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        assert_eq!(program.competitions.ledger.len(), 1);
        assert!(program.competitions.pairs.is_empty());
        assert_eq!(program.competitions.pruned_projected, 1);
        assert!(matches!(
            program.competitions.ledger[0].omission,
            Some(super::super::competition::PairOmissionHint::ProjectedBoundsDisjoint)
        ));
        assert!(
            !program
                .events
                .generators
                .iter()
                .any(|event| event.kind == super::super::event_kinds::EventKind::DepthSwap)
        );
        let projective = check_projective(&graph, &config, &structural, program).unwrap();
        let program_set = super::super::PixelsProgramSet {
            symbolic_graphs: vec![graph],
            structural_programs: vec![structural],
            projective_programs: vec![projective],
            compiled_renderers: Vec::new(),
        };
        let mut report = String::new();
        super::super::report::append_program_set(&mut report, &program_set).unwrap();
        assert_eq!(
            report
                .lines()
                .filter(|line| line.trim_start().starts_with("CompetitionOmission "))
                .count(),
            1
        );
        assert!(report.contains(
            "CompetitionOmission a=g0 a_primitive=f0 a_source=sphere:0:0@bytes=0..0 \
             b=g1 b_primitive=f1 b_source=right-column:0:0@bytes=0..0"
        ));
        // The proof ordinal is a global counter over every exclusion family, so
        // pin the omission's shape rather than its position in that counter.
        assert!(report.lines().any(|line| {
            line.contains("reason=projected-bounds-disjoint proof=proof")
                && line.contains("domain=domain0 dependencies=[] margin=[")
        }));
        let mut repeated = String::new();
        super::super::report::append_program_set(&mut repeated, &program_set).unwrap();
        assert_eq!(report, repeated);
    }

    #[test]
    fn constant_and_zero_rate_ordered_planes_are_statically_excluded() {
        let (mut graph, config) = minimal_graph_and_config();
        let zero = ScalarId(0);
        let one = ScalarId(1);
        let parameter = ScalarId(2);
        let coordinate_z = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::CoordZ,
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("ordered-plane-z"),
            )
            .unwrap();
        let parameter_plane_scalar = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Add(coordinate_z, parameter),
                    dependency: Dependency::CoordinateAndParameter,
                },
                NodeOrigin::synthetic("zero-rate-plane-scalar"),
            )
            .unwrap();
        let two = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(2.0_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("ordered-plane-two"),
            )
            .unwrap();
        let constant_plane_scalar = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Add(coordinate_z, two),
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("constant-plane-scalar"),
            )
            .unwrap();
        let first = graph.field_root;
        let first_node = graph.fields.get_mut(first).unwrap();
        first_node.kind = FieldKind::Primitive(Primitive::Plane {
            normal: [zero, zero, one],
            offset: zero,
        });
        first_node.scalar_value = coordinate_z;
        let second = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Plane {
                        normal: [zero, zero, one],
                        offset: parameter,
                    }),
                    scalar_value: parameter_plane_scalar,
                },
                NodeOrigin::synthetic("zero-rate-ordered-plane"),
            )
            .unwrap();
        let third = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Plane {
                        normal: [zero, zero, one],
                        offset: two,
                    }),
                    scalar_value: constant_plane_scalar,
                },
                NodeOrigin::synthetic("constant-ordered-plane"),
            )
            .unwrap();
        let first_two_scalar = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Min(coordinate_z, parameter_plane_scalar),
                    dependency: Dependency::CoordinateAndParameter,
                },
                NodeOrigin::synthetic("ordered-plane-min"),
            )
            .unwrap();
        let first_two = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::HardUnion {
                        a: first,
                        b: second,
                    },
                    scalar_value: first_two_scalar,
                },
                NodeOrigin::synthetic("ordered-plane-union"),
            )
            .unwrap();
        let all_scalar = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Min(first_two_scalar, constant_plane_scalar),
                    dependency: Dependency::CoordinateAndParameter,
                },
                NodeOrigin::synthetic("all-ordered-planes"),
            )
            .unwrap();
        graph.field_root = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::HardUnion {
                        a: first_two,
                        b: third,
                    },
                    scalar_value: all_scalar,
                },
                NodeOrigin::synthetic("all-ordered-plane-union"),
            )
            .unwrap();

        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        assert!(
            structural
                .program()
                .params
                .slots
                .iter()
                .any(|slot| slot.id == ParamId(0) && slot.immutable)
        );
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        assert_eq!(program.competitions.ledger.len(), 3);
        assert_eq!(program.competitions.pruned_strict_order, 3);
        assert!(program.competitions.pairs.is_empty());
        assert!(program.competitions.ledger.iter().all(|decision| {
            decision.omission
                == Some(super::super::competition::PairOmissionHint::StaticStrictOrder)
        }));
        assert!(
            program
                .exclusions
                .records
                .iter()
                .filter(|record| {
                    matches!(
                        record.subject,
                        super::super::exclusions::ExclusionSubject::Competition(_)
                    )
                })
                .all(|record| {
                    record.reason == super::super::exclusions::ExclusionReason::StaticStrictOrder
                })
        );
        assert!(
            !program
                .events
                .generators
                .iter()
                .any(|event| event.kind == super::super::event_kinds::EventKind::DepthSwap)
        );
        check_projective(&graph, &config, &structural, program).unwrap();
    }

    #[test]
    fn mutually_exclusive_csg_influence_emits_a_pair_exclusion() {
        let (mut graph, config) = minimal_graph_and_config();
        let zero = ScalarId(0);
        let radius = ScalarId(2);
        let scalar_value = graph.fields.get(graph.field_root).unwrap().scalar_value;
        let left = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32((-0.25_f32).to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("left-object-x"),
            )
            .unwrap();
        let right = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.25_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("right-object-x"),
            )
            .unwrap();
        graph.fields.get_mut(graph.field_root).unwrap().kind =
            FieldKind::Primitive(Primitive::Sphere {
                center: [left, zero, zero],
                radius,
            });
        let b = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Sphere {
                        center: [right, zero, zero],
                        radius,
                    }),
                    scalar_value,
                },
                NodeOrigin::synthetic("object-b"),
            )
            .unwrap();
        let c = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Sphere {
                        center: [zero, zero, zero],
                        radius,
                    }),
                    scalar_value,
                },
                NodeOrigin::synthetic("object-c"),
            )
            .unwrap();
        let a_and_c = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::HardIntersection {
                        a: FieldId(0),
                        b: c,
                    },
                    scalar_value,
                },
                NodeOrigin::synthetic("a-and-c"),
            )
            .unwrap();
        let b_without_c = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::HardSubtract { a: b, b: c },
                    scalar_value,
                },
                NodeOrigin::synthetic("b-without-c"),
            )
            .unwrap();
        graph.field_root = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::HardUnion {
                        a: a_and_c,
                        b: b_without_c,
                    },
                    scalar_value,
                },
                NodeOrigin::synthetic("conditional-union"),
            )
            .unwrap();

        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        assert_eq!(structural.program().objects.objects.len(), 3);
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        let a_feature = structural
            .program()
            .features
            .iter()
            .find(|feature| feature.primitive == FieldId(0))
            .unwrap()
            .id;
        let b_feature = structural
            .program()
            .features
            .iter()
            .find(|feature| feature.primitive == b)
            .unwrap()
            .id;
        let subject = super::super::competition::CompetitionSubject {
            a: a_feature.min(b_feature),
            b: a_feature.max(b_feature),
        };
        let decision = program
            .competitions
            .ledger
            .iter()
            .find(|decision| decision.subject == subject)
            .expect("objects a and b have a competition subject");
        assert!(
            matches!(
                decision.omission,
                Some(
                    super::super::competition::PairOmissionHint::CsgJointInfluenceImpossible {
                        variable_count: 3,
                        states_checked: 8,
                    }
                )
            ),
            "{decision:?}"
        );
        let exclusion = program
            .exclusions
            .records
            .iter()
            .find(|record| {
                record.subject
                    == super::super::exclusions::ExclusionSubject::Competition(decision.subject)
            })
            .expect("pairwise CSG omission has an exclusion record");
        assert_eq!(
            exclusion.reason,
            super::super::exclusions::ExclusionReason::CsgNonInfluential
        );
        check_projective(&graph, &config, &structural, program).unwrap();
    }

    #[test]
    fn fused_validity_boundary_scales_with_deformation_root_envelope() {
        let (mut graph, config) = minimal_graph_and_config();
        graph.fields.get_mut(graph.field_root).unwrap().kind =
            FieldKind::Primitive(Primitive::Box {
                center: [ScalarId(0); 3],
                half: [ScalarId(2); 3],
            });
        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let mut program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        program.equations.features[0].max_root_count = 28;
        let feature = program.equations.features[0].clone();
        assert!(
            !feature.validity_predicates.is_empty(),
            "fused box face must have analytic validity boundaries"
        );
        let predicate = feature.validity_predicates[0];
        let bound = super::super::events::feature_boundary_root_bound(
            &program.equations,
            &feature,
            predicate,
        )
        .unwrap();
        assert!(
            bound >= 28 && bound > 6,
            "a high-frequency deformation envelope must not inherit the old constant-six boundary budget"
        );
    }

    #[test]
    fn validity_event_tracks_tangential_extent_parameters() {
        let (mut graph, config) = minimal_graph_and_config();
        graph.params[1].range_min = 0.25;
        graph.params[1].range_max = 0.75;
        let tangential_half = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Param(ParamId(1)),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("tangential-half"),
            )
            .unwrap();
        let box_scalar = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Sub(ScalarId(3), tangential_half),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("parameterized-box-scalar"),
            )
            .unwrap();
        let root = graph.fields.get_mut(graph.field_root).unwrap();
        root.kind = FieldKind::Primitive(Primitive::Box {
            center: [ScalarId(0); 3],
            half: [ScalarId(2), tangential_half, ScalarId(1)],
        });
        root.scalar_value = box_scalar;
        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        let feature = &program.equations.features[0];
        assert!(!feature.influencing_params.contains(&ParamId(1)));
        let boundary = program
            .events
            .generators
            .iter()
            .find(|event| {
                event.kind == super::super::event_kinds::EventKind::FeatureBoundary
                    && event.participants.iter().any(|participant| {
                        participant == super::super::events::Participant::Feature(feature.feature)
                    })
                    && event.coefficient_dependencies.contains(&ParamId(1))
            })
            .expect("a tangential box validity boundary depends on its extent parameter");
        assert!(boundary.coefficient_dependencies.contains(&ParamId(0)));
        check_projective(&graph, &config, &structural, program).unwrap();
    }

    #[test]
    fn moving_parameter_material_predicate_is_excluded_or_restored_from_complete_box() {
        let (mut graph, config) = minimal_graph_and_config();
        graph.params[1].range_min = -1.0;
        graph.params[1].range_max = 1.0;
        graph.params[1].rate = Some((0.25, 0.0));
        let parameter = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Param(ParamId(1)),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("material-proof-parameter"),
            )
            .unwrap();
        let squared = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Mul(parameter, parameter),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("material-proof-square"),
            )
            .unwrap();
        let half = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.5_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("material-proof-half"),
            )
            .unwrap();
        let positive = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Add(ScalarId(1), squared),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("material-proof-positive"),
            )
            .unwrap();
        let predicate = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Compare {
                        op: CompareOp::Gt,
                        a: positive,
                        b: ScalarId(0),
                    },
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("material-proof-predicate"),
            )
            .unwrap();
        let alternate = graph
            .materials
            .push(
                graph.materials.get(graph.material_root).unwrap().clone(),
                NodeOrigin::synthetic("material-proof-alternate"),
            )
            .unwrap();
        graph.material_root = graph
            .materials
            .push(
                MaterialNode {
                    kind: MaterialKind::Select {
                        predicate,
                        a: graph.material_root,
                        b: alternate,
                    },
                },
                NodeOrigin::synthetic("material-proof-select"),
            )
            .unwrap();

        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        assert!(
            !program.events.generators.iter().any(|event| {
                event.kind == super::super::event_kinds::EventKind::MaterialBoundary
            })
        );
        let exclusion = program
            .exclusions
            .records
            .iter()
            .find(|record| {
                record.reason == super::super::exclusions::ExclusionReason::MaterialClassIrrelevant
            })
            .expect("1 + p² is strictly positive over the moving parameter box");
        let super::super::exclusions::ProofPayload::Bernstein(proof) =
            &program.exclusions.proofs[exclusion.proof.index()].payload
        else {
            panic!("material strict-sign omission must carry its complete-box proof")
        };
        assert_eq!(
            proof.normalized_box,
            vec![
                super::super::exclusions::NormalizedAxis { lo: 1.0, hi: 1.0 },
                super::super::exclusions::NormalizedAxis { lo: -1.0, hi: 1.0 },
            ]
        );
        assert!(proof.minimum_margin > 0.0);
        check_projective(&graph, &config, &structural, program).unwrap();

        let mut crossing_graph = graph.clone();
        crossing_graph.scalar.get_mut(positive).unwrap().op = ScalarOp::Sub(squared, half);
        let crossing_structural =
            super::super::compile_structural_renderer(&crossing_graph, &config).unwrap();
        let crossing =
            super::super::derive_projective_program(&crossing_graph, &config, &crossing_structural)
                .unwrap();
        assert!(
            crossing.events.generators.iter().any(|event| {
                event.kind == super::super::event_kinds::EventKind::MaterialBoundary
            }),
            "p² - 0.5 crosses zero, so inconclusive static analysis must restore the runtime event"
        );
        assert!(!crossing.exclusions.records.iter().any(|record| {
            record.reason == super::super::exclusions::ExclusionReason::MaterialClassIrrelevant
        }));
        check_projective(&crossing_graph, &config, &crossing_structural, crossing).unwrap();
    }

    #[test]
    fn y_repeat_indexes_and_events_follow_each_instance_span() {
        let (mut graph, mut config) = minimal_graph_and_config();
        config.width = 64;
        config.height = 64;
        config.world_min.y = -8.0;
        config.world_max.y = 8.0;
        config.camera_max_motion = 0.25;
        let period = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(4.0_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("y-repeat-period"),
            )
            .unwrap();
        graph.field_root = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::FiniteRepeat {
                        child: graph.field_root,
                        axis: Axis::Y,
                        first: -1,
                        count: 3,
                        period,
                    },
                    scalar_value: ScalarId(3),
                },
                NodeOrigin::synthetic("projective-y-repeat"),
            )
            .unwrap();

        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        let template = &structural.program().repeats[0];
        assert_eq!(template.instances.len(), 3);
        let instance_rows = template
            .instances
            .iter()
            .map(|instance| {
                let spans = structural
                    .program()
                    .features
                    .iter()
                    .filter(|feature| feature.object == instance.object)
                    .map(|feature| program.spans[feature.id.index()].tiles.y)
                    .collect::<Vec<_>>();
                (
                    instance.object,
                    spans.iter().map(|span| span.start).min().unwrap(),
                    spans.iter().map(|span| span.end).max().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert!(
            instance_rows
                .windows(2)
                .any(|pair| (pair[0].1, pair[0].2) != (pair[1].1, pair[1].2)),
            "the regression requires repeat instances with distinct projected row spans"
        );
        for row in 0..program.indexes.tiles_y {
            let expected = instance_rows
                .iter()
                .enumerate()
                .filter(|(_, (_, start, end))| *start <= row && row < *end)
                .map(|(instance, _)| u32::try_from(instance).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                program.indexes.row_block_repeats.lookup(row).unwrap(),
                expected,
                "row-block repeat lookup must use each instance's own projected span"
            );
        }
        let repeat_owners = program
            .events
            .generators
            .iter()
            .filter(|event| event.kind == super::super::event_kinds::EventKind::RepeatBoundary)
            .filter_map(|event| {
                event.participants.iter().find_map(|participant| {
                    let super::super::events::Participant::Object(object) = participant else {
                        return None;
                    };
                    Some(object)
                })
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            repeat_owners,
            template
                .instances
                .iter()
                .map(|instance| instance.object)
                .collect(),
            "every repeat instance must own its boundary event records"
        );
        check_projective(&graph, &config, &structural, program).unwrap();
    }

    #[test]
    fn tilted_torus_uses_the_bounded_local_silhouette_oracle() {
        let (mut graph, config) = minimal_graph_and_config();
        let quarter = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.25_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("torus-minor"),
            )
            .unwrap();
        graph.fields.get_mut(graph.field_root).unwrap().kind =
            FieldKind::Primitive(Primitive::Torus {
                center: [ScalarId(0); 3],
                axis: [ScalarId(1); 3],
                major: ScalarId(1),
                minor: quarter,
            });
        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        let silhouette = program
            .events
            .generators
            .iter()
            .find(|event| event.kind == super::super::event_kinds::EventKind::Silhouette)
            .expect("tilted torus silhouette");
        assert_eq!(
            silhouette.maximum_root_count,
            super::super::events::TORUS_SILHOUETTE_ROOT_BOUND_V1
        );
        let super::super::events::EventRepresentation::TorusLocalOracle {
            derivative_qq,
            derivative_qq_abs_bound,
            remainder,
            ..
        } = &silhouette.representation
        else {
            panic!("torus silhouette must use the local numeric oracle")
        };
        assert_eq!(
            *derivative_qq, program.derivatives.bundles[0].second.qq,
            "the emitted oracle must contain the missing lower-right Jacobian entry G_qq"
        );
        assert!(derivative_qq_abs_bound.is_finite());
        assert!(*derivative_qq_abs_bound >= 0.0);
        assert!(remainder.is_finite());
        assert!(*remainder > 0.0);

        let mut corrupt = program.clone();
        corrupt
            .events
            .generators
            .iter_mut()
            .find(|event| event.kind == super::super::event_kinds::EventKind::Silhouette)
            .unwrap()
            .maximum_root_count = 4;
        assert!(verify_event_families(&graph, structural.program(), &corrupt).is_err());

        let mut corrupt_jacobian = program.clone();
        let oracle = &mut corrupt_jacobian
            .events
            .generators
            .iter_mut()
            .find(|event| event.kind == super::super::event_kinds::EventKind::Silhouette)
            .unwrap()
            .representation;
        let super::super::events::EventRepresentation::TorusLocalOracle {
            root,
            derivative_qq,
            ..
        } = oracle
        else {
            unreachable!()
        };
        *derivative_qq = *root;
        assert!(verify_accounting(structural.program(), &corrupt_jacobian).is_err());
    }

    #[test]
    fn two_quartic_smooth_boundary_uses_the_bezout_root_ceiling() {
        let (mut graph, config) = minimal_graph_and_config();
        let quarter = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.25_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("smooth-torus-minor"),
            )
            .unwrap();
        let original = graph.field_root;
        graph.fields.get_mut(original).unwrap().kind = FieldKind::Primitive(Primitive::Torus {
            center: [ScalarId(0); 3],
            axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
            major: ScalarId(1),
            minor: quarter,
        });
        let duplicate = graph
            .fields
            .push(
                graph.fields.get(original).unwrap().clone(),
                NodeOrigin::synthetic("smooth-torus-right"),
            )
            .unwrap();
        graph.field_root = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::SmoothUnion {
                        a: original,
                        b: duplicate,
                        k: quarter,
                    },
                    scalar_value: ScalarId(3),
                },
                NodeOrigin::synthetic("smooth-two-quartics"),
            )
            .unwrap();
        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        let smooth_events = program
            .events
            .generators
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    super::super::event_kinds::EventKind::SmoothBandEnter
                        | super::super::event_kinds::EventKind::SmoothCenterTie
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(smooth_events.len(), 2);
        assert!(smooth_events.iter().all(|event| {
            event.kind == super::super::event_kinds::EventKind::SmoothCenterTie
                && event.maximum_root_count == 16
        }));
        for feature in structural.program().features.iter() {
            assert_eq!(
                super::super::events::smooth_event_root_bound(
                    graph.field_root,
                    super::super::event_kinds::EventKind::SmoothBandEnter,
                    feature,
                    &structural.program().features,
                    &program.equations.features,
                )
                .unwrap(),
                32
            );
        }
        assert_eq!(
            program
                .exclusions
                .records
                .iter()
                .filter(|record| {
                    record.reason == super::super::exclusions::ExclusionReason::SupportShellDisjoint
                })
                .count(),
            2,
            "the identically separated smooth bands must be audited exclusions"
        );
        assert!(program.capacities.row_event_intervals >= 32);

        let mut corrupt = program.clone();
        corrupt
            .events
            .generators
            .iter_mut()
            .find(|event| event.kind == super::super::event_kinds::EventKind::SmoothCenterTie)
            .unwrap()
            .maximum_root_count = 8;
        assert!(verify_event_families(&graph, structural.program(), &corrupt).is_err());
    }

    #[test]
    fn smooth_interior_zero_uses_the_composed_scalar_root_tube() {
        let (mut graph, config) = minimal_graph_and_config();
        let quarter = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.25_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("smooth-interior-quarter"),
            )
            .unwrap();
        let one = ScalarId(1);
        let original = graph.field_root;
        graph.fields.get_mut(original).unwrap().scalar_value = quarter;
        let duplicate = graph
            .fields
            .push(
                graph.fields.get(original).unwrap().clone(),
                NodeOrigin::synthetic("smooth-interior-right"),
            )
            .unwrap();
        let smooth_scalar = graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::SmoothMin {
                        a: quarter,
                        b: quarter,
                        k: one,
                        semantic: super::super::scalar::SemanticOpId::SmoothMinF32V1,
                    },
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("smooth-interior-scalar"),
            )
            .unwrap();
        graph.field_root = graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::SmoothUnion {
                        a: original,
                        b: duplicate,
                        k: one,
                    },
                    scalar_value: smooth_scalar,
                },
                NodeOrigin::synthetic("smooth-interior-root"),
            )
            .unwrap();

        assert_ne!(0.25_f32, 0.0);
        assert_eq!(
            super::super::scalar::source_smooth_min(0.25, 0.25, 1.0),
            0.0
        );
        let structural = super::super::compile_structural_renderer(&graph, &config).unwrap();
        let program =
            super::super::derive_projective_program(&graph, &config, &structural).unwrap();
        let cluster = &program.derivatives.clusters[0];
        assert_eq!(cluster.root_tube.scalar_root, smooth_scalar);
        assert_ne!(cluster.root_tube.scalar_root, quarter);
        assert!(cluster.root_tube.value_domain.contains_zero());
        assert!(cluster.root_tube.requires_boundary_events);
        assert_eq!(cluster.root_tube.maximum_predictor_roots, 4);
        assert_eq!(cluster.root_tube.maximum_object_roots, 16);
        assert_eq!(cluster.leaf_signature.len(), 2);
        assert_eq!(cluster.bundles.len(), 2);

        let mut corrupt = program.clone();
        corrupt.derivatives.clusters[0].root_tube.scalar_root = quarter;
        assert!(verify_projective_features(&graph, structural.program(), &corrupt).is_err());
    }

    #[test]
    fn projective_verifier_rejects_removed_structural_event_families_and_swaps() {
        let (mut smooth_graph, config) = minimal_graph_and_config();
        smooth_graph.params[1].range_min = 0.25;
        smooth_graph.params[1].range_max = 0.5;
        smooth_graph.params[1].rate = Some((0.125, 0.0));
        let moving_k = smooth_graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Param(ParamId(1)),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("smooth-moving-k"),
            )
            .unwrap();
        let original = smooth_graph.field_root;
        let duplicate = smooth_graph
            .fields
            .push(
                smooth_graph.fields.get(original).unwrap().clone(),
                NodeOrigin::synthetic("smooth-right"),
            )
            .unwrap();
        let smooth_root = smooth_graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::SmoothUnion {
                        a: original,
                        b: duplicate,
                        k: moving_k,
                    },
                    scalar_value: ScalarId(3),
                },
                NodeOrigin::synthetic("smooth-union"),
            )
            .unwrap();
        smooth_graph.field_root = smooth_root;
        let smooth_structural =
            super::super::compile_structural_renderer(&smooth_graph, &config).unwrap();
        let smooth =
            super::super::derive_projective_program(&smooth_graph, &config, &smooth_structural)
                .unwrap();
        let band = smooth
            .events
            .generators
            .iter()
            .find(|event| event.kind == super::super::event_kinds::EventKind::SmoothBandEnter)
            .expect("smooth fixture has a band event");
        assert_eq!(
            band.side_meaning,
            super::super::event_kinds::EventSideMeaning::crossing(
                super::super::event_kinds::EventSide::Active,
                super::super::event_kinds::EventSide::Inactive,
            )
        );
        assert!(
            band.coefficient_dependencies.contains(&ParamId(1)),
            "a parameter used only by smooth k must invalidate the band event"
        );
        let super::super::events::EventRepresentation::SmoothBandTaylorPredicate {
            derivatives,
            taylor_order,
            world_delta_abs_bound,
            remainder,
            ..
        } = &band.representation
        else {
            panic!("smooth band has the wrong predicate representation")
        };
        assert!(derivatives.sources.contains(&moving_k));
        assert_eq!(*taylor_order, 2);
        let (expected_delta, expected_remainder) =
            super::super::events::quadratic_taylor_remainder(
                derivatives.third_world_abs,
                smooth_structural.program().features[0].world_bounds,
            )
            .unwrap();
        assert_eq!(world_delta_abs_bound.to_bits(), expected_delta.to_bits());
        assert_eq!(remainder.to_bits(), expected_remainder.to_bits());
        let mut corrupt_remainder = smooth.clone();
        let super::super::events::EventRepresentation::SmoothBandTaylorPredicate {
            remainder, ..
        } = &mut corrupt_remainder
            .events
            .generators
            .iter_mut()
            .find(|event| event.kind == super::super::event_kinds::EventKind::SmoothBandEnter)
            .unwrap()
            .representation
        else {
            unreachable!()
        };
        *remainder = super::super::reference::interval::next_down(*remainder);
        assert!(verify_accounting(smooth_structural.program(), &corrupt_remainder).is_err());
        for kind in [
            super::super::event_kinds::EventKind::SmoothBandEnter,
            super::super::event_kinds::EventKind::SmoothCenterTie,
        ] {
            let mut corrupt = smooth.clone();
            let position = corrupt
                .events
                .generators
                .iter()
                .position(|event| event.kind == kind)
                .unwrap_or_else(|| panic!("smooth fixture lacks {kind:?}"));
            corrupt.events.generators.remove(position);
            assert!(
                verify_event_families(&smooth_graph, smooth_structural.program(), &corrupt,)
                    .and_then(|()| verify_accounting(smooth_structural.program(), &corrupt))
                    .is_err()
            );
        }
        assert!(
            !smooth.competitions.pairs.is_empty(),
            "overlapping smooth leaves must remain competition candidates"
        );
        let mut corrupt_remainder = smooth.clone();
        let depth = corrupt_remainder
            .events
            .generators
            .iter_mut()
            .find(|event| event.kind == super::super::event_kinds::EventKind::DepthSwap)
            .expect("overlapping quadrics have a depth swap");
        let super::super::events::EventRepresentation::TaylorDepthDifference { remainder, .. } =
            &mut depth.representation
        else {
            panic!("overlapping quadrics must use a Taylor depth difference")
        };
        assert!(remainder.fallback_remainder_abs_bound > 0.0);
        remainder.fallback_remainder_abs_bound = 0.0;
        assert!(verify_accounting(smooth_structural.program(), &corrupt_remainder).is_err());
        let mut missing_remainder = smooth.clone();
        missing_remainder
            .events
            .generators
            .iter_mut()
            .find(|event| event.kind == super::super::event_kinds::EventKind::DepthSwap)
            .unwrap()
            .representation = super::super::events::EventRepresentation::FixedPointReset;
        assert!(verify_accounting(smooth_structural.program(), &missing_remainder).is_err());

        let mut corrupt_swap = smooth.clone();
        let swap = corrupt_swap
            .events
            .generators
            .iter()
            .position(|event| event.kind == super::super::event_kinds::EventKind::DepthSwap)
            .expect("overlapping smooth leaves have a depth swap");
        corrupt_swap.events.generators.remove(swap);
        assert!(verify_accounting(smooth_structural.program(), &corrupt_swap).is_err());

        let (mut repeat_graph, mut repeat_config) = minimal_graph_and_config();
        repeat_config.camera_max_motion = 0.25;
        repeat_graph.field_root = repeat_graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::FiniteRepeat {
                        child: repeat_graph.field_root,
                        axis: Axis::X,
                        first: 0,
                        count: 2,
                        period: ScalarId(1),
                    },
                    scalar_value: ScalarId(3),
                },
                NodeOrigin::synthetic("projective-repeat"),
            )
            .unwrap();
        let repeat_structural =
            super::super::compile_structural_renderer(&repeat_graph, &repeat_config).unwrap();
        let mut repeat = super::super::derive_projective_program(
            &repeat_graph,
            &repeat_config,
            &repeat_structural,
        )
        .unwrap();
        let repeat_event = repeat
            .events
            .generators
            .iter()
            .position(|event| event.kind == super::super::event_kinds::EventKind::RepeatBoundary)
            .expect("moving repeat fixture has a repeat-boundary event");
        repeat.events.generators.remove(repeat_event);
        assert!(
            verify_event_families(&repeat_graph, repeat_structural.program(), &repeat,)
                .and_then(|()| verify_accounting(repeat_structural.program(), &repeat))
                .is_err()
        );

        let (mut material_graph, material_config) = minimal_graph_and_config();
        material_graph.params[1].range_min = -0.5;
        material_graph.params[1].range_max = 0.5;
        material_graph.params[1].rate = Some((0.125, 0.0));
        let threshold = material_graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Param(ParamId(1)),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("material-moving-threshold"),
            )
            .unwrap();
        let coordinate = material_graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::CoordX,
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("projective-material-coordinate"),
            )
            .unwrap();
        let predicate = material_graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Compare {
                        op: CompareOp::Lt,
                        a: coordinate,
                        b: threshold,
                    },
                    dependency: Dependency::CoordinateAndParameter,
                },
                NodeOrigin::synthetic("projective-material-predicate"),
            )
            .unwrap();
        let alternate = material_graph
            .materials
            .push(
                material_graph
                    .materials
                    .get(material_graph.material_root)
                    .unwrap()
                    .clone(),
                NodeOrigin::synthetic("projective-material-alternate"),
            )
            .unwrap();
        material_graph.material_root = material_graph
            .materials
            .push(
                MaterialNode {
                    kind: MaterialKind::Select {
                        predicate,
                        a: material_graph.material_root,
                        b: alternate,
                    },
                },
                NodeOrigin::synthetic("projective-material-select"),
            )
            .unwrap();
        let material_structural =
            super::super::compile_structural_renderer(&material_graph, &material_config).unwrap();
        let mut material = super::super::derive_projective_program(
            &material_graph,
            &material_config,
            &material_structural,
        )
        .unwrap();
        let material_event = material
            .events
            .generators
            .iter()
            .position(|event| event.kind == super::super::event_kinds::EventKind::MaterialBoundary)
            .expect("material select fixture has a material-boundary event");
        material.events.generators.remove(material_event);
        assert!(
            verify_event_families(&material_graph, material_structural.program(), &material,)
                .and_then(|()| verify_accounting(material_structural.program(), &material))
                .is_err()
        );
    }

    #[test]
    fn verifier_rejects_corrupt_proof_record_fields_specifically() {
        let (graph, program) = minimal_program();
        let (_, config) = minimal_graph_and_config();

        let mut corrupt = program.clone();
        corrupt.features[0].orientation =
            super::super::primitive::OrientationProgram::DeformedInward;
        assert!(
            verify_features(&graph, &corrupt)
                .unwrap_err()
                .contains("orientation differs")
        );

        let mut corrupt = program.clone();
        corrupt.features[0].validity.shared_boundary =
            !corrupt.features[0].validity.shared_boundary;
        assert!(
            verify_features(&graph, &corrupt)
                .unwrap_err()
                .contains("validity predicate differs")
        );

        let mut corrupt = program.clone();
        corrupt.features[0].world_bounds.max[0] =
            super::super::reference::interval::next_down(corrupt.features[0].world_bounds.max[0]);
        assert!(
            verify_features(&graph, &corrupt)
                .unwrap_err()
                .contains("world bounds differ")
        );

        let mut corrupt = program.clone();
        corrupt.features[0].identity_set = u32::MAX;
        assert!(
            verify_features(&graph, &corrupt)
                .unwrap_err()
                .contains("identity differs")
        );

        let mut corrupt = program.clone();
        corrupt
            .support
            .fields
            .get_mut(&FieldId(0))
            .unwrap()
            .max_budget
            .hi = super::super::reference::interval::next_up(
            corrupt.support.fields[&FieldId(0)].max_budget.hi,
        );
        assert!(
            verify_analysis_rules(&graph, &config, &corrupt)
                .unwrap_err()
                .contains("support paths or budgets")
        );

        let mut corrupt = program.clone();
        corrupt
            .support
            .fields
            .get_mut(&FieldId(0))
            .unwrap()
            .leaf_supports[0]
            .path
            .clear();
        assert!(
            verify_analysis_rules(&graph, &config, &corrupt)
                .unwrap_err()
                .contains("support paths or budgets")
        );

        let mut corrupt = program;
        corrupt.csg.influence[0].when_false.constant = corrupt.csg.influence[0]
            .when_false
            .constant
            .map(|value| !value);
        assert!(
            verify_csg(&corrupt)
                .unwrap_err()
                .contains("CSG cofactors differ")
        );
    }

    #[test]
    fn verifier_rejects_removed_repeat_deformation_and_material_event_records() {
        let (mut repeat_graph, repeat_config) = minimal_graph_and_config();
        let repeated_root = repeat_graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::FiniteRepeat {
                        child: repeat_graph.field_root,
                        axis: Axis::X,
                        first: 0,
                        count: 2,
                        period: ScalarId(1),
                    },
                    scalar_value: repeat_graph
                        .fields
                        .get(repeat_graph.field_root)
                        .unwrap()
                        .scalar_value,
                },
                NodeOrigin::synthetic("repeat"),
            )
            .unwrap();
        repeat_graph.field_root = repeated_root;
        let mut repeat_program =
            super::super::compile_structural_renderer(&repeat_graph, &repeat_config)
                .unwrap()
                .program()
                .clone();
        assert_eq!(repeat_program.repeats.len(), 1);
        assert!(
            repeat_program.repeats[0].wrap_events.is_empty(),
            "static camera and zero-rate parameters do not create wrap events"
        );
        let mut moving_config = repeat_config.clone();
        moving_config.camera_max_motion = 0.25;
        let moving_program =
            super::super::compile_structural_renderer(&repeat_graph, &moving_config).unwrap();
        assert!(
            !moving_program.program().repeats[0].wrap_events.is_empty(),
            "camera motion across adjacent relevant cells creates wrap obligations"
        );
        let mut corrupt_repeat = moving_program.program().clone();
        corrupt_repeat.repeats[0].instances[0].translations[0]
            .translation
            .hi = super::super::reference::interval::next_up(
            corrupt_repeat.repeats[0].instances[0].translations[0]
                .translation
                .hi,
        );
        assert!(
            verify_repeats(&repeat_graph, &moving_config, &corrupt_repeat)
                .unwrap_err()
                .contains("translations or wrap events")
        );
        repeat_program.repeats.clear();
        assert!(
            verify_capacities(&repeat_graph, &repeat_program)
                .unwrap_err()
                .contains("has no exact repeat template instance")
        );

        let (mut deform_graph, deform_config) = minimal_graph_and_config();
        deform_graph.params[1].range_min = -0.25;
        deform_graph.params[1].range_max = 0.25;
        deform_graph.params[1].rate = Some((0.125, 0.0));
        let base_scalar = deform_graph
            .fields
            .get(deform_graph.field_root)
            .unwrap()
            .scalar_value;
        let mut scalar = |op, dependency, label| {
            deform_graph
                .scalar
                .push(ScalarNode { op, dependency }, NodeOrigin::synthetic(label))
                .unwrap()
        };
        let coordinate_x = scalar(ScalarOp::CoordX, Dependency::Coordinate, "deform-x");
        let moving_phase = scalar(
            ScalarOp::Param(ParamId(1)),
            Dependency::Parameter,
            "deform-moving-phase",
        );
        let frequency_x = scalar(
            ScalarOp::Mul(ScalarId(1), coordinate_x),
            Dependency::Coordinate,
            "deform-frequency-x",
        );
        let angle = scalar(
            ScalarOp::Add(frequency_x, moving_phase),
            Dependency::CoordinateAndParameter,
            "deform-angle",
        );
        let wave = scalar(
            ScalarOp::SinRestricted(
                angle,
                super::super::scalar::SemanticOpId::SinRestrictedF32V1,
            ),
            Dependency::CoordinateAndParameter,
            "deform-wave",
        );
        let displacement = scalar(
            ScalarOp::Mul(ScalarId(1), wave),
            Dependency::CoordinateAndParameter,
            "deform-displacement",
        );
        let amplitude_bound = scalar(
            ScalarOp::ConstF32(super::super::scalar::SOURCE_TRIG_VALUE_FACTOR_V2.to_bits()),
            Dependency::Constant,
            "deform-amplitude-bound",
        );
        let gradient_bound = scalar(
            ScalarOp::ConstF32(super::super::scalar::SOURCE_TRIG_GRADIENT_FACTOR_V2.to_bits()),
            Dependency::Constant,
            "deform-gradient-bound",
        );
        let hessian_bound = scalar(
            ScalarOp::ConstF32(super::super::scalar::SOURCE_TRIG_HESSIAN_FACTOR_V2.to_bits()),
            Dependency::Constant,
            "deform-hessian-bound",
        );
        let third_derivative_bound = scalar(
            ScalarOp::ConstF32(super::super::scalar::SOURCE_TRIG_THIRD_FACTOR_V2.to_bits()),
            Dependency::Constant,
            "deform-third-bound",
        );
        let scalar_value = scalar(
            ScalarOp::Add(base_scalar, displacement),
            Dependency::CoordinateAndParameter,
            "deformed-field",
        );
        let deformed_root = deform_graph
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::BoundedDisplace {
                        base: deform_graph.field_root,
                        displacement,
                        contract: DerivedDeformContract {
                            amplitude_bound,
                            gradient_bound,
                            hessian_bound,
                            third_derivative_bound,
                            coordinate_x,
                            frequency: ScalarId(1),
                            phase: moving_phase,
                            derivation: ClosedDeformDerivation::SinusoidalX,
                        },
                    },
                    scalar_value,
                },
                NodeOrigin::synthetic("deformation"),
            )
            .unwrap();
        deform_graph.field_root = deformed_root;
        let mut deform_program =
            super::super::compile_structural_renderer(&deform_graph, &deform_config)
                .unwrap()
                .program()
                .clone();
        assert_eq!(deform_program.deformations.len(), 1);
        let verified_deform =
            super::super::compile_structural_renderer(&deform_graph, &deform_config).unwrap();
        let projective_deform = super::super::derive_projective_program(
            &deform_graph,
            &deform_config,
            &verified_deform,
        )
        .unwrap();
        assert!(
            projective_deform.equations.features[0].max_root_count > 1,
            "deformation oscillations must enlarge the planar root bound"
        );
        let deformation_event = projective_deform
            .events
            .generators
            .iter()
            .find(|event| {
                matches!(
                    event.representation,
                    super::super::events::EventRepresentation::DeformationTaylorPredicate { .. }
                )
            })
            .expect("deformation fixture has a numeric silhouette");
        assert_eq!(
            deformation_event.maximum_root_count,
            u32::from(projective_deform.equations.features[0].max_root_count)
        );
        let super::super::events::EventRepresentation::DeformationTaylorPredicate {
            scalar_derivatives,
            phase_recurrence,
            ..
        } = &deformation_event.representation
        else {
            unreachable!()
        };
        assert!(scalar_derivatives.sources.contains(&displacement));
        assert!(scalar_derivatives.sources.contains(&coordinate_x));
        assert_eq!(phase_recurrence.coordinate_x, coordinate_x);
        assert_eq!(phase_recurrence.phase_scalar, moving_phase);
        assert!(
            deformation_event
                .coefficient_dependencies
                .contains(&ParamId(1)),
            "a moving deformation phase must invalidate the numeric event"
        );
        let mut corrupt_deform = deform_program.clone();
        corrupt_deform.deformations[0].amplitude =
            super::super::reference::interval::next_up(corrupt_deform.deformations[0].amplitude);
        assert!(
            verify_deformations(&deform_graph, &corrupt_deform)
                .unwrap_err()
                .contains("numeric contract")
        );
        let mut corrupt_contract_graph = deform_graph.clone();
        // This fixture's frequency is 1 and every `SOURCE_TRIG_*_FACTOR_V2` is
        // the same multiplier, so `amplitude * frequency^k` is one value for
        // all four bounds: aliasing one contract scalar onto another is no
        // longer a value-level corruption for it to catch. Declare a bound
        // that genuinely disagrees with the closed derivation instead.
        let wrong_bound = corrupt_contract_graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(2.0_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("corrupt-gradient-bound"),
            )
            .unwrap();
        let FieldKind::BoundedDisplace { contract, .. } = &mut corrupt_contract_graph
            .fields
            .get_mut(deformed_root)
            .unwrap()
            .kind
        else {
            panic!("test deformation root changed kind")
        };
        contract.gradient_bound = wrong_bound;
        assert!(
            super::super::compile_structural_renderer(&corrupt_contract_graph, &deform_config)
                .unwrap_err()
                .contains("contract scalar")
        );
        deform_program.deformations.clear();
        assert_eq!(
            verify_deformations(&deform_graph, &deform_program).unwrap_err(),
            format!("pixels::verify: deformation field {deformed_root} has no derived template")
        );

        let (mut material_graph, material_config) = minimal_graph_and_config();
        material_graph.params[1].range_min = -0.5;
        material_graph.params[1].range_max = 0.5;
        material_graph.params[1].rate = Some((0.125, 0.0));
        let threshold = material_graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Param(ParamId(1)),
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("material-moving-threshold"),
            )
            .unwrap();
        let coordinate = material_graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::CoordX,
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("material-coordinate"),
            )
            .unwrap();
        let predicate = material_graph
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Compare {
                        op: CompareOp::Lt,
                        a: coordinate,
                        b: threshold,
                    },
                    dependency: Dependency::CoordinateAndParameter,
                },
                NodeOrigin::synthetic("material-predicate"),
            )
            .unwrap();
        let sample = material_graph
            .materials
            .get(material_graph.material_root)
            .unwrap()
            .clone();
        let alternate = material_graph
            .materials
            .push(sample, NodeOrigin::synthetic("alternate-material"))
            .unwrap();
        let selected = material_graph
            .materials
            .push(
                MaterialNode {
                    kind: MaterialKind::Select {
                        predicate,
                        a: material_graph.material_root,
                        b: alternate,
                    },
                },
                NodeOrigin::synthetic("material-select"),
            )
            .unwrap();
        material_graph.material_root = selected;
        let mut material_program =
            super::super::compile_structural_renderer(&material_graph, &material_config)
                .unwrap()
                .program()
                .clone();
        assert_eq!(material_program.material_events.len(), 1);
        let verified_material =
            super::super::compile_structural_renderer(&material_graph, &material_config).unwrap();
        let projective_material = super::super::derive_projective_program(
            &material_graph,
            &material_config,
            &verified_material,
        )
        .unwrap();
        let material_generator = projective_material
            .events
            .generators
            .iter()
            .find(|event| event.kind == super::super::event_kinds::EventKind::MaterialBoundary)
            .expect("material fixture has a boundary generator");
        assert_eq!(
            u32::from(material_generator.maximum_root_count),
            material_program.material_events[0].crossing_bound
        );
        assert!(
            material_generator
                .coefficient_dependencies
                .contains(&ParamId(1)),
            "a parameter used only by the material threshold must invalidate the event"
        );
        assert!(matches!(
            material_generator.representation,
            super::super::events::EventRepresentation::MaterialDifferenceTaylorPredicate {
                left: candidate_left,
                right: candidate_right,
                ..
            } if candidate_left == coordinate && candidate_right == threshold
        ));
        let mut corrupt_material = material_program.clone();
        corrupt_material.material_events[0].crossing_bound = corrupt_material.material_events[0]
            .crossing_bound
            .saturating_add(1);
        assert!(
            verify_material_events(&material_graph, &corrupt_material)
                .unwrap_err()
                .contains("owner, crossing, kind, or origin")
        );
        material_program.material_events.clear();
        assert!(
            verify_exact_derivation(&material_graph, &material_config, &material_program)
                .unwrap_err()
                .contains("material events")
        );
    }

    #[test]
    fn verifier_reports_lowest_stable_missing_object_reference() {
        let mut program = empty_program();
        program.csg = CsgProgram {
            constant: None,
            instructions: vec![CsgInst::Push(ObjectId(0))],
            max_stack: 1,
            influence: Vec::new(),
        };
        assert_eq!(
            verify_csg(&program).unwrap_err(),
            "pixels::verify: CSG references missing object o0"
        );
    }

    #[test]
    fn verifier_rejects_corrupt_report_and_capacity_derivations() {
        let mut program = empty_program();
        program.report.object_count = 1;
        assert_eq!(
            verify_report(&program).unwrap_err(),
            "pixels::verify: structural report differs from verified inputs"
        );

        program.report.object_count = 0;
        program.capacities.max_run_records_per_tile_row =
            super::super::projection_bounds::TILE_WIDTH_V1;
        program.capacities.max_local_rebuild_queue = super::super::projection_bounds::TILE_WIDTH_V1;
        program.capacities.max_transparent_layers = 0;
        let terminal_records = u64::from(super::super::projection_bounds::TILE_WIDTH_V1);
        program.capacities.run_bytes =
            terminal_records * super::super::capacities::RUN_RECORD_BYTES_V1;
        program.capacities.corridor_bytes =
            terminal_records * super::super::capacities::CORRIDOR_RECORD_BYTES_V1;
        program.capacities.fixed_q_bytes =
            terminal_records * super::super::capacities::FIXED_Q_RECORD_BYTES_V1;
        program.capacities.shading_bytes =
            terminal_records * super::super::capacities::SHADING_RECORD_BYTES_V1;
        program.capacities.per_worker_scratch_bytes = program.capacities.run_bytes
            + program.capacities.corridor_bytes
            + program.capacities.fixed_q_bytes
            + program.capacities.shading_bytes;
        program.capacities.all_worker_scratch_bytes = program.capacities.per_worker_scratch_bytes;
        program.capacities.state_header_bytes =
            super::super::capacities::RENDERER_STATE_HEADER_BYTES_V1;
        program.capacities.frame_dependency_snapshot_bytes =
            super::super::capacities::P7_CANONICAL_FRAME_SNAPSHOT_BYTES * 2;
        program.capacities.failure_record_bytes = super::super::capacities::FAILURE_RECORD_BYTES_V1;
        let pre_framebuffer = program.capacities.state_header_bytes
            + program.capacities.frame_dependency_snapshot_bytes
            + program.capacities.all_worker_scratch_bytes;
        let page = wrela_machine::layout::PIXELS_STATE_PAGE_ALIGNMENT;
        program.capacities.total_renderer_state_bytes =
            ((pre_framebuffer + page - 1) & !(page - 1)) + program.capacities.failure_record_bytes;
        program.capacities.total_renderer_state_bytes_instrumented =
            program.capacities.total_renderer_state_bytes;
        let graph = empty_graph();
        assert!(verify_capacities(&graph, &program).is_ok());

        program.capacities.candidate_bytes = 1;
        assert_eq!(
            verify_capacities(&graph, &program).unwrap_err(),
            "pixels::verify: candidate storage bytes 1 differ from derived 0"
        );
    }

    #[test]
    fn the_exact_derivation_switch_fails_closed_on_anything_but_one() {
        use std::ffi::OsStr;
        assert!(exact_verification_requested(Some(OsStr::new("1"))));
        for declined in ["", "0", "true", "yes", "1 ", "01"] {
            assert!(
                !exact_verification_requested(Some(OsStr::new(declined))),
                "`{declined}` must not enable the exact re-derivation ratchet"
            );
        }
        assert!(!exact_verification_requested(None));
        // The deep lane sets this exact name; keeping it a shared constant is
        // what stops a rename from silently disabling the ratchet.
        assert_eq!(EXACT_VERIFY_ENV, "WRELA_PIXELS_EXACT_VERIFY");
    }
}
