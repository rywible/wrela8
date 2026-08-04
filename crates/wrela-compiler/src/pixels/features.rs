//! Exact fused primitive feature decomposition.

use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;

use super::bounds::ValueBounds;
use super::graph::Primitive;
use super::ids::{FeatureId, FieldId, ObjectId};
use super::objects::ObjectPartition;
use super::primitive::{AnalyticPredicate, FeatureKind, OrientationProgram, PredicateProgram};
use super::support::{OccurrenceStep, SupportTable};
use super::symbolic::SymbolicGraph;
use super::world_bounds::{Aabb64, WorldBounds};

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureRecord {
    pub id: FeatureId,
    pub template_id: u32,
    pub object: ObjectId,
    pub primitive: FieldId,
    pub occurrence_path: Vec<OccurrenceStep>,
    pub kind: FeatureKind,
    pub world_bounds: Aabb64,
    pub support_expand: f64,
    pub validity: PredicateProgram,
    pub orientation: OrientationProgram,
    pub identity_set: u32,
    pub scalar_semantic_root: FieldId,
}

fn decomposition(primitive: &Primitive) -> Vec<(FeatureKind, AnalyticPredicate)> {
    match primitive {
        Primitive::Plane { .. } => vec![(FeatureKind::Plane, AnalyticPredicate::Always)],
        Primitive::Sphere { center, radius } => vec![(
            FeatureKind::Quadric,
            AnalyticPredicate::Sphere {
                center: *center,
                radius: *radius,
            },
        )],
        Primitive::Box { center, half } => (0_u8..3)
            .flat_map(|axis| {
                [-1_i8, 1].map(move |sign| {
                    (
                        FeatureKind::Plane,
                        AnalyticPredicate::BoxFace {
                            axis,
                            sign,
                            center: *center,
                            half: *half,
                        },
                    )
                })
            })
            .collect(),
        Primitive::RoundBox {
            center,
            half,
            radius,
        } => (0_u8..3)
            .flat_map(|axis| {
                [-1_i8, 1].map(move |sign| {
                    (
                        FeatureKind::Plane,
                        AnalyticPredicate::RoundBoxFace {
                            axis,
                            sign,
                            center: *center,
                            half: *half,
                            radius: *radius,
                        },
                    )
                })
            })
            .chain((0_u8..3).flat_map(|axis| {
                [-1_i8, 1].into_iter().flat_map(move |a| {
                    [-1_i8, 1].map(move |b| {
                        (
                            FeatureKind::Quadric,
                            AnalyticPredicate::RoundBoxEdge {
                                axis,
                                signs: [a, b],
                                center: *center,
                                half: *half,
                                radius: *radius,
                            },
                        )
                    })
                })
            }))
            .chain([-1_i8, 1].into_iter().flat_map(|x| {
                [-1_i8, 1].into_iter().flat_map(move |y| {
                    [-1_i8, 1].map(move |z| {
                        (
                            FeatureKind::Quadric,
                            AnalyticPredicate::RoundBoxCorner {
                                signs: [x, y, z],
                                center: *center,
                                half: *half,
                                radius: *radius,
                            },
                        )
                    })
                })
            }))
            .collect(),
        Primitive::Capsule { a, b, radius } => std::iter::once((
            FeatureKind::Quadric,
            AnalyticPredicate::SegmentSide {
                a: *a,
                b: *b,
                radius_a: *radius,
                radius_b: *radius,
            },
        ))
        .chain((0_u8..2).map(|endpoint| {
            (
                FeatureKind::Quadric,
                AnalyticPredicate::SegmentCap {
                    endpoint,
                    a: *a,
                    b: *b,
                    radius: *radius,
                    hemisphere: true,
                },
            )
        }))
        .collect(),
        Primitive::FiniteCylinder { a, b, radius } => std::iter::once((
            FeatureKind::Quadric,
            AnalyticPredicate::SegmentSide {
                a: *a,
                b: *b,
                radius_a: *radius,
                radius_b: *radius,
            },
        ))
        .chain((0_u8..2).map(|endpoint| {
            (
                FeatureKind::Plane,
                AnalyticPredicate::SegmentCap {
                    endpoint,
                    a: *a,
                    b: *b,
                    radius: *radius,
                    hemisphere: false,
                },
            )
        }))
        .collect(),
        Primitive::FiniteCone {
            a,
            b,
            radius_a,
            radius_b,
        } => vec![
            (
                FeatureKind::Quadric,
                AnalyticPredicate::SegmentSide {
                    a: *a,
                    b: *b,
                    radius_a: *radius_a,
                    radius_b: *radius_b,
                },
            ),
            (
                FeatureKind::Plane,
                AnalyticPredicate::SegmentCap {
                    endpoint: 0,
                    a: *a,
                    b: *b,
                    radius: *radius_a,
                    hemisphere: false,
                },
            ),
        ],
        Primitive::Torus {
            center,
            axis,
            major,
            minor,
        } => vec![(
            FeatureKind::Quartic,
            AnalyticPredicate::TorusDomain {
                center: *center,
                axis: *axis,
                major: *major,
                minor: *minor,
            },
        )],
    }
}

