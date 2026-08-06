//! Structural material-discontinuity obligations.

use std::collections::{BTreeMap, BTreeSet};

use super::bounds::ValueBounds;
use super::features::FeatureRecord;
use super::ids::{FeatureId, MaterialId, ObjectId, ScalarId};
use super::material_graph::MaterialKind;
use super::objects::ObjectPartition;
use super::scalar::ScalarOp;
use super::symbolic::SymbolicGraph;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialEventKind {
    NominalIdentity,
    ScalarThreshold,
    ProceduralBoundary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialEvent {
    pub predicate: ScalarId,
    pub kind: MaterialEventKind,
    pub owners: Vec<ObjectId>,
    pub feature_owners: Vec<FeatureId>,
    pub crossing_bound: u32,
    pub origin: super::arena::NodeOrigin,
}

#[derive(Clone, Copy)]
struct AnalyticClass {
    degree: u32,
    crossings: u32,
    procedural: bool,
}

fn add_u32(a: u32, b: u32, description: &str) -> Result<u32, String> {
    a.checked_add(b)
        .ok_or_else(|| format!("P014: material {description} bound overflows u32"))
}

fn analytic_class(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    id: ScalarId,
    memo: &mut BTreeMap<ScalarId, AnalyticClass>,
) -> Result<AnalyticClass, String> {
    if let Some(class) = memo.get(&id) {
        return Ok(*class);
    }
    let child = |id, memo: &mut BTreeMap<_, _>| analytic_class(graph, values, id, memo);
    let class = match &graph.scalar.get(id)?.op {
        ScalarOp::ConstF32(_) | ScalarOp::ConstF64(_) | ScalarOp::Param(_) => AnalyticClass {
            degree: 0,
            crossings: 0,
            procedural: false,
        },
        ScalarOp::CoordX | ScalarOp::CoordY | ScalarOp::CoordZ | ScalarOp::SurfacePosition(_) => {
            AnalyticClass {
                degree: 1,
                crossings: 1,
                procedural: false,
            }
        }
        ScalarOp::Neg(value) => child(*value, memo)?,
        ScalarOp::Abs(value) => {
            let value = child(*value, memo)?;
            AnalyticClass {
                degree: value.degree,
                crossings: value
                    .crossings
                    .checked_mul(2)
                    .ok_or_else(|| "P014: material kink crossing bound overflows u32".to_string())?
                    .max(1),
                // The |x| kink itself is an event boundary even when the
                // surrounding comparison has a stable sign on either side.
                procedural: true,
            }
        }
        ScalarOp::Add(a, b) | ScalarOp::Sub(a, b) => {
            let a = child(*a, memo)?;
            let b = child(*b, memo)?;
            AnalyticClass {
                degree: a.degree.max(b.degree),
                crossings: add_u32(a.crossings, b.crossings, "sum crossing")?.max(1),
                procedural: a.procedural || b.procedural,
            }
        }
        ScalarOp::Mul(a, b) => {
            let a = child(*a, memo)?;
            let b = child(*b, memo)?;
            let degree = add_u32(a.degree, b.degree, "polynomial degree")?;
            if degree > 4 {
                return Err(format!(
                    "P014: material scalar {id} exceeds the v1 analytic degree limit"
                ));
            }
            AnalyticClass {
                degree,
                crossings: add_u32(a.crossings, b.crossings, "product crossing")?.max(degree),
                procedural: a.procedural || b.procedural,
            }
        }
        ScalarOp::Div(a, b) => {
            let a = child(*a, memo)?;
            let b = child(*b, memo)?;
            if b.degree != 0 {
                return Err(format!(
                    "P014: material scalar {id} has a spatially varying denominator"
                ));
            }
            a
        }
        ScalarOp::SinRestricted(argument, _) | ScalarOp::CosRestricted(argument, _) => {
            let argument_class = child(*argument, memo)?;
            if argument_class.degree > 1 {
                return Err(format!(
                    "P014: material trigonometric argument {argument} is not affine"
                ));
            }
            let cycles = (values.get(*argument)?.width() / std::f64::consts::PI).ceil();
            if !cycles.is_finite() || cycles > f64::from(u32::MAX - 2) {
                return Err(format!(
                    "P014: material trigonometric argument {argument} has no finite crossing bound"
                ));
            }
            AnalyticClass {
                degree: 0,
                crossings: (cycles as u32).saturating_add(2),
                procedural: true,
            }
        }
        ScalarOp::Compare { .. } => {
            return Err(format!(
                "P014: nested comparison {id} is not an analytic threshold expression"
            ));
        }
        _ => {
            return Err(format!(
                "P014: material scalar {id} uses an unsupported discontinuous or nonanalytic operation"
            ));
        }
    };
    memo.insert(id, class);
    Ok(class)
}

fn predicate_crossing_bound(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    predicate: ScalarId,
) -> Result<Option<(u32, MaterialEventKind)>, String> {
    let ScalarOp::Compare { a, b, .. } = graph.scalar.get(predicate)?.op else {
        return Err(format!(
            "P014: material discontinuity {predicate} has no finite analytic crossing family"
        ));
    };
    // Even when structural interval propagation proves this comparison stable,
    // retain the analytic obligation. The projective omission pass owns the
    // complete-box strict-sign proof and its auditable exclusion record.
    if a == b {
        return Ok(None);
    }
    let mut memo = BTreeMap::new();
    let a = analytic_class(graph, values, a, &mut memo)?;
    let b = analytic_class(graph, values, b, &mut memo)?;
    Ok(Some((
        add_u32(a.crossings, b.crossings, "predicate crossing")?
            .max(a.degree.max(b.degree))
            .max(1),
        if a.procedural || b.procedural {
            MaterialEventKind::ProceduralBoundary
        } else {
            MaterialEventKind::ScalarThreshold
        },
    )))
}

fn owners_for_identity(
    objects: &ObjectPartition,
    identity: &super::graph::CanonicalIdentity,
) -> BTreeSet<ObjectId> {
    objects
        .objects
        .iter()
        .filter(|object| {
            objects.identities[object.identity_set as usize]
                .pairs
                .iter()
                .any(|pair| &pair.material == identity)
        })
        .map(|object| object.id)
        .collect()
}

fn restrict_owners(
    candidates: &BTreeSet<ObjectId>,
    incoming: &BTreeSet<ObjectId>,
) -> BTreeSet<ObjectId> {
    candidates.intersection(incoming).copied().collect()
}

fn visit_material(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    objects: &ObjectPartition,
    id: MaterialId,
    owners: &BTreeSet<ObjectId>,
    events: &mut BTreeMap<ScalarId, (u32, MaterialEventKind, BTreeSet<ObjectId>)>,
    seen: &mut BTreeSet<(MaterialId, Vec<ObjectId>)>,
) -> Result<(), String> {
    let owner_key = owners.iter().copied().collect::<Vec<_>>();
    if !seen.insert((id, owner_key)) {
        return Ok(());
    }
    match &graph.materials.get(id)?.kind {
        MaterialKind::Sample(sample) => {
            if let Some(pattern) = &sample.pattern {
                if pattern.width == 0 || pattern.height == 0 {
                    return Err(
                        "P014: immutable texture has zero dimensions after symbolic verification"
                            .to_string(),
                    );
                }
            }
            Ok(())
        }
        MaterialKind::Select { predicate, a, b } => {
            if let Some((crossing_bound, kind)) =
                predicate_crossing_bound(graph, values, *predicate)?
            {
                let entry =
                    events
                        .entry(*predicate)
                        .or_insert((crossing_bound, kind, BTreeSet::new()));
                entry.0 = entry.0.max(crossing_bound);
                if entry.1 != kind {
                    return Err(format!(
                        "P014: material predicate {predicate} has inconsistent event classification"
                    ));
                }
                entry.2.extend(owners);
            }
            visit_material(graph, values, objects, *a, owners, events, seen)?;
            visit_material(graph, values, objects, *b, owners, events, seen)
        }
        MaterialKind::IdentityTable { cases, .. } => {
            for (identity, material) in cases {
                let case_owners = restrict_owners(&owners_for_identity(objects, identity), owners);
                visit_material(
                    graph,
                    values,
                    objects,
                    *material,
                    &case_owners,
                    events,
                    seen,
                )?;
            }
            Ok(())
        }
    }
}

pub fn compile(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    objects: &ObjectPartition,
    features: &[FeatureRecord],
) -> Result<Vec<MaterialEvent>, String> {
    let all_owners = objects
        .objects
        .iter()
        .map(|object| object.id)
        .collect::<BTreeSet<_>>();
    let mut events = BTreeMap::new();
    visit_material(
        graph,
        values,
        objects,
        graph.material_root,
        &all_owners,
        &mut events,
        &mut BTreeSet::new(),
    )?;
    events
        .into_iter()
        .map(|(predicate, (crossing_bound, kind, owners))| {
            let owners = owners.into_iter().collect::<Vec<_>>();
            let owner_set = owners.iter().copied().collect::<BTreeSet<_>>();
            let feature_owners = features
                .iter()
                .filter(|feature| owner_set.contains(&feature.object))
                .map(|feature| feature.id)
                .collect();
            Ok(MaterialEvent {
                predicate,
                kind,
                owners,
                feature_owners,
                crossing_bound,
                origin: graph.scalar.origin(predicate)?.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::arena::NodeOrigin;
    use super::super::bounds::ScalarBound;
    use super::super::graph::FieldArena;
    use super::super::ids::{FieldId, MaterialId};
    use super::super::material_graph::MaterialArena;
    use super::super::scalar::{
        CompareOp, Dependency, ScalarArena, ScalarNode, ScalarOp, SemanticOpId,
    };
    use super::super::symbolic::SymbolicGraph;
    use super::*;
    use crate::sema::types::Type;

    #[test]
    fn nested_identity_owner_sets_cannot_escape_the_incoming_branch() {
        let candidates = BTreeSet::from([ObjectId(0), ObjectId(1), ObjectId(2)]);
        let incoming = BTreeSet::from([ObjectId(1), ObjectId(3)]);
        assert_eq!(
            restrict_owners(&candidates, &incoming),
            BTreeSet::from([ObjectId(1)])
        );
    }

    #[test]
    fn bounded_trigonometric_threshold_is_a_procedural_event_family() {
        let mut scalar = ScalarArena::new(1);
        let x = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::CoordX,
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("x"),
            )
            .unwrap();
        let sine = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::SinRestricted(x, SemanticOpId::SinRestrictedF32V1),
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("sin"),
            )
            .unwrap();
        let zero = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.0_f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("zero"),
            )
            .unwrap();
        let predicate = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Compare {
                        op: CompareOp::Ge,
                        a: sine,
                        b: zero,
                    },
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("predicate"),
            )
            .unwrap();
        let absolute = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Abs(x),
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("absolute-kink"),
            )
            .unwrap();
        let kink_predicate = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Compare {
                        op: CompareOp::Ge,
                        a: absolute,
                        b: zero,
                    },
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("kink-predicate"),
            )
            .unwrap();
        let graph = SymbolicGraph {
            renderer_index: 0,
            field_key: String::new(),
            material_key: String::new(),
            params_type: Type::Unit,
            material_type: Type::Unit,
            params: Vec::new(),
            scalar,
            fields: FieldArena::new(2),
            materials: MaterialArena::new(3),
            field_root: FieldId(0),
            material_root: MaterialId(0),
            obligations: Vec::new(),
            quota: Default::default(),
        };
        let values = ValueBounds {
            scalar: [
                (
                    x,
                    ScalarBound {
                        value: super::super::reference::interval::F64Interval::new(-10.0, 10.0)
                            .unwrap(),
                        rule: "test",
                    },
                ),
                (
                    sine,
                    ScalarBound {
                        value: super::super::reference::interval::F64Interval::new(-1.0, 1.0)
                            .unwrap(),
                        rule: "test",
                    },
                ),
                (
                    zero,
                    ScalarBound {
                        value: super::super::reference::interval::F64Interval::point(0.0).unwrap(),
                        rule: "test",
                    },
                ),
                (
                    predicate,
                    ScalarBound {
                        value: super::super::reference::interval::F64Interval::new(0.0, 1.0)
                            .unwrap(),
                        rule: "test",
                    },
                ),
                (
                    absolute,
                    ScalarBound {
                        value: super::super::reference::interval::F64Interval::new(0.0, 10.0)
                            .unwrap(),
                        rule: "test",
                    },
                ),
                (
                    kink_predicate,
                    ScalarBound {
                        value: super::super::reference::interval::F64Interval::new(0.0, 1.0)
                            .unwrap(),
                        rule: "test",
                    },
                ),
            ]
            .into_iter()
            .collect(),
        };
        let (_, kind) = predicate_crossing_bound(&graph, &values, predicate)
            .unwrap()
            .unwrap();
        assert_eq!(kind, MaterialEventKind::ProceduralBoundary);
        let (crossings, kind) = predicate_crossing_bound(&graph, &values, kink_predicate)
            .unwrap()
            .expect("an absolute-value kink requires an event coverage path");
        assert!(crossings >= 2);
        assert_eq!(kind, MaterialEventKind::ProceduralBoundary);
    }
}
