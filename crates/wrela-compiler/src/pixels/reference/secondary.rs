//! Deterministic surface-bound BVH and fail-closed secondary segments.

use super::interval::F64Interval;
use super::light::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    fn centroid(self) -> Vec3 {
        self.min.add(self.max).scale(0.5)
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: Vec3 {
                x: self.min.x.min(other.min.x),
                y: self.min.y.min(other.min.y),
                z: self.min.z.min(other.min.z),
            },
            max: Vec3 {
                x: self.max.x.max(other.max.x),
                y: self.max.y.max(other.max.y),
                z: self.max.z.max(other.max.z),
            },
        }
    }

    fn intersects(self, query: SegmentQuery) -> bool {
        let mut lo = query.t_min;
        let mut hi = query.t_max;
        for (origin, direction, minimum, maximum) in [
            (query.origin.x, query.direction.x, self.min.x, self.max.x),
            (query.origin.y, query.direction.y, self.min.y, self.max.y),
            (query.origin.z, query.direction.z, self.min.z, self.max.z),
        ] {
            if direction == 0.0 {
                if origin < minimum || origin > maximum {
                    return false;
                }
                continue;
            }
            let a = (minimum - origin) / direction;
            let b = (maximum - origin) / direction;
            lo = lo.max(a.min(b));
            hi = hi.min(a.max(b));
            if lo > hi {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceObject {
    pub object_id: u32,
    pub feature_first: u32,
    pub feature_count: u32,
    pub bounds: Aabb,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentQuery {
    pub origin: Vec3,
    pub direction: Vec3,
    pub t_min: f64,
    pub t_max: f64,
    pub exclude_feature: u32,
    pub exclusion_corridor_max: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SegmentVisibility {
    Clear,
    Blocked {
        first_t: F64Interval,
        identity_set: u32,
    },
    Unresolved(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    pub(crate) bounds: Aabb,
    pub(crate) first: usize,
    pub(crate) count: usize,
    pub(crate) left: Option<usize>,
    pub(crate) right: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceBvh {
    objects: Vec<SurfaceObject>,
    nodes: Vec<Node>,
    root: usize,
    pub stack_capacity: usize,
}

fn bounds(objects: &[SurfaceObject]) -> Result<Aabb, String> {
    let first = objects
        .first()
        .ok_or_else(|| "P027: secondary BVH cannot be empty".to_string())?;
    Ok(objects
        .iter()
        .skip(1)
        .fold(first.bounds, |bounds, object| bounds.union(object.bounds)))
}

fn build_node(
    objects: &mut [SurfaceObject],
    base: usize,
    nodes: &mut Vec<Node>,
    depth: usize,
) -> Result<usize, String> {
    let node_bounds = bounds(objects)?;
    let index = nodes.len();
    nodes.push(Node {
        bounds: node_bounds,
        first: base,
        count: objects.len(),
        left: None,
        right: None,
    });
    if objects.len() <= 2 {
        return Ok(index);
    }
    let mut centroid_min = objects[0].bounds.centroid();
    let mut centroid_max = centroid_min;
    for object in &objects[1..] {
        let centroid = object.bounds.centroid();
        centroid_min.x = centroid_min.x.min(centroid.x);
        centroid_min.y = centroid_min.y.min(centroid.y);
        centroid_min.z = centroid_min.z.min(centroid.z);
        centroid_max.x = centroid_max.x.max(centroid.x);
        centroid_max.y = centroid_max.y.max(centroid.y);
        centroid_max.z = centroid_max.z.max(centroid.z);
    }
    let extents = [
        centroid_max.x - centroid_min.x,
        centroid_max.y - centroid_min.y,
        centroid_max.z - centroid_min.z,
    ];
    // Largest centroid extent, with x/y/z tie order.
    let axis = (0..3)
        .max_by(|a, b| extents[*a].total_cmp(&extents[*b]).then_with(|| b.cmp(a)))
        .unwrap();
    let component = |object: &SurfaceObject| match axis {
        0 => object.bounds.centroid().x,
        1 => object.bounds.centroid().y,
        _ => object.bounds.centroid().z,
    };
    objects.sort_by(|a, b| {
        component(a)
            .total_cmp(&component(b))
            .then_with(|| a.object_id.cmp(&b.object_id))
    });
    let middle = objects.len() / 2;
    let (left_objects, right_objects) = objects.split_at_mut(middle);
    let left = build_node(left_objects, base, nodes, depth + 1)?;
    let right = build_node(right_objects, base + middle, nodes, depth + 1)?;
    nodes[index].left = Some(left);
    nodes[index].right = Some(right);
    Ok(index)
}

impl SurfaceBvh {
    pub fn build(mut objects: Vec<SurfaceObject>) -> Result<Self, String> {
        if objects.iter().any(|object| {
            object.feature_count == 0
                || ![
                    object.bounds.min.x,
                    object.bounds.min.y,
                    object.bounds.min.z,
                    object.bounds.max.x,
                    object.bounds.max.y,
                    object.bounds.max.z,
                ]
                .into_iter()
                .all(f64::is_finite)
                || object.bounds.min.x > object.bounds.max.x
                || object.bounds.min.y > object.bounds.max.y
                || object.bounds.min.z > object.bounds.max.z
        }) {
            return Err("P027: invalid secondary surface-object bounds".to_string());
        }
        objects.sort_by_key(|object| object.object_id);
        if objects
            .windows(2)
            .any(|pair| pair[0].object_id == pair[1].object_id)
        {
            return Err("P027: duplicate secondary object ID".to_string());
        }
        let mut feature_ranges = objects
            .iter()
            .map(|object| {
                object
                    .feature_first
                    .checked_add(object.feature_count)
                    .map(|end| (object.feature_first, end, object.object_id))
                    .ok_or_else(|| "P027: secondary feature range overflow".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        feature_ranges.sort_unstable();
        if feature_ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err("P027: overlapping secondary feature ranges".to_string());
        }
        let mut nodes = Vec::new();
        let root = build_node(&mut objects, 0, &mut nodes, 0)?;
        let stack_capacity = (usize::BITS - objects.len().leading_zeros()) as usize * 2 + 1;
        Ok(Self {
            objects,
            nodes,
            root,
            stack_capacity,
        })
    }

    pub(crate) fn wire_parts(&self) -> (&[SurfaceObject], &[Node], usize) {
        (&self.objects, &self.nodes, self.root)
    }

    pub fn candidates(
        &self,
        query: SegmentQuery,
        stack: &mut [usize],
        output: &mut Vec<u32>,
    ) -> Result<(), String> {
        if ![
            query.origin.x,
            query.origin.y,
            query.origin.z,
            query.direction.x,
            query.direction.y,
            query.direction.z,
            query.t_min,
            query.t_max,
            query.exclusion_corridor_max,
        ]
        .into_iter()
        .all(f64::is_finite)
            || query.t_min < 0.0
            || query.t_min > query.t_max
            || query.exclusion_corridor_max < query.t_min
            || query.direction.dot(query.direction) <= 0.0
            || stack.len() < self.stack_capacity
        {
            return Err("P027: invalid or under-capacity secondary query".to_string());
        }
        output.clear();
        let mut count = 1_usize;
        stack[0] = self.root;
        while count != 0 {
            count -= 1;
            let node = &self.nodes[stack[count]];
            if !node.bounds.intersects(query) {
                continue;
            }
            match (node.left, node.right) {
                (Some(left), Some(right)) => {
                    if count + 2 > stack.len() {
                        return Err("P027: secondary BVH stack exhausted".to_string());
                    }
                    // Left is visited first; stable IDs make this independent of allocation.
                    stack[count] = right;
                    stack[count + 1] = left;
                    count += 2;
                }
                (None, None) => output.extend(
                    self.objects[node.first..node.first + node.count]
                        .iter()
                        .filter(|object| object.bounds.intersects(query))
                        .map(|object| object.object_id),
                ),
                _ => return Err("P027: corrupt secondary BVH node".to_string()),
            }
        }
        output.sort_unstable();
        output.dedup();
        Ok(())
    }

    /// `isolate` returns all feature roots for one candidate object after CSG
    /// occupancy, as `(t interval, exact feature, identity set)` tuples.
    pub fn visibility(
        &self,
        query: SegmentQuery,
        stack: &mut [usize],
        candidates: &mut Vec<u32>,
        isolate: impl Fn(u32) -> Result<Vec<(F64Interval, u32, u32)>, String>,
    ) -> SegmentVisibility {
        if self.candidates(query, stack, candidates).is_err() {
            return SegmentVisibility::Unresolved("bvh");
        }
        let mut roots = Vec::new();
        for object in candidates.iter().copied() {
            let Some(metadata) = self.objects.iter().find(|entry| entry.object_id == object) else {
                return SegmentVisibility::Unresolved("bvh-object");
            };
            let feature_end = match metadata.feature_first.checked_add(metadata.feature_count) {
                Some(end) => end,
                None => return SegmentVisibility::Unresolved("feature-range"),
            };
            let Ok(found) = isolate(object) else {
                return SegmentVisibility::Unresolved("root-isolation");
            };
            if found
                .iter()
                .any(|(_, feature, _)| !(metadata.feature_first..feature_end).contains(feature))
            {
                return SegmentVisibility::Unresolved("feature-range");
            }
            let segment = F64Interval {
                lo: query.t_min,
                hi: query.t_max,
            };
            for (root, feature, identity_set) in found {
                let Some(mut root) = root.intersect(segment) else {
                    continue;
                };
                if feature == query.exclude_feature {
                    if root.hi <= query.exclusion_corridor_max {
                        continue;
                    }
                    root.lo = root.lo.max(query.exclusion_corridor_max);
                }
                roots.push((root, feature, identity_set));
            }
        }
        roots.sort_by(|a, b| a.0.lo.total_cmp(&b.0.lo).then_with(|| a.1.cmp(&b.1)));
        let Some((first, _, identity_set)) = roots.first().copied() else {
            return SegmentVisibility::Clear;
        };
        if roots.get(1).is_some_and(|second| first.hi >= second.0.lo) {
            return SegmentVisibility::Unresolved("front-order");
        }
        SegmentVisibility::Blocked {
            first_t: first,
            identity_set,
        }
    }
}

pub fn offset_origin(position: Vec3, normal: Vec3, epsilon: f64) -> Result<Vec3, String> {
    if !epsilon.is_finite() || epsilon <= 0.0 {
        return Err("P027: invalid secondary origin epsilon".to_string());
    }
    Ok(position.add(
        normal
            .normalize()
            .map_err(|_| "P027: invalid certified normal".to_string())?
            .scale(epsilon),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(id: u32, z: f64) -> SurfaceObject {
        SurfaceObject {
            object_id: id,
            feature_first: id,
            feature_count: 1,
            bounds: Aabb {
                min: Vec3 {
                    x: -1.0,
                    y: -1.0,
                    z,
                },
                max: Vec3 {
                    x: 1.0,
                    y: 1.0,
                    z: z + 0.1,
                },
            },
        }
    }

    #[test]
    fn bvh_candidates_match_brute_bounds_and_are_sorted() {
        let bvh = SurfaceBvh::build(vec![object(7, 4.0), object(2, 2.0), object(5, 20.0)]).unwrap();
        let query = SegmentQuery {
            origin: Vec3::default(),
            direction: Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            t_min: 0.01,
            t_max: 10.0,
            exclude_feature: 0,
            exclusion_corridor_max: 0.02,
        };
        let mut stack = vec![0; bvh.stack_capacity];
        let mut candidates = Vec::new();
        bvh.candidates(query, &mut stack, &mut candidates).unwrap();
        assert_eq!(candidates, vec![2, 7]);
    }

    #[test]
    fn wire_format_marks_internal_nodes_and_preserves_both_subtrees() {
        let bvh = SurfaceBvh::build(vec![
            object(7, 4.0),
            object(2, 2.0),
            object(5, 20.0),
            object(9, 6.0),
        ])
        .unwrap();
        let (_, nodes, root) = bvh.wire_parts();
        assert!(nodes[root].count > 2);
        assert!(nodes[root].left.is_some() && nodes[root].right.is_some());
        let query = SegmentQuery {
            origin: Vec3::default(),
            direction: Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            t_min: 0.0,
            t_max: 30.0,
            exclude_feature: u32::MAX,
            exclusion_corridor_max: 0.0,
        };
        let mut candidates = Vec::new();
        let mut stack = vec![0; bvh.stack_capacity];
        bvh.candidates(query, &mut stack, &mut candidates).unwrap();
        assert_eq!(candidates, vec![2, 5, 7, 9]);
    }

    #[test]
    fn exclusion_applies_only_to_exact_feature_inside_corridor() {
        let mut source = object(2, 0.0);
        source.feature_first = 9;
        source.feature_count = 2;
        let bvh = SurfaceBvh::build(vec![source]).unwrap();
        let query = SegmentQuery {
            origin: Vec3::default(),
            direction: Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            t_min: 0.0,
            t_max: 2.0,
            exclude_feature: 9,
            exclusion_corridor_max: 0.02,
        };
        let mut stack = vec![0; bvh.stack_capacity];
        let mut candidates = Vec::new();
        let result = bvh.visibility(query, &mut stack, &mut candidates, |_| {
            Ok(vec![
                (F64Interval::new(0.0, 0.01).unwrap(), 9, 1),
                (F64Interval::new(0.015, 0.016).unwrap(), 10, 2),
            ])
        });
        assert!(matches!(
            result,
            SegmentVisibility::Blocked {
                identity_set: 2,
                ..
            }
        ));
    }

    #[test]
    fn returned_root_is_clipped_to_the_query_and_exclusion_corridor() {
        let mut source = object(2, 0.0);
        source.feature_first = 9;
        source.bounds.min.z = 0.1;
        source.bounds.max.z = 0.5;
        let bvh = SurfaceBvh::build(vec![source]).unwrap();
        let query = SegmentQuery {
            origin: Vec3::default(),
            direction: Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            t_min: 0.25,
            t_max: 0.75,
            exclude_feature: 9,
            exclusion_corridor_max: 0.4,
        };
        let mut stack = vec![0; bvh.stack_capacity];
        let mut candidates = Vec::new();
        let result = bvh.visibility(query, &mut stack, &mut candidates, |_| {
            Ok(vec![(F64Interval::new(0.1, 0.5).unwrap(), 9, 3)])
        });
        assert_eq!(
            result,
            SegmentVisibility::Blocked {
                first_t: F64Interval::new(0.4, 0.5).unwrap(),
                identity_set: 3,
            }
        );
    }

    #[test]
    fn blocker_touching_the_light_endpoint_is_not_dropped() {
        let mut source = object(2, 0.0);
        source.bounds.min.z = 0.74;
        source.bounds.max.z = 0.75;
        let bvh = SurfaceBvh::build(vec![source]).unwrap();
        let query = SegmentQuery {
            origin: Vec3::default(),
            direction: Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            t_min: 0.0,
            t_max: 0.75,
            exclude_feature: u32::MAX,
            exclusion_corridor_max: 0.0,
        };
        let mut stack = vec![0; bvh.stack_capacity];
        let mut candidates = Vec::new();
        let root = F64Interval::new(0.7499, 0.75).unwrap();
        assert_eq!(
            bvh.visibility(query, &mut stack, &mut candidates, |_| {
                Ok(vec![(root, 2, 4)])
            }),
            SegmentVisibility::Blocked {
                first_t: root,
                identity_set: 4,
            }
        );
    }

    #[test]
    fn roots_outside_the_compiled_feature_range_are_unresolved() {
        let bvh = SurfaceBvh::build(vec![object(2, 0.0)]).unwrap();
        let query = SegmentQuery {
            origin: Vec3::default(),
            direction: Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            t_min: 0.0,
            t_max: 2.0,
            exclude_feature: 0,
            exclusion_corridor_max: 0.0,
        };
        let mut stack = vec![0; bvh.stack_capacity];
        let mut candidates = Vec::new();
        assert_eq!(
            bvh.visibility(query, &mut stack, &mut candidates, |_| Ok(vec![(
                F64Interval::new(0.5, 0.6).unwrap(),
                99,
                1
            ),])),
            SegmentVisibility::Unresolved("feature-range")
        );
    }

    #[test]
    fn overlapping_feature_ranges_fail_before_source_exclusion() {
        let mut first = object(2, 0.0);
        first.feature_first = 10;
        first.feature_count = 2;
        let mut second = object(3, 2.0);
        second.feature_first = 11;
        second.feature_count = 2;
        assert_eq!(
            SurfaceBvh::build(vec![first, second]),
            Err("P027: overlapping secondary feature ranges".to_string()),
        );
    }

    #[test]
    fn zero_direction_and_under_capacity_queries_fail_closed() {
        let bvh = SurfaceBvh::build(vec![object(2, 0.0)]).unwrap();
        let query = SegmentQuery {
            origin: Vec3::default(),
            direction: Vec3::default(),
            t_min: 0.0,
            t_max: 1.0,
            exclude_feature: 0,
            exclusion_corridor_max: 0.0,
        };
        assert!(bvh.candidates(query, &mut [0; 8], &mut Vec::new()).is_err());
    }

    #[test]
    fn certified_origin_offset_scales_linearly_without_cross_axis_drift() {
        for scale in [1.0e-6, 1.0, 1.0e6] {
            let position = Vec3 {
                x: scale,
                y: 0.0,
                z: -scale,
            };
            let shifted = offset_origin(
                position,
                Vec3 {
                    x: 0.0,
                    y: 1.0,
                    z: 0.0,
                },
                scale * 1.0e-5,
            )
            .unwrap();
            assert_eq!((shifted.x, shifted.z), (position.x, position.z));
            assert!(shifted.y > position.y);
        }
    }
}