pub(crate) fn expected_feature_count(primitive: &Primitive) -> usize {
    match primitive {
        Primitive::Plane { .. } | Primitive::Sphere { .. } | Primitive::Torus { .. } => 1,
        Primitive::Box { .. } => 6,
        Primitive::RoundBox { .. } => 26,
        Primitive::Capsule { .. } | Primitive::FiniteCylinder { .. } => 3,
        Primitive::FiniteCone { .. } => 2,
    }
}

fn occurrence_orientation(
    graph: &SymbolicGraph,
    path: &[OccurrenceStep],
) -> Result<OrientationProgram, String> {
    let mut inverted = false;
    let mut deformed = false;
    for pair in path.windows(2) {
        let parent = pair[1];
        match &graph.fields.get(parent.field)?.kind {
            super::graph::FieldKind::SmoothSubtract { .. }
            | super::graph::FieldKind::HardSubtract { .. }
                if parent.child_slot == 1 =>
            {
                inverted = !inverted;
            }
            super::graph::FieldKind::Neg { .. } => {
                inverted = !inverted;
            }
            super::graph::FieldKind::BoundedDisplace { .. } => {
                deformed = true;
            }
            _ => {}
        }
    }
    Ok(match (inverted, deformed) {
        (false, false) => OrientationProgram::Outward,
        (true, false) => OrientationProgram::Inward,
        (false, true) => OrientationProgram::DeformedOutward,
        (true, true) => OrientationProgram::DeformedInward,
    })
}

fn occurrence_base_bounds(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    bounds: &WorldBounds,
    object: &super::objects::SmoothObject,
    path: &[OccurrenceStep],
) -> Result<Option<Aabb64>, String> {
    let Some(primitive) = path.first() else {
        return Ok(None);
    };
    let Some(mut current) = bounds.get(primitive.field)?.bounds else {
        return Ok(None);
    };
    for parent in path.iter().skip(1) {
        match &graph.fields.get(parent.field)?.kind {
            super::graph::FieldKind::Transform { transform, .. } => {
                if current != bounds.world {
                    current = super::world_bounds::transform_bounds(current, transform, values)?;
                }
            }
            super::graph::FieldKind::FiniteRepeat { axis, .. } => {
                if current == bounds.world {
                    continue;
                }
                let instance = object
                    .repeat_instances
                    .iter()
                    .find(|instance| {
                        instance.repeat_field == parent.field
                            || instance.equivalent_fields.contains(&parent.field)
                    })
                    .ok_or_else(|| {
                        format!(
                            "pixels::features: object {} has no fixed repeat instance for {}",
                            object.id, parent.field
                        )
                    })?;
                let component = match axis {
                    super::graph::Axis::X => 0,
                    super::graph::Axis::Y => 1,
                    super::graph::Axis::Z => 2,
                };
                let shift = super::world_bounds::repeat_translation_interval(
                    instance.first,
                    instance.index,
                    instance.period,
                )?;
                current.min[component] =
                    super::reference::interval::next_down(current.min[component] + shift.lo);
                current.max[component] =
                    super::reference::interval::next_up(current.max[component] + shift.hi);
            }
            _ => {}
        }
    }
    Ok(Some(current))
}

