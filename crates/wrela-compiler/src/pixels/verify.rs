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
    if prior_end != program.params.packed_bytes {
        return Err(format!(
            "pixels::verify: packed byte count {} does not equal final slot end {prior_end}",
            program.params.packed_bytes
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
        if let Some(texture) = &sample.pattern {
            let exact = super::material_graph::compiler_texture(texture.stable_id, texture.filter)?;
            if *texture != exact {
                return Err(format!(
                    "pixels::verify: material {material} texture descriptor differs from compiler-owned asset `{}`",
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
    if u64::from(capacities.max_event_records) != expected_events
        || u64::from(capacities.max_run_records_per_tile_row)
            != expected_events
                .checked_add(1)
                .ok_or_else(|| "pixels::verify: run record arithmetic overflow".to_string())?
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
    let expected_state = checked_sum(
        &[
            capacities.state_header_bytes,
            capacities.coefficient_snapshot_bytes,
            capacities.frame_dependency_snapshot_bytes,
            capacities.frame_complex_double_buffer_bytes,
            capacities.all_worker_scratch_bytes,
            capacities.output_double_buffer_bytes,
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
                u64::from(program.params.frame_dependencies.runtime_bytes),
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
    verify_exact_derivation(graph, config, &program)?;
    Ok(VerifiedStructuralProgram(program))
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
            camera_max_motion: 0.0,
            light_capacity: 0,
            light_kinds: Vec::new(),
            exposure: ScalarRangeConfig { min: 0.0, max: 0.0 },
            environment: RgbRangeConfig {
                min: [0.0; 3],
                max: [0.0; 3],
            },
            ao_enabled: false,
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
        let frequency_x = scalar(
            ScalarOp::Mul(ScalarId(1), coordinate_x),
            Dependency::Coordinate,
            "deform-frequency-x",
        );
        let angle = scalar(
            ScalarOp::Add(frequency_x, ScalarId(0)),
            Dependency::Coordinate,
            "deform-angle",
        );
        let wave = scalar(
            ScalarOp::SinRestricted(
                angle,
                super::super::scalar::SemanticOpId::SinRestrictedF32V1,
            ),
            Dependency::Coordinate,
            "deform-wave",
        );
        let displacement = scalar(
            ScalarOp::Mul(ScalarId(1), wave),
            Dependency::Coordinate,
            "deform-displacement",
        );
        let amplitude_bound = scalar(
            ScalarOp::ConstF32(super::super::scalar::SOURCE_TRIG_VALUE_FACTOR_V1.to_bits()),
            Dependency::Constant,
            "deform-amplitude-bound",
        );
        let gradient_bound = scalar(
            ScalarOp::ConstF32(super::super::scalar::SOURCE_TRIG_GRADIENT_FACTOR_V1.to_bits()),
            Dependency::Constant,
            "deform-gradient-bound",
        );
        let hessian_bound = scalar(
            ScalarOp::ConstF32(super::super::scalar::SOURCE_TRIG_HESSIAN_FACTOR_V1.to_bits()),
            Dependency::Constant,
            "deform-hessian-bound",
        );
        let third_derivative_bound = scalar(
            ScalarOp::ConstF32(super::super::scalar::SOURCE_TRIG_THIRD_FACTOR_V1.to_bits()),
            Dependency::Constant,
            "deform-third-bound",
        );
        let scalar_value = scalar(
            ScalarOp::Add(base_scalar, displacement),
            Dependency::Coordinate,
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
                            phase: ScalarId(0),
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
        let mut corrupt_deform = deform_program.clone();
        corrupt_deform.deformations[0].amplitude =
            super::super::reference::interval::next_up(corrupt_deform.deformations[0].amplitude);
        assert!(
            verify_deformations(&deform_graph, &corrupt_deform)
                .unwrap_err()
                .contains("numeric contract")
        );
        let mut corrupt_contract_graph = deform_graph.clone();
        let FieldKind::BoundedDisplace { contract, .. } = &mut corrupt_contract_graph
            .fields
            .get_mut(deformed_root)
            .unwrap()
            .kind
        else {
            panic!("test deformation root changed kind")
        };
        contract.gradient_bound = contract.amplitude_bound;
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
                        b: ScalarId(0),
                    },
                    dependency: Dependency::Coordinate,
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
        program.capacities.max_run_records_per_tile_row = 1;
        program.capacities.max_local_rebuild_queue = 1;
        program.capacities.max_transparent_layers = 0;
        program.capacities.run_bytes = super::super::capacities::RUN_RECORD_BYTES_V1;
        program.capacities.corridor_bytes = super::super::capacities::CORRIDOR_RECORD_BYTES_V1;
        program.capacities.fixed_q_bytes = super::super::capacities::FIXED_Q_RECORD_BYTES_V1;
        program.capacities.shading_bytes = super::super::capacities::SHADING_RECORD_BYTES_V1;
        program.capacities.per_worker_scratch_bytes = program.capacities.run_bytes
            + program.capacities.corridor_bytes
            + program.capacities.fixed_q_bytes
            + program.capacities.shading_bytes;
        program.capacities.all_worker_scratch_bytes = program.capacities.per_worker_scratch_bytes;
        program.capacities.state_header_bytes =
            super::super::capacities::RENDERER_STATE_HEADER_BYTES_V1;
        program.capacities.failure_record_bytes = super::super::capacities::FAILURE_RECORD_BYTES_V1;
        program.capacities.total_renderer_state_bytes = program.capacities.state_header_bytes
            + program.capacities.all_worker_scratch_bytes
            + program.capacities.failure_record_bytes;
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
}
