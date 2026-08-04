//! Maximal smooth-object partitioning and stable source identity sets.

use std::collections::{BTreeMap, BTreeSet};

use super::bounds::ValueBounds;
use super::graph::{Axis, CanonicalIdentity, FieldKind};
use super::ids::{FieldId, ObjectId, ScalarId};
use super::reference::interval::F64Interval;
use super::support::OccurrenceStep;
use super::support::SupportTable;
use super::symbolic::SymbolicGraph;
use super::world_bounds::{Aabb64, WorldBounds};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdentityPair {
    pub object: CanonicalIdentity,
    pub material: CanonicalIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentitySet {
    pub id: u32,
    pub pairs: Vec<IdentityPair>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepeatInstance {
    pub repeat_field: FieldId,
    pub equivalent_fields: Vec<FieldId>,
    pub axis: Axis,
    pub first: i32,
    pub index: i32,
    pub period: F64Interval,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SmoothObject {
    pub id: ObjectId,
    pub source_root: FieldId,
    pub scalar_root: ScalarId,
    pub bounds: Aabb64,
    pub primitive_occurrences: Vec<Vec<OccurrenceStep>>,
    pub support_max: F64Interval,
    pub identity_set: u32,
    pub repeat_instances: Vec<RepeatInstance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CsgExpr {
    Const(bool),
    Leaf(ObjectId),
    Not(Box<CsgExpr>),
    And(Box<CsgExpr>, Box<CsgExpr>),
    Or(Box<CsgExpr>, Box<CsgExpr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectPartition {
    pub objects: Vec<SmoothObject>,
    pub identities: Vec<IdentitySet>,
    pub csg: CsgExpr,
}

#[derive(Clone)]
struct Draft {
    key: usize,
    structural_root_key: (FieldKind, ScalarId),
    source_origin: super::arena::OriginSite,
    source_root: FieldId,
    scalar_root: ScalarId,
    bounds: Aabb64,
    primitive_occurrences: Vec<Vec<OccurrenceStep>>,
    support_max: F64Interval,
    identities: Vec<IdentityPair>,
    repeat_instances: Vec<RepeatInstance>,
}

#[derive(Clone, PartialEq, Eq)]
enum DraftExpr {
    Const(bool),
    Leaf(usize),
    Not(Box<DraftExpr>),
    And(Box<DraftExpr>, Box<DraftExpr>),
    Or(Box<DraftExpr>, Box<DraftExpr>),
}

fn are_complements(a: &DraftExpr, b: &DraftExpr) -> bool {
    matches!(a, DraftExpr::Not(child) if child.as_ref() == b)
        || matches!(b, DraftExpr::Not(child) if child.as_ref() == a)
}

fn simplify_draft(expr: DraftExpr) -> DraftExpr {
    match expr {
        DraftExpr::Not(child) => match simplify_draft(*child) {
            DraftExpr::Const(value) => DraftExpr::Const(!value),
            DraftExpr::Not(grandchild) => *grandchild,
            child => DraftExpr::Not(Box::new(child)),
        },
        DraftExpr::And(a, b) => {
            let a = simplify_draft(*a);
            let b = simplify_draft(*b);
            if matches!(a, DraftExpr::Const(false))
                || matches!(b, DraftExpr::Const(false))
                || are_complements(&a, &b)
            {
                DraftExpr::Const(false)
            } else if matches!(a, DraftExpr::Const(true)) {
                b
            } else if matches!(b, DraftExpr::Const(true)) || a == b {
                a
            } else {
                DraftExpr::And(Box::new(a), Box::new(b))
            }
        }
        DraftExpr::Or(a, b) => {
            let a = simplify_draft(*a);
            let b = simplify_draft(*b);
            if matches!(a, DraftExpr::Const(true))
                || matches!(b, DraftExpr::Const(true))
                || are_complements(&a, &b)
            {
                DraftExpr::Const(true)
            } else if matches!(a, DraftExpr::Const(false)) {
                b
            } else if matches!(b, DraftExpr::Const(false)) || a == b {
                a
            } else {
                DraftExpr::Or(Box::new(a), Box::new(b))
            }
        }
        other => other,
    }
}

fn collect_live_drafts(expr: &DraftExpr, live: &mut BTreeSet<usize>) {
    match expr {
        DraftExpr::Const(_) => {}
        DraftExpr::Leaf(key) => {
            live.insert(*key);
        }
        DraftExpr::Not(child) => collect_live_drafts(child, live),
        DraftExpr::And(a, b) | DraftExpr::Or(a, b) => {
            collect_live_drafts(a, live);
            collect_live_drafts(b, live);
        }
    }
}

struct Partitioner<'a> {
    graph: &'a SymbolicGraph,
    values: &'a ValueBounds,
    bounds: &'a WorldBounds,
    support: &'a SupportTable,
    drafts: Vec<Draft>,
}

impl<'a> Partitioner<'a> {
    fn partition(&mut self, id: FieldId, inherited: &[IdentityPair]) -> Result<DraftExpr, String> {
        let node = self.graph.fields.get(id)?;
        if self.bounds.get(id)?.bounds.is_none() {
            return Ok(DraftExpr::Const(false));
        }
        match &node.kind {
            FieldKind::Mark {
                child,
                object_source,
                material_source,
            } => {
                let mut identities = inherited.to_vec();
                identities.push(IdentityPair {
                    object: object_source.clone(),
                    material: material_source.clone(),
                });
                identities.sort();
                identities.dedup();
                self.partition(*child, &identities)
            }
            FieldKind::HardUnion { a, b } => {
                let left = simplify_draft(self.partition(*a, inherited)?);
                if left == DraftExpr::Const(true) {
                    Ok(left)
                } else {
                    Ok(simplify_draft(DraftExpr::Or(
                        Box::new(left),
                        Box::new(self.partition(*b, inherited)?),
                    )))
                }
            }
            FieldKind::HardIntersection { a, b } => {
                let left = simplify_draft(self.partition(*a, inherited)?);
                if left == DraftExpr::Const(false) {
                    Ok(left)
                } else {
                    Ok(simplify_draft(DraftExpr::And(
                        Box::new(left),
                        Box::new(self.partition(*b, inherited)?),
                    )))
                }
            }
            FieldKind::HardSubtract { a, b } => {
                let left = simplify_draft(self.partition(*a, inherited)?);
                if left == DraftExpr::Const(false) {
                    Ok(left)
                } else {
                    Ok(simplify_draft(DraftExpr::And(
                        Box::new(left),
                        Box::new(DraftExpr::Not(Box::new(self.partition(*b, inherited)?))),
                    )))
                }
            }
            FieldKind::Neg { child } => {
                Ok(DraftExpr::Not(Box::new(self.partition(*child, inherited)?)))
            }
            _ => self.make_objects(id, inherited),
        }
    }

    fn make_objects(
        &mut self,
        id: FieldId,
        inherited: &[IdentityPair],
    ) -> Result<DraftExpr, String> {
        ensure_smooth_subtree(self.graph, id)?;
        let Some(_aggregate_bounds) = self.bounds.get(id)?.bounds else {
            return Ok(DraftExpr::Const(false));
        };
        let scalar_root = self.graph.fields.get(id)?.scalar_value;
        let combinations = collect_repeat_combinations(self.graph, self.values, self.bounds, id)?;
        let mut occurrences = self
            .support
            .get(id)?
            .leaf_supports
            .iter()
            .map(|leaf| leaf.path.clone())
            .collect::<Vec<_>>();
        occurrences.sort();
        if occurrences.is_empty() {
            return Err(format!(
                "pixels::objects: smooth object rooted at {id} has no primitive leaf"
            ));
        }
        let mut identities = inherited.to_vec();
        collect_identities(self.graph, id, &mut identities, &mut BTreeSet::new())?;
        identities.sort();
        identities.dedup();

        let mut expression = DraftExpr::Const(false);
        for repeat_instances in combinations {
            let Some(bounds) = fixed_instance_bounds(
                self.graph,
                self.values,
                self.bounds,
                self.support,
                id,
                &repeat_instances,
            )?
            .and_then(|bound| bound.clip(self.bounds.world)) else {
                continue;
            };
            if let Some(existing) = self.drafts.iter().find(|draft| {
                draft.source_root == id
                    && draft.scalar_root == scalar_root
                    && draft.bounds == bounds
                    && draft.identities == identities
                    && draft.repeat_instances == repeat_instances
            }) {
                let leaf = DraftExpr::Leaf(existing.key);
                expression = match expression {
                    DraftExpr::Const(false) => leaf,
                    prior => DraftExpr::Or(Box::new(prior), Box::new(leaf)),
                };
                continue;
            }
            let key = self.drafts.len();
            self.drafts.push(Draft {
                key,
                structural_root_key: (self.graph.fields.get(id)?.kind.clone(), scalar_root),
                source_origin: self.graph.fields.origin(id)?.primary.clone(),
                source_root: id,
                scalar_root,
                bounds,
                primitive_occurrences: occurrences.clone(),
                support_max: self.support.get(id)?.max_budget,
                identities: identities.clone(),
                repeat_instances,
            });
            let leaf = DraftExpr::Leaf(key);
            expression = match expression {
                DraftExpr::Const(false) => leaf,
                prior => DraftExpr::Or(Box::new(prior), Box::new(leaf)),
            };
        }
        Ok(expression)
    }
}

fn ensure_smooth_subtree(graph: &SymbolicGraph, root: FieldId) -> Result<(), String> {
    let mut stack = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match &graph.fields.get(id)?.kind {
            FieldKind::HardUnion { .. }
            | FieldKind::HardIntersection { .. }
            | FieldKind::HardSubtract { .. }
            | FieldKind::Neg { .. } => {
                return Err(format!(
                    "pixels::objects: hard CSG node {id} escaped source-f32 wrapper normalization before object partitioning"
                ));
            }
            FieldKind::Primitive(_) => {}
            FieldKind::SmoothUnion { a, b, .. }
            | FieldKind::SmoothIntersection { a, b, .. }
            | FieldKind::SmoothSubtract { a, b, .. } => stack.extend([*a, *b]),
            FieldKind::Transform { child, .. }
            | FieldKind::FiniteRepeat { child, .. }
            | FieldKind::Mark { child, .. } => stack.push(*child),
            FieldKind::BoundedDisplace { base, .. } => stack.push(*base),
        }
    }
    Ok(())
}

fn fixed_instance_bounds(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    bounds: &WorldBounds,
    support: &SupportTable,
    id: FieldId,
    instances: &[RepeatInstance],
) -> Result<Option<Aabb64>, String> {
    let child = |child| fixed_instance_bounds(graph, values, bounds, support, child, instances);
    Ok(match &graph.fields.get(id)?.kind {
        FieldKind::Primitive(_) => bounds.get(id)?.bounds,
        FieldKind::HardUnion { .. }
        | FieldKind::HardIntersection { .. }
        | FieldKind::HardSubtract { .. }
        | FieldKind::Neg { .. } => {
            return Err(format!(
                "pixels::objects: hard CSG node {id} escaped partition-frontier validation"
            ));
        }
        FieldKind::SmoothUnion { a, b, k } => {
            let expansion =
                super::world_bounds::smooth_node_expansion(values, support, *a, *b, *k)?;
            match (child(*a)?, child(*b)?) {
                (Some(a), Some(b)) => Some(a.union(b).expand(expansion)?),
                (Some(bound), None) | (None, Some(bound)) => Some(bound.expand(expansion)?),
                (None, None) => None,
            }
        }
        FieldKind::SmoothIntersection { a, b, k } => {
            let expansion =
                super::world_bounds::smooth_node_expansion(values, support, *a, *b, *k)?;
            match (child(*a)?, child(*b)?) {
                (Some(a), Some(b)) => a
                    .intersect(b)
                    .map(|bound| bound.expand(expansion))
                    .transpose()?,
                _ => None,
            }
        }
        FieldKind::SmoothSubtract { a, b, k } => child(*a)?
            .map(|bound| {
                bound.expand(super::world_bounds::smooth_node_expansion(
                    values, support, *a, *b, *k,
                )?)
            })
            .transpose()?,
        FieldKind::Transform {
            child: source,
            transform,
        } => child(*source)?
            .map(|bound| {
                if bound == bounds.world {
                    Ok(bounds.world)
                } else {
                    super::world_bounds::transform_bounds(bound, transform, values)
                }
            })
            .transpose()?,
        FieldKind::FiniteRepeat {
            child: source,
            axis,
            ..
        } => {
            let instance = instances
                .iter()
                .find(|instance| {
                    instance.repeat_field == id || instance.equivalent_fields.contains(&id)
                })
                .ok_or_else(|| {
                    format!("pixels::objects: fixed object omits repeat selection for {id}")
                })?;
            let mut bound = child(*source)?.ok_or_else(|| {
                format!("pixels::objects: repeat {id} selected an empty child bound")
            })?;
            if bound == bounds.world {
                return Ok(Some(bounds.world));
            }
            let component = match axis {
                Axis::X => 0,
                Axis::Y => 1,
                Axis::Z => 2,
            };
            let shift = super::world_bounds::repeat_translation_interval(
                instance.first,
                instance.index,
                instance.period,
            )?;
            bound.min[component] =
                super::reference::interval::next_down(bound.min[component] + shift.lo);
            bound.max[component] =
                super::reference::interval::next_up(bound.max[component] + shift.hi);
            Some(bound)
        }
        FieldKind::BoundedDisplace { base, contract, .. } => child(*base)?
            .map(|bound| {
                if bound == bounds.world {
                    Ok(bounds.world)
                } else {
                    bound.expand(super::reference::interval::next_up(
                        values.get(contract.amplitude_bound)?.abs_upper()
                            * support.get(*base)?.max_value_to_distance.hi,
                    ))
                }
            })
            .transpose()?,
        FieldKind::Mark { child: source, .. } => child(*source)?,
    })
}

fn collect_identities(
    graph: &SymbolicGraph,
    id: FieldId,
    identities: &mut Vec<IdentityPair>,
    seen: &mut BTreeSet<FieldId>,
) -> Result<(), String> {
    if !seen.insert(id) {
        return Ok(());
    }
    match &graph.fields.get(id)?.kind {
        FieldKind::Mark {
            child,
            object_source,
            material_source,
        } => {
            identities.push(IdentityPair {
                object: object_source.clone(),
                material: material_source.clone(),
            });
            collect_identities(graph, *child, identities, seen)?;
        }
        FieldKind::Primitive(_) => {}
        FieldKind::HardUnion { a, b }
        | FieldKind::HardIntersection { a, b }
        | FieldKind::HardSubtract { a, b }
        | FieldKind::SmoothUnion { a, b, .. }
        | FieldKind::SmoothIntersection { a, b, .. }
        | FieldKind::SmoothSubtract { a, b, .. } => {
            collect_identities(graph, *a, identities, seen)?;
            collect_identities(graph, *b, identities, seen)?;
        }
        FieldKind::Neg { child }
        | FieldKind::Transform { child, .. }
        | FieldKind::FiniteRepeat { child, .. } => {
            collect_identities(graph, *child, identities, seen)?;
        }
        FieldKind::BoundedDisplace { base, .. } => {
            collect_identities(graph, *base, identities, seen)?;
        }
    }
    Ok(())
}

fn collect_repeat_combinations(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    bounds: &WorldBounds,
    id: FieldId,
) -> Result<Vec<Vec<RepeatInstance>>, String> {
    fn visit(
        graph: &SymbolicGraph,
        values: &ValueBounds,
        bounds: &WorldBounds,
        repeats_below_transform: &BTreeSet<FieldId>,
        id: FieldId,
        families: &mut BTreeMap<FieldId, Vec<RepeatInstance>>,
        seen: &mut BTreeSet<FieldId>,
    ) -> Result<(), String> {
        if !seen.insert(id) {
            return Ok(());
        }
        match &graph.fields.get(id)?.kind {
            FieldKind::FiniteRepeat {
                child,
                axis,
                first,
                count,
                period,
            } => {
                let period = values.get(*period)?;
                if period.lo <= 0.0 {
                    return Err(format!(
                        "P012: repetition has no finite relevant-instance bound in the renderer world box: period={period:?}"
                    ));
                }
                let Some(base_bounds) = bounds.get(*child)?.bounds else {
                    return Ok(());
                };
                let indices =
                    if base_bounds == bounds.world || repeats_below_transform.contains(&id) {
                        super::world_bounds::authored_repeat_indices(*first, *count)?
                    } else {
                        super::world_bounds::relevant_repeat_indices(
                            base_bounds,
                            bounds.world,
                            *axis,
                            *first,
                            *count,
                            period,
                        )?
                    };
                let family = indices
                    .into_iter()
                    .map(|index| RepeatInstance {
                        repeat_field: id,
                        equivalent_fields: vec![id],
                        axis: *axis,
                        first: *first,
                        index,
                        period,
                    })
                    .collect::<Vec<_>>();
                match families.entry(id) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(family);
                    }
                    std::collections::btree_map::Entry::Occupied(entry)
                        if entry.get() != &family =>
                    {
                        return Err(format!(
                            "pixels::objects: repeat field {id} produced inconsistent instance families"
                        ));
                    }
                    std::collections::btree_map::Entry::Occupied(_) => {}
                }
                visit(
                    graph,
                    values,
                    bounds,
                    repeats_below_transform,
                    *child,
                    families,
                    seen,
                )?;
            }
            FieldKind::Primitive(_) => {}
            FieldKind::HardUnion { a, b }
            | FieldKind::HardIntersection { a, b }
            | FieldKind::HardSubtract { a, b }
            | FieldKind::SmoothUnion { a, b, .. }
            | FieldKind::SmoothIntersection { a, b, .. }
            | FieldKind::SmoothSubtract { a, b, .. } => {
                visit(
                    graph,
                    values,
                    bounds,
                    repeats_below_transform,
                    *a,
                    families,
                    seen,
                )?;
                visit(
                    graph,
                    values,
                    bounds,
                    repeats_below_transform,
                    *b,
                    families,
                    seen,
                )?;
            }
            FieldKind::Neg { child }
            | FieldKind::Transform { child, .. }
            | FieldKind::Mark { child, .. } => visit(
                graph,
                values,
                bounds,
                repeats_below_transform,
                *child,
                families,
                seen,
            )?,
            FieldKind::BoundedDisplace { base, .. } => visit(
                graph,
                values,
                bounds,
                repeats_below_transform,
                *base,
                families,
                seen,
            )?,
        }
        Ok(())
    }
    let mut families = BTreeMap::new();
    let repeats_below_transform = super::world_bounds::repeats_below_transform(graph)?;
    visit(
        graph,
        values,
        bounds,
        &repeats_below_transform,
        id,
        &mut families,
        &mut BTreeSet::new(),
    )?;
    fn equivalent_depth(
        graph: &SymbolicGraph,
        values: &ValueBounds,
        id: FieldId,
        signature: (Axis, ScalarId, i32, u64, u64),
        memo: &mut BTreeMap<FieldId, u32>,
    ) -> Result<u32, String> {
        if let Some(depth) = memo.get(&id) {
            return Ok(*depth);
        }
        let depth = match &graph.fields.get(id)?.kind {
            FieldKind::Primitive(_) => 0,
            FieldKind::HardUnion { a, b }
            | FieldKind::HardIntersection { a, b }
            | FieldKind::HardSubtract { a, b }
            | FieldKind::SmoothUnion { a, b, .. }
            | FieldKind::SmoothIntersection { a, b, .. }
            | FieldKind::SmoothSubtract { a, b, .. } => {
                equivalent_depth(graph, values, *a, signature, memo)?
                    .max(equivalent_depth(graph, values, *b, signature, memo)?)
            }
            FieldKind::FiniteRepeat {
                child,
                axis,
                first,
                period,
                ..
            } => {
                let period_id = *period;
                let period = values.get(period_id)?;
                let matches = (
                    *axis,
                    period_id,
                    *first,
                    period.lo.to_bits(),
                    period.hi.to_bits(),
                ) == signature;
                equivalent_depth(graph, values, *child, signature, memo)?
                    .checked_add(u32::from(matches))
                    .ok_or_else(|| "pixels::objects: repeat nesting depth overflow".to_string())?
            }
            FieldKind::Neg { child }
            | FieldKind::Transform { child, .. }
            | FieldKind::Mark { child, .. } => {
                equivalent_depth(graph, values, *child, signature, memo)?
            }
            FieldKind::BoundedDisplace { base, .. } => {
                equivalent_depth(graph, values, *base, signature, memo)?
            }
        };
        memo.insert(id, depth);
        Ok(depth)
    }
    let mut canonical_families = BTreeMap::<
        (
            super::arena::OriginSite,
            Vec<super::arena::OriginSite>,
            Axis,
            ScalarId,
            i32,
            u64,
            u64,
            Vec<i32>,
            Vec<(u64, u64)>,
            u32,
        ),
        (FieldId, Vec<RepeatInstance>),
    >::new();
    for (field, family) in families {
        let Some(first) = family.first() else {
            continue;
        };
        let (period_id, authored_first, child) = match &graph.fields.get(field)?.kind {
            FieldKind::FiniteRepeat {
                child,
                first,
                period,
                ..
            } => (*period, *first, *child),
            _ => {
                return Err(format!(
                    "pixels::objects: repeat family key {field} does not name a finite repeat"
                ));
            }
        };
        let signature = (
            first.axis,
            period_id,
            authored_first,
            first.period.lo.to_bits(),
            first.period.hi.to_bits(),
        );
        let depth = equivalent_depth(graph, values, child, signature, &mut BTreeMap::new())?;
        let origin = graph.fields.origin(field)?;
        let key = (
            origin.primary.clone(),
            origin.expansion_chain.clone(),
            signature.0,
            signature.1,
            signature.2,
            signature.3,
            signature.4,
            family.iter().map(|instance| instance.index).collect(),
            family
                .iter()
                .map(|instance| {
                    super::world_bounds::repeat_translation_interval(
                        instance.first,
                        instance.index,
                        instance.period,
                    )
                    .map(|translation| (translation.lo.to_bits(), translation.hi.to_bits()))
                })
                .collect::<Result<Vec<_>, _>>()?,
            depth,
        );
        canonical_families
            .entry(key)
            .and_modify(|(canonical, instances)| {
                for (instance, incoming) in instances.iter_mut().zip(&family) {
                    instance
                        .equivalent_fields
                        .extend(incoming.equivalent_fields.iter().copied());
                    instance.equivalent_fields.sort();
                    instance.equivalent_fields.dedup();
                }
                if field < *canonical {
                    *canonical = field;
                    for instance in instances {
                        instance.repeat_field = field;
                    }
                }
            })
            .or_insert_with(|| (field, family));
    }
    let mut combinations = repeat_combinations(
        &canonical_families
            .into_values()
            .map(|(_, family)| family)
            .collect::<Vec<_>>(),
    )?;
    if combinations.is_empty() {
        return Ok(Vec::new());
    }
    combinations.sort_by(|a, b| {
        a.iter()
            .map(|instance| {
                (
                    instance.repeat_field,
                    instance.axis,
                    instance.index,
                    instance.period.lo.to_bits(),
                    instance.period.hi.to_bits(),
                )
            })
            .collect::<Vec<_>>()
            .cmp(
                &b.iter()
                    .map(|instance| {
                        (
                            instance.repeat_field,
                            instance.axis,
                            instance.index,
                            instance.period.lo.to_bits(),
                            instance.period.hi.to_bits(),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
    });
    combinations.dedup_by(|a, b| {
        a.iter()
            .map(|instance| {
                (
                    instance.repeat_field,
                    instance.axis,
                    instance.index,
                    instance.period.lo.to_bits(),
                    instance.period.hi.to_bits(),
                )
            })
            .eq(b.iter().map(|instance| {
                (
                    instance.repeat_field,
                    instance.axis,
                    instance.index,
                    instance.period.lo.to_bits(),
                    instance.period.hi.to_bits(),
                )
            }))
    });
    Ok(combinations)
}

fn repeat_combinations(
    repeat_families: &[Vec<RepeatInstance>],
) -> Result<Vec<Vec<RepeatInstance>>, String> {
    let mut combinations = vec![Vec::new()];
    for family in repeat_families {
        let capacity = combinations
            .len()
            .checked_mul(family.len())
            .ok_or_else(|| "P015: repeated instance product overflow".to_string())?;
        let ceiling =
            super::capacities::PixelsCeilings::MACHINE_V1.repeat_analysis_candidates as usize;
        if capacity > ceiling {
            return Err(format!(
                "P015: renderer capacity `repeated_instances` needs {capacity} slots, which exceeds the analysis safety ceiling of {ceiling}"
            ));
        }
        let mut expanded = Vec::with_capacity(capacity);
        for combination in &combinations {
            for instance in family {
                let mut next = combination.clone();
                next.push(instance.clone());
                expanded.push(next);
            }
        }
        combinations = expanded;
    }
    Ok(combinations)
}

fn remap_expr(expr: DraftExpr, ids: &BTreeMap<usize, ObjectId>) -> Result<CsgExpr, String> {
    Ok(match expr {
        DraftExpr::Const(value) => CsgExpr::Const(value),
        DraftExpr::Leaf(key) => CsgExpr::Leaf(*ids.get(&key).ok_or_else(|| {
            format!("pixels::objects: missing stable object ID for draft key {key}")
        })?),
        DraftExpr::Not(child) => CsgExpr::Not(Box::new(remap_expr(*child, ids)?)),
        DraftExpr::And(a, b) => CsgExpr::And(
            Box::new(remap_expr(*a, ids)?),
            Box::new(remap_expr(*b, ids)?),
        ),
        DraftExpr::Or(a, b) => CsgExpr::Or(
            Box::new(remap_expr(*a, ids)?),
            Box::new(remap_expr(*b, ids)?),
        ),
    })
}

pub fn partition(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    bounds: &WorldBounds,
    support: &SupportTable,
) -> Result<ObjectPartition, String> {
    let mut partitioner = Partitioner {
        graph,
        values,
        bounds,
        support,
        drafts: Vec::new(),
    };
    let expression = simplify_draft(partitioner.partition(graph.field_root, &[])?);
    let mut live = BTreeSet::new();
    collect_live_drafts(&expression, &mut live);
    partitioner.drafts.retain(|draft| live.contains(&draft.key));
    let object_ceiling = super::capacities::PixelsCeilings::MACHINE_V1.objects as usize;
    if partitioner.drafts.len() > object_ceiling {
        return Err(format!(
            "P015: renderer capacity `objects` exceeds the machine-v1 ceiling of {object_ceiling} after exact Boolean pruning ({} live objects)",
            partitioner.drafts.len()
        ));
    }
    partitioner.drafts.sort_by(|a, b| {
        (
            &a.structural_root_key,
            &a.source_origin,
            a.repeat_instances
                .iter()
                .map(|instance| (instance.repeat_field, instance.index))
                .collect::<Vec<_>>(),
            &a.identities,
            a.source_root,
        )
            .cmp(&(
                &b.structural_root_key,
                &b.source_origin,
                b.repeat_instances
                    .iter()
                    .map(|instance| (instance.repeat_field, instance.index))
                    .collect::<Vec<_>>(),
                &b.identities,
                b.source_root,
            ))
    });

    let mut identity_keys = partitioner
        .drafts
        .iter()
        .map(|draft| draft.identities.clone())
        .collect::<Vec<_>>();
    identity_keys.sort();
    identity_keys.dedup();
    let identities = identity_keys
        .iter()
        .enumerate()
        .map(|(index, pairs)| {
            Ok(IdentitySet {
                id: u32::try_from(index)
                    .map_err(|_| "pixels::objects: identity set ID overflow".to_string())?,
                pairs: pairs.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let identity_ids = identity_keys
        .into_iter()
        .zip(identities.iter().map(|set| set.id))
        .collect::<BTreeMap<_, _>>();

    let mut ids = BTreeMap::new();
    let mut objects = Vec::with_capacity(partitioner.drafts.len());
    for (index, draft) in partitioner.drafts.into_iter().enumerate() {
        let id = ObjectId(
            u32::try_from(index).map_err(|_| "pixels::objects: object ID overflow".to_string())?,
        );
        ids.insert(draft.key, id);
        objects.push(SmoothObject {
            id,
            source_root: draft.source_root,
            scalar_root: draft.scalar_root,
            bounds: draft.bounds,
            primitive_occurrences: draft.primitive_occurrences,
            support_max: draft.support_max,
            identity_set: *identity_ids.get(&draft.identities).ok_or_else(|| {
                "pixels::objects: missing stable identity-set ID for object draft".to_string()
            })?,
            repeat_instances: draft.repeat_instances,
        });
    }
    Ok(ObjectPartition {
        objects,
        identities,
        csg: remap_expr(expression, &ids)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::arena::NodeOrigin;
    use super::super::graph::{FieldArena, FieldNode, Primitive, TransformProgram};
    use super::*;

    #[test]
    fn repeat_cartesian_order_is_lexicographic_and_negative_stable() {
        let family = vec![
            RepeatInstance {
                repeat_field: FieldId(0),
                equivalent_fields: vec![FieldId(0)],
                axis: Axis::X,
                first: -2,
                index: -2,
                period: F64Interval::point(2.0).unwrap(),
            },
            RepeatInstance {
                repeat_field: FieldId(0),
                equivalent_fields: vec![FieldId(0)],
                axis: Axis::X,
                first: -2,
                index: -1,
                period: F64Interval::point(2.0).unwrap(),
            },
        ];
        let combinations = repeat_combinations(&[family]).unwrap();
        assert_eq!(combinations[0][0].index, -2);
        assert_eq!(combinations[1][0].index, -1);
    }

    #[test]
    fn hostile_graph_that_bypasses_wrapper_normalization_is_internal() {
        let scalar = ScalarId(0);
        let mut fields = FieldArena::new(7);
        let left = fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Plane {
                        normal: [scalar; 3],
                        offset: scalar,
                    }),
                    scalar_value: scalar,
                },
                NodeOrigin::synthetic("left"),
            )
            .unwrap();
        let right = fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Sphere {
                        center: [scalar; 3],
                        radius: scalar,
                    }),
                    scalar_value: scalar,
                },
                NodeOrigin::synthetic("right"),
            )
            .unwrap();
        let hard = fields
            .push(
                FieldNode {
                    kind: FieldKind::HardUnion { a: left, b: right },
                    scalar_value: scalar,
                },
                NodeOrigin::synthetic("hard"),
            )
            .unwrap();
        let wrapped = fields
            .push(
                FieldNode {
                    kind: FieldKind::Transform {
                        child: hard,
                        transform: TransformProgram::Translate { by: [scalar; 3] },
                    },
                    scalar_value: scalar,
                },
                NodeOrigin::synthetic("wrapped"),
            )
            .unwrap();
        let graph = SymbolicGraph {
            renderer_index: 0,
            field_key: String::new(),
            material_key: String::new(),
            params_type: crate::sema::types::Type::Unit,
            material_type: crate::sema::types::Type::Unit,
            params: Vec::new(),
            scalar: super::super::scalar::ScalarArena::new(8),
            fields,
            materials: super::super::material_graph::MaterialArena::new(9),
            field_root: wrapped,
            material_root: super::super::ids::MaterialId(0),
            obligations: Vec::new(),
            quota: Default::default(),
        };
        let error = ensure_smooth_subtree(&graph, wrapped).unwrap_err();
        assert!(error.starts_with("pixels::objects: hard CSG node"));
        assert!(error.contains("escaped source-f32 wrapper normalization"));
    }
}