pub fn decompose(
    graph: &SymbolicGraph,
    objects: &ObjectPartition,
    values: &ValueBounds,
    bounds: &WorldBounds,
    support: &SupportTable,
) -> Result<Vec<FeatureRecord>, String> {
    let mut features = Vec::new();
    let mut template_ids = BTreeMap::new();
    for object in &objects.objects {
        for leaf_support in &support.get(object.source_root)?.leaf_supports {
            let leaf = leaf_support.leaf();
            let super::graph::FieldKind::Primitive(primitive) = &graph.fields.get(leaf)?.kind
            else {
                return Err(format!(
                    "pixels::features: support leaf {leaf} is not a primitive"
                ));
            };
            let derived_orientation = occurrence_orientation(graph, &leaf_support.path)?;
            let support_expand = leaf_support.total_expand()?.hi;
            let world_bounds = occurrence_base_bounds(
                graph,
                values,
                bounds,
                object,
                &leaf_support.path,
            )?
                    .ok_or_else(|| {
                        format!(
                            "pixels::features: primitive {leaf} occurrence has no geometric path in object {}",
                            object.id
                        )
                    })?
                    .expand(support_expand)?
                    .clip(bounds.world)
                    .and_then(|bound| bound.clip(object.bounds))
                    .ok_or_else(|| {
                        format!(
                            "pixels::features: primitive {leaf} expansion is outside object {}",
                            object.id
                        )
                    })?;
            for (ordinal, (kind, validity)) in decomposition(primitive).into_iter().enumerate() {
                let template_key = (
                    object.source_root,
                    leaf_support.path.clone(),
                    ordinal,
                    object.identity_set,
                );
                let next_template = u32::try_from(template_ids.len())
                    .map_err(|_| "pixels::features: template ID overflow".to_string())?;
                let template_id = *template_ids.entry(template_key).or_insert(next_template);
                let id = FeatureId(
                    u32::try_from(features.len())
                        .map_err(|_| "pixels::features: feature ID overflow".to_string())?,
                );
                features.push(FeatureRecord {
                    id,
                    template_id,
                    object: object.id,
                    primitive: leaf,
                    occurrence_path: leaf_support.path.clone(),
                    kind,
                    world_bounds,
                    support_expand,
                    validity: PredicateProgram {
                        constraints: vec![validity],
                        shared_boundary: true,
                    },
                    orientation: derived_orientation,
                    identity_set: object.identity_set,
                    scalar_semantic_root: object.source_root,
                });
            }
        }
    }
    Ok(features)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::arena::NodeOrigin;
    use crate::pixels::graph::{FieldArena, FieldKind, FieldNode};
    use crate::pixels::ids::ScalarId;
    use crate::pixels::material_graph::MaterialArena;
    use crate::pixels::scalar::ScalarArena;
    use crate::pixels::symbolic::SymbolicGraph;
    use crate::sema::types::Type;

    #[test]
    fn fused_feature_counts_are_exact() {
        let scalar = ScalarId(0);
        let cases = [
            (
                Primitive::Plane {
                    normal: [scalar; 3],
                    offset: scalar,
                },
                1,
            ),
            (
                Primitive::Sphere {
                    center: [scalar; 3],
                    radius: scalar,
                },
                1,
            ),
            (
                Primitive::Box {
                    center: [scalar; 3],
                    half: [scalar; 3],
                },
                6,
            ),
            (
                Primitive::RoundBox {
                    center: [scalar; 3],
                    half: [scalar; 3],
                    radius: scalar,
                },
                26,
            ),
            (
                Primitive::Capsule {
                    a: [scalar; 3],
                    b: [scalar; 3],
                    radius: scalar,
                },
                3,
            ),
            (
                Primitive::FiniteCylinder {
                    a: [scalar; 3],
                    b: [scalar; 3],
                    radius: scalar,
                },
                3,
            ),
            (
                Primitive::FiniteCone {
                    a: [scalar; 3],
                    b: [scalar; 3],
                    radius_a: scalar,
                    radius_b: scalar,
                },
                2,
            ),
            (
                Primitive::Torus {
                    center: [scalar; 3],
                    axis: [scalar; 3],
                    major: scalar,
                    minor: scalar,
                },
                1,
            ),
        ];
        for (primitive, expected) in cases {
            let decomposition = decomposition(&primitive);
            assert_eq!(decomposition.len(), expected);
            assert_eq!(
                decomposition
                    .iter()
                    .map(|(_, predicate)| predicate)
                    .collect::<BTreeSet<_>>()
                    .len(),
                expected,
                "every fused feature needs a distinct analytic validity domain"
            );
        }
    }

    #[test]
    fn closed_primitive_validity_domains_cover_boundary_without_wrong_branches() {
        let scalar = ScalarId(0);
        let matches = |predicate: &AnalyticPredicate, point: [f64; 3]| match predicate {
            AnalyticPredicate::Always
            | AnalyticPredicate::Sphere { .. }
            | AnalyticPredicate::TorusDomain { .. } => true,
            AnalyticPredicate::BoxFace { axis, sign, .. } => {
                let axis = *axis as usize;
                (point[axis] - f64::from(*sign)).abs() < 1.0e-12
                    && (0..3)
                        .filter(|component| *component != axis)
                        .all(|component| point[component].abs() <= 1.0)
            }
            AnalyticPredicate::SegmentSide { .. } => {
                let t = (point[1] + 1.0) / 2.0;
                (0.0..=1.0).contains(&t)
            }
            AnalyticPredicate::SegmentCap {
                endpoint,
                hemisphere,
                ..
            } => {
                let t = (point[1] + 1.0) / 2.0;
                if *hemisphere {
                    if *endpoint == 0 { t <= 0.0 } else { t >= 1.0 }
                } else if *endpoint == 0 {
                    (point[1] + 1.0).abs() < 1.0e-12 && point[0].hypot(point[2]) <= 1.0
                } else {
                    (point[1] - 1.0).abs() < 1.0e-12 && point[0].hypot(point[2]) <= 1.0
                }
            }
            _ => false,
        };
        let assert_domains = |primitive: Primitive, samples: &[([f64; 3], usize)]| {
            let features = decomposition(&primitive);
            for (point, expected_count) in samples {
                let count = features
                    .iter()
                    .filter(|(_, predicate)| matches(predicate, *point))
                    .count();
                assert_eq!(
                    count, *expected_count,
                    "{primitive:?} validity mismatch at {point:?}"
                );
            }
        };

        assert_domains(
            Primitive::Plane {
                normal: [scalar; 3],
                offset: scalar,
            },
            &[([0.0, 0.0, 0.0], 1)],
        );
        assert_domains(
            Primitive::Sphere {
                center: [scalar; 3],
                radius: scalar,
            },
            &[([1.0, 0.0, 0.0], 1)],
        );
        assert_domains(
            Primitive::Box {
                center: [scalar; 3],
                half: [scalar; 3],
            },
            &[
                ([-1.0, 0.0, 0.0], 1),
                ([1.0, 0.0, 0.0], 1),
                ([0.0, -1.0, 0.0], 1),
                ([0.0, 1.0, 0.0], 1),
                ([0.0, 0.0, -1.0], 1),
                ([0.0, 0.0, 1.0], 1),
                ([1.0, 1.0, 0.0], 2),
            ],
        );
        assert_domains(
            Primitive::Capsule {
                a: [scalar; 3],
                b: [scalar; 3],
                radius: scalar,
            },
            &[
                ([1.0, 0.0, 0.0], 1),
                ([0.0, -2.0, 0.0], 1),
                ([0.0, 2.0, 0.0], 1),
            ],
        );
        assert_domains(
            Primitive::FiniteCylinder {
                a: [scalar; 3],
                b: [scalar; 3],
                radius: scalar,
            },
            &[
                ([1.0, 0.0, 0.0], 1),
                ([0.0, -1.0, 0.0], 2),
                ([0.0, 1.0, 0.0], 2),
            ],
        );
        assert_domains(
            Primitive::FiniteCone {
                a: [scalar; 3],
                b: [scalar; 3],
                radius_a: scalar,
                radius_b: scalar,
            },
            &[
                ([1.0, 0.0, 0.0], 1),
                ([0.0, -1.0, 0.0], 2),
                ([0.0, 1.0, 0.0], 1),
            ],
        );
        assert_domains(
            Primitive::Torus {
                center: [scalar; 3],
                axis: [scalar; 3],
                major: scalar,
                minor: scalar,
            },
            &[([2.0, 0.0, 0.0], 1)],
        );
    }

    #[test]
    fn production_predicates_cover_transformed_parameterized_and_tapered_boundaries() {
        use super::super::primitive::contains_boundary_point;

        let ids = (0_u32..20).map(ScalarId).collect::<Vec<_>>();
        let center = [ids[0], ids[1], ids[2]];
        let half = [ids[3], ids[4], ids[5]];
        let radius = ids[6];
        let a = [ids[7], ids[8], ids[9]];
        let b = [ids[10], ids[11], ids[12]];
        let radius_a = ids[13];
        let radius_b = ids[14];
        let axis = [ids[15], ids[16], ids[17]];
        let major = ids[18];
        let minor = ids[19];
        let values = [
            0.5, -0.25, 0.75, // center
            1.0, 1.5, 0.5,  // half
            0.25, // radius
            0.0, -1.0, 0.0, // a
            0.0, 2.0, 0.0, // b
            0.8, 0.2, // tapered radii
            0.0, 1.0, 0.0, // torus axis
            2.0, 0.5, // torus radii
        ];
        let resolver = |id: ScalarId| {
            values
                .get(id.0 as usize)
                .copied()
                .ok_or_else(|| format!("missing test coefficient {id}"))
        };
        let count = |primitive: Primitive, point: [f64; 3]| {
            decomposition(&primitive)
                .iter()
                .filter(|(_, predicate)| {
                    contains_boundary_point(predicate, point, &resolver, 1.0e-10).unwrap()
                })
                .count()
        };

        assert_eq!(
            count(Primitive::Sphere { center, radius }, [0.75, -0.25, 0.75]),
            1
        );
        assert_eq!(count(Primitive::Box { center, half }, [1.5, 0.0, 0.75]), 1);
        let edge = 0.25 / 2.0_f64.sqrt();
        assert_eq!(
            count(
                Primitive::RoundBox {
                    center,
                    half,
                    radius,
                },
                [1.5 + edge, -0.25, 1.25 + edge],
            ),
            1
        );
        assert_eq!(
            count(Primitive::Capsule { a, b, radius }, [0.25, 0.5, 0.0],),
            1
        );
        assert_eq!(
            count(Primitive::FiniteCylinder { a, b, radius }, [0.0, -1.0, 0.0],),
            1
        );
        assert_eq!(
            count(
                Primitive::FiniteCone {
                    a,
                    b,
                    radius_a,
                    radius_b,
                },
                [0.5, 0.5, 0.0],
            ),
            1,
            "tapered side must use the interpolated radius"
        );
        assert_eq!(
            count(
                Primitive::Torus {
                    center,
                    axis,
                    major,
                    minor,
                },
                [3.0, -0.25, 0.75],
            ),
            1
        );

        // Predicates consume local coordinates. This translated world sample
        // maps back to the same parameterized sphere boundary.
        let translation = [4.0, -3.0, 2.0];
        let world = [4.75, -3.25, 2.75];
        let local = std::array::from_fn(|component| world[component] - translation[component]);
        assert_eq!(count(Primitive::Sphere { center, radius }, local), 1);

        // The exact same production predicate accepts both endpoints of a
        // declared radius range when supplied the corresponding coefficient
        // snapshot.
        let sphere = decomposition(&Primitive::Sphere { center, radius });
        for ranged_radius in [0.2, 0.8] {
            let ranged = |id: ScalarId| {
                if id == radius {
                    Ok(ranged_radius)
                } else {
                    resolver(id)
                }
            };
            assert!(
                contains_boundary_point(
                    &sphere[0].1,
                    [0.5 + ranged_radius, -0.25, 0.75],
                    &ranged,
                    1.0e-10,
                )
                .unwrap()
            );
        }
    }

    #[test]
    fn rounded_box_interior_samples_select_only_the_geometric_branch() {
        let scalar = ScalarId(0);
        let predicates = decomposition(&Primitive::RoundBox {
            center: [scalar; 3],
            half: [scalar; 3],
            radius: scalar,
        });
        let half = [1.0; 3];
        let sign = |value: f64| if value < 0.0 { -1 } else { 1 };
        let matches = |predicate: &AnalyticPredicate, point: [f64; 3]| match predicate {
            AnalyticPredicate::RoundBoxFace {
                axis,
                sign: expected,
                ..
            } => {
                sign(point[*axis as usize]) == *expected
                    && (0..3)
                        .filter(|component| *component != *axis as usize)
                        .all(|component| point[component].abs() < half[component])
            }
            AnalyticPredicate::RoundBoxEdge {
                axis,
                signs: expected,
                ..
            } => {
                let radial = (0..3)
                    .filter(|component| *component != *axis as usize)
                    .collect::<Vec<_>>();
                point[*axis as usize].abs() < half[*axis as usize]
                    && sign(point[radial[0]]) == expected[0]
                    && sign(point[radial[1]]) == expected[1]
                    && radial
                        .iter()
                        .all(|component| point[*component].abs() > half[*component])
            }
            AnalyticPredicate::RoundBoxCorner {
                signs: expected, ..
            } => (0..3).all(|component| {
                point[component].abs() > half[component]
                    && sign(point[component]) == expected[component]
            }),
            _ => false,
        };

        let radius = 0.25;
        let mut samples = Vec::new();
        for axis in 0..3 {
            for direction in [-1.0, 1.0] {
                let mut point = [0.0; 3];
                point[axis] = direction * (half[axis] + radius);
                samples.push(point);
            }
        }
        for axis in 0..3 {
            let radial = (0..3)
                .filter(|component| *component != axis)
                .collect::<Vec<_>>();
            for a in [-1.0, 1.0] {
                for b in [-1.0, 1.0] {
                    let mut point = [0.0; 3];
                    point[radial[0]] = a * (half[radial[0]] + radius / 2.0_f64.sqrt());
                    point[radial[1]] = b * (half[radial[1]] + radius / 2.0_f64.sqrt());
                    samples.push(point);
                }
            }
        }
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    let offset = radius / 3.0_f64.sqrt();
                    samples.push([
                        x * (half[0] + offset),
                        y * (half[1] + offset),
                        z * (half[2] + offset),
                    ]);
                }
            }
        }
        assert_eq!(samples.len(), 26);
        for point in samples {
            assert_eq!(
                predicates
                    .iter()
                    .filter(|(_, predicate)| matches(predicate, point))
                    .count(),
                1,
                "rounded-box branch predicates overlap at interior sample {point:?}"
            );
        }
    }

    #[test]
    fn shared_child_occurrences_keep_subtraction_edge_orientation() {
        let scalar = ScalarId(0);
        let mut fields = FieldArena::new(2);
        let primitive = fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Sphere {
                        center: [scalar; 3],
                        radius: scalar,
                    }),
                    scalar_value: scalar,
                },
                NodeOrigin::synthetic("primitive"),
            )
            .unwrap();
        let subtract = fields
            .push(
                FieldNode {
                    kind: FieldKind::SmoothSubtract {
                        a: primitive,
                        b: primitive,
                        k: scalar,
                    },
                    scalar_value: scalar,
                },
                NodeOrigin::synthetic("subtract"),
            )
            .unwrap();
        let deformed = fields
            .push(
                FieldNode {
                    kind: FieldKind::BoundedDisplace {
                        base: subtract,
                        displacement: scalar,
                        contract: super::super::graph::DerivedDeformContract {
                            amplitude_bound: scalar,
                            gradient_bound: scalar,
                            hessian_bound: scalar,
                            third_derivative_bound: scalar,
                            coordinate_x: scalar,
                            frequency: scalar,
                            phase: scalar,
                            derivation: super::super::graph::ClosedDeformDerivation::SinusoidalX,
                        },
                    },
                    scalar_value: scalar,
                },
                NodeOrigin::synthetic("deformed"),
            )
            .unwrap();
        let graph = SymbolicGraph {
            renderer_index: 0,
            field_key: String::new(),
            material_key: String::new(),
            params_type: Type::Unit,
            material_type: Type::Unit,
            params: Vec::new(),
            scalar: ScalarArena::new(1),
            fields,
            materials: MaterialArena::new(3),
            field_root: deformed,
            material_root: super::super::ids::MaterialId(0),
            obligations: Vec::new(),
            quota: Default::default(),
        };
        let left = [
            OccurrenceStep {
                field: primitive,
                child_slot: 0,
            },
            OccurrenceStep {
                field: subtract,
                child_slot: 0,
            },
        ];
        let right = [
            OccurrenceStep {
                field: primitive,
                child_slot: 0,
            },
            OccurrenceStep {
                field: subtract,
                child_slot: 1,
            },
        ];
        assert_eq!(
            occurrence_orientation(&graph, &left).unwrap(),
            OrientationProgram::Outward
        );
        assert_eq!(
            occurrence_orientation(&graph, &right).unwrap(),
            OrientationProgram::Inward
        );
        let deformed_right = [
            OccurrenceStep {
                field: primitive,
                child_slot: 0,
            },
            OccurrenceStep {
                field: subtract,
                child_slot: 1,
            },
            OccurrenceStep {
                field: deformed,
                child_slot: 0,
            },
        ];
        assert_eq!(
            occurrence_orientation(&graph, &deformed_right).unwrap(),
            OrientationProgram::DeformedInward
        );
    }
}
