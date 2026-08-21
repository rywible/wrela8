//! Structural material-discontinuity obligations.

use std::collections::{BTreeMap, BTreeSet};

use super::bounds::ValueBounds;
use super::features::FeatureRecord;
use super::ids::{FeatureId, MaterialId, ObjectId, ScalarId};
use super::material_graph::MaterialKind;
use super::objects::ObjectPartition;
use super::scalar::ScalarOp;
use super::symbolic::SymbolicGraph;

/// Complete-range topology class for one material-program node.
///
/// `Parameterized` deliberately remains a transfer layer for every runtime
/// value.  That conservative policy avoids an opacity==0/1 topology event;
/// it is the permitted fail-closed alternative in P10.1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OpacityClass {
    Opaque,
    Transparent { lo: f64, hi: f64 },
    Invisible,
    Parameterized,
}

impl OpacityClass {
    pub fn emits_transfer_layer(self) -> bool {
        !matches!(self, Self::Invisible)
    }

    pub fn may_transmit(self) -> bool {
        !matches!(self, Self::Opaque | Self::Invisible)
    }

    pub fn tag(self) -> u64 {
        match self {
            Self::Opaque => 1,
            Self::Transparent { .. } => 2,
            Self::Invisible => 3,
            Self::Parameterized => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialOpacity {
    pub material: MaterialId,
    pub class: OpacityClass,
    pub opacity_lo: f64,
    pub opacity_hi: f64,
    pub maximum_emissive: [f64; 3],
}

fn merge_opacity(
    material: MaterialId,
    children: &[MaterialOpacity],
    parameterized_choice: bool,
) -> Result<MaterialOpacity, String> {
    let first = children
        .first()
        .ok_or_else(|| format!("P019: material {material} has no reachable opacity cases"))?;
    let opacity_lo = children
        .iter()
        .map(|child| child.opacity_lo)
        .fold(f64::INFINITY, f64::min);
    let opacity_hi = children
        .iter()
        .map(|child| child.opacity_hi)
        .fold(f64::NEG_INFINITY, f64::max);
    let maximum_emissive = std::array::from_fn(|channel| {
        children
            .iter()
            .map(|child| child.maximum_emissive[channel])
            .fold(0.0_f64, f64::max)
    });
    let same_class = children.iter().all(|child| child.class == first.class);
    let class = if same_class {
        first.class
    } else if opacity_lo == 0.0 && opacity_hi == 0.0 && maximum_emissive == [0.0; 3] {
        OpacityClass::Invisible
    } else if opacity_lo == 1.0 && opacity_hi == 1.0 {
        OpacityClass::Opaque
    } else if parameterized_choice
        || children
            .iter()
            .any(|child| matches!(child.class, OpacityClass::Parameterized))
    {
        OpacityClass::Parameterized
    } else {
        OpacityClass::Transparent {
            lo: opacity_lo,
            hi: opacity_hi,
        }
    };
    Ok(MaterialOpacity {
        material,
        class,
        opacity_lo,
        opacity_hi,
        maximum_emissive,
    })
}

fn classify_material(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    material: MaterialId,
    memo: &mut BTreeMap<MaterialId, MaterialOpacity>,
    visiting: &mut BTreeSet<MaterialId>,
) -> Result<MaterialOpacity, String> {
    if let Some(classification) = memo.get(&material) {
        return Ok(classification.clone());
    }
    if !visiting.insert(material) {
        return Err(format!(
            "P019: material {material} opacity classification contains a cycle"
        ));
    }
    let classification = match &graph.materials.get(material)?.kind {
        MaterialKind::Sample(sample) => {
            let opacity = values.get(sample.opacity)?;
            if !opacity.lo.is_finite()
                || !opacity.hi.is_finite()
                || opacity.lo < 0.0
                || opacity.hi > 1.0
                || opacity.lo > opacity.hi
            {
                return Err(format!(
                    "P019: material {material} has no finite opacity/radiance-tail bound"
                ));
            }
            let mut maximum_emissive = [0.0; 3];
            for (channel, scalar) in sample.emissive.iter().enumerate() {
                let bound = values.get(*scalar)?;
                if !bound.hi.is_finite() || bound.lo < 0.0 {
                    return Err(format!(
                        "P019: material {material} has no finite opacity/radiance-tail bound"
                    ));
                }
                maximum_emissive[channel] = bound.hi;
            }
            let class = if opacity.lo == 1.0 && opacity.hi == 1.0 {
                OpacityClass::Opaque
            } else if opacity.lo == 0.0 && opacity.hi == 0.0 && maximum_emissive == [0.0; 3] {
                OpacityClass::Invisible
            } else if graph.scalar.get(sample.opacity)?.dependency
                != super::scalar::Dependency::Constant
                && graph.scalar.get(sample.opacity)?.dependency
                    != super::scalar::Dependency::Coordinate
            {
                OpacityClass::Parameterized
            } else {
                OpacityClass::Transparent {
                    lo: opacity.lo,
                    hi: opacity.hi,
                }
            };
            MaterialOpacity {
                material,
                class,
                opacity_lo: opacity.lo,
                opacity_hi: opacity.hi,
                maximum_emissive,
            }
        }
        MaterialKind::Select { predicate, a, b } => {
            let children = [
                classify_material(graph, values, *a, memo, visiting)?,
                classify_material(graph, values, *b, memo, visiting)?,
            ];
            let parameterized_choice =
                graph.scalar.get(*predicate)?.dependency != super::scalar::Dependency::Constant;
            merge_opacity(material, &children, parameterized_choice)?
        }
        MaterialKind::IdentityTable { cases, .. } => {
            let children = cases
                .iter()
                .map(|(_, child)| classify_material(graph, values, *child, memo, visiting))
                .collect::<Result<Vec<_>, _>>()?;
            merge_opacity(material, &children, false)?
        }
    };
    visiting.remove(&material);
    memo.insert(material, classification.clone());
    Ok(classification)
}

pub fn classify_opacity(
    graph: &SymbolicGraph,
    values: &ValueBounds,
) -> Result<Vec<MaterialOpacity>, String> {
    let mut memo = BTreeMap::new();
    for (material, _) in graph.materials.iter() {
        classify_material(graph, values, material, &mut memo, &mut BTreeSet::new())?;
    }
    Ok(memo.into_values().collect())
}

pub fn maximum_radiance_v1(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    config: &super::config::RendererConfig,
) -> Result<[f64; 3], String> {
    let mut incident = config.environment.max.map(f64::from);
    for (slot, range) in config.light_ranges.iter().enumerate() {
        let kind = config
            .light_kinds
            .get(slot)
            .map(String::as_str)
            .unwrap_or("Disabled");
        let scale = match kind {
            "Disabled" => 0.0,
            "Point" => {
                let radius = super::reference::light::POINT_RADIUS_MIN_V1;
                1.0 / (radius * radius)
            }
            "Directional" | "Rectangle" | "Disk" => 1.0,
            other => {
                return Err(format!(
                    "P019: material radiance bound has unknown light kind `{other}`"
                ));
            }
        };
        for channel in 0..3 {
            incident[channel] += f64::from(range.radiance_max[channel]) * scale;
        }
    }
    let mut maximum = config.environment.max.map(f64::from);
    for (_, node) in graph.materials.iter() {
        let MaterialKind::Sample(sample) = &node.kind else {
            continue;
        };
        for channel in 0..3 {
            let base = values.get(sample.base_color[channel])?.hi;
            let emissive = values.get(sample.emissive[channel])?.hi;
            let glossy = values.get(sample.specular_level)?.hi > 0.0
                || values.get(sample.metallic)?.hi > 0.0;
            // Match the sealed P9 terminal BRDF-response envelope. Tail
            // termination must cover the complete shaded layer, including a
            // legal roughness-floor highlight, rather than only Lambertian
            // base color and emissive radiance.
            let brdf_response = base + if glossy { 4.0e23 } else { 0.0 };
            let candidate = emissive + brdf_response * incident[channel];
            if !candidate.is_finite() || candidate < 0.0 {
                return Err(
                    "P019: transparent material has no finite radiance-tail bound".to_string(),
                );
            }
            maximum[channel] = maximum[channel].max(candidate);
        }
    }
    Ok(maximum)
}

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

    fn opacity(
        material: u32,
        class: OpacityClass,
        lo: f64,
        hi: f64,
        maximum_emissive: [f64; 3],
    ) -> MaterialOpacity {
        MaterialOpacity {
            material: MaterialId(material),
            class,
            opacity_lo: lo,
            opacity_hi: hi,
            maximum_emissive,
        }
    }

    #[test]
    fn opacity_merge_preserves_invisible_emissive_and_parameterized_topology() {
        let invisible = opacity(0, OpacityClass::Invisible, 0.0, 0.0, [0.0; 3]);
        assert_eq!(
            merge_opacity(MaterialId(2), &[invisible.clone(), invisible], false)
                .unwrap()
                .class,
            OpacityClass::Invisible
        );
        let emissive = opacity(
            0,
            OpacityClass::Transparent { lo: 0.0, hi: 0.0 },
            0.0,
            0.0,
            [4.0, 0.0, 0.0],
        );
        assert!(emissive.class.emits_transfer_layer());
        let mixed = merge_opacity(
            MaterialId(3),
            &[
                opacity(0, OpacityClass::Opaque, 1.0, 1.0, [0.0; 3]),
                opacity(
                    1,
                    OpacityClass::Transparent { lo: 0.25, hi: 0.75 },
                    0.25,
                    0.75,
                    [1.0, 2.0, 3.0],
                ),
            ],
            true,
        )
        .unwrap();
        assert_eq!(mixed.class, OpacityClass::Parameterized);
        assert_eq!((mixed.opacity_lo, mixed.opacity_hi), (0.25, 1.0));
        assert_eq!(mixed.maximum_emissive, [1.0, 2.0, 3.0]);
    }

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
