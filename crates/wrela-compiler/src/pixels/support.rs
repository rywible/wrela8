//! Smooth-CSG support-shell propagation.

use std::collections::BTreeMap;

use super::bounds::ValueBounds;
use super::graph::FieldKind;
use super::ids::FieldId;
use super::reference::interval::next_up_f32;
use super::reference::interval::{F64Interval, next_down, next_up};
use super::symbolic::SymbolicGraph;

#[derive(Clone, Debug, PartialEq)]
pub struct GapBudgetProgram {
    pub left: FieldId,
    pub right: FieldId,
    pub left_negated: bool,
    pub right_negated: bool,
    pub k: F64Interval,
    pub gap_lower: f64,
    pub bulge_upper: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LeafSupport {
    /// Primitive-to-root path. Keeping occurrences rather than only FieldId
    /// is essential because the canonical field graph is a DAG.
    pub path: Vec<OccurrenceStep>,
    pub smooth_budget: F64Interval,
    pub deformation_expand: F64Interval,
    /// Conservative geometric distance represented by one scalar value unit
    /// at this point in the authored field tree.
    pub value_to_distance: F64Interval,
}

impl LeafSupport {
    pub fn leaf(&self) -> FieldId {
        self.path[0].field
    }

    pub fn total_expand(&self) -> Result<F64Interval, String> {
        if self.smooth_budget.lo == 0.0
            && self.smooth_budget.hi == 0.0
            && self.deformation_expand.lo == 0.0
            && self.deformation_expand.hi == 0.0
        {
            F64Interval::point(0.0)
        } else {
            self.smooth_budget.add_outward(self.deformation_expand)
        }
    }
}

/// One node in a primitive-to-root DAG occurrence. `child_slot` is the edge
/// used to enter this node (0 for unary/left, 1 for right). It prevents CSE
/// from conflating two authored uses of the same child.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OccurrenceStep {
    pub field: FieldId,
    pub child_slot: u8,
}

impl std::fmt::Display for OccurrenceStep {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}@{}", self.field, self.child_slot)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SupportInfo {
    pub max_budget: F64Interval,
    pub leaf_supports: Vec<LeafSupport>,
    pub max_value_to_distance: F64Interval,
    pub gap_sensitive: Option<GapBudgetProgram>,
    pub maximum_path: Vec<FieldId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SupportTable {
    pub fields: BTreeMap<FieldId, SupportInfo>,
}

impl SupportTable {
    pub fn get(&self, id: FieldId) -> Result<&SupportInfo, String> {
        self.fields
            .get(&id)
            .ok_or_else(|| format!("pixels::support: child budget missing for {id}"))
    }
}

fn zero() -> Result<F64Interval, String> {
    F64Interval::point(0.0)
}

fn merged_occurrences(a: &[LeafSupport], b: &[LeafSupport], parent: FieldId) -> Vec<LeafSupport> {
    a.iter()
        .cloned()
        .map(|mut leaf| {
            leaf.path.push(OccurrenceStep {
                field: parent,
                child_slot: 0,
            });
            leaf
        })
        .chain(b.iter().cloned().map(|mut leaf| {
            leaf.path.push(OccurrenceStep {
                field: parent,
                child_slot: 1,
            });
            leaf
        }))
        .collect()
}

fn add_budget(leaves: &[LeafSupport], budget: F64Interval) -> Result<Vec<LeafSupport>, String> {
    leaves
        .iter()
        .map(|leaf| {
            let addition = budget.mul_outward(leaf.value_to_distance)?;
            let mut leaf = leaf.clone();
            leaf.smooth_budget = leaf.smooth_budget.add_outward(addition)?;
            Ok(leaf)
        })
        .collect()
}

/// Conservative scalar-value support added by one source-f32 smooth-min/max
/// node. `k/4` is the real-arithmetic bulge; the second term encloses every
/// rounded operation in the pinned source implementation at the magnitude
/// reachable from the descendant support shell.
pub(crate) fn smooth_source_support_radius(
    k: F64Interval,
    descendant_budget: f64,
) -> Result<F64Interval, String> {
    const SMOOTH_SOURCE_F32_ULPS_V1: f64 = 128.0;
    if k.lo <= 0.0 || !descendant_budget.is_finite() || descendant_budget < 0.0 {
        return Err(format!(
            "pixels::support: invalid rounded smooth support inputs k={k:?} descendant={descendant_budget}"
        ));
    }
    let magnitude = (k.hi + descendant_budget).max(1.0);
    let magnitude_f32 = magnitude as f32;
    if !magnitude_f32.is_finite() {
        return Err("pixels::support: rounded smooth support magnitude exceeds f32".to_string());
    }
    let ulp = f64::from(next_up_f32(magnitude_f32)) - f64::from(magnitude_f32);
    let slack = next_up(ulp.max(f64::from(f32::from_bits(1))) * SMOOTH_SOURCE_F32_ULPS_V1);
    F64Interval::new(next_down(k.lo / 4.0), next_up(k.hi / 4.0 + slack))
}

fn max_budget(leaves: &[LeafSupport]) -> Result<F64Interval, String> {
    if leaves.is_empty() {
        return zero();
    }
    F64Interval::new(
        leaves
            .iter()
            .map(|leaf| leaf.smooth_budget.lo)
            .fold(f64::NEG_INFINITY, f64::max),
        leaves
            .iter()
            .map(|leaf| leaf.smooth_budget.hi)
            .fold(f64::NEG_INFINITY, f64::max),
    )
}

fn max_value_to_distance(leaves: &[LeafSupport]) -> Result<F64Interval, String> {
    if leaves.is_empty() {
        return F64Interval::point(1.0);
    }
    F64Interval::new(
        leaves
            .iter()
            .map(|leaf| leaf.value_to_distance.lo)
            .fold(f64::NEG_INFINITY, f64::max),
        leaves
            .iter()
            .map(|leaf| leaf.value_to_distance.hi)
            .fold(f64::NEG_INFINITY, f64::max),
    )
}

fn append_path(mut info: SupportInfo, id: FieldId) -> SupportInfo {
    for leaf in &mut info.leaf_supports {
        leaf.path.push(OccurrenceStep {
            field: id,
            child_slot: 0,
        });
    }
    info
}

fn gap_program(
    left: FieldId,
    right: FieldId,
    left_negated: bool,
    right_negated: bool,
    left_value: F64Interval,
    right_value: F64Interval,
    k: F64Interval,
) -> Result<Option<GapBudgetProgram>, String> {
    let gap_lower = if left_value.hi < right_value.lo {
        right_value.lo - left_value.hi
    } else if right_value.hi < left_value.lo {
        left_value.lo - right_value.hi
    } else {
        0.0
    };
    if gap_lower <= 0.0 || gap_lower >= k.hi {
        return Ok(None);
    }
    // For fixed positive gap g, (k-g)^2/(4k) is monotone on k >= g,
    // so the upper endpoint controls both numerator and denominator.
    let bulge_upper = next_up((k.hi - gap_lower).powi(2) / (4.0 * k.hi));
    Ok(Some(GapBudgetProgram {
        left,
        right,
        left_negated,
        right_negated,
        k,
        gap_lower: next_down(gap_lower),
        bulge_upper,
    }))
}

pub fn propagate(graph: &SymbolicGraph, values: &ValueBounds) -> Result<SupportTable, String> {
    let mut table = SupportTable {
        fields: BTreeMap::new(),
    };
    for (id, node) in graph.fields.iter() {
        let child = |id| table.get(id);
        let info = match &node.kind {
            FieldKind::Primitive(_) => SupportInfo {
                max_budget: zero()?,
                leaf_supports: vec![LeafSupport {
                    path: vec![OccurrenceStep {
                        field: id,
                        child_slot: 0,
                    }],
                    smooth_budget: zero()?,
                    deformation_expand: zero()?,
                    value_to_distance: F64Interval::point(1.0)?,
                }],
                max_value_to_distance: F64Interval::point(1.0)?,
                gap_sensitive: None,
                maximum_path: vec![id],
            },
            FieldKind::SmoothUnion { a, b, k }
            | FieldKind::SmoothIntersection { a, b, k }
            | FieldKind::SmoothSubtract { a, b, k } => {
                let k = values.get(*k)?;
                if k.lo <= 0.0 {
                    return Err(format!(
                        "P011: smooth CSG radius `k` is not strictly positive over its declared range: {k:?}"
                    ));
                }
                let descendant_budget = child(*a)?.max_budget.hi.max(child(*b)?.max_budget.hi);
                let addition = smooth_source_support_radius(k, descendant_budget)?;
                let leaves = add_budget(
                    &merged_occurrences(&child(*a)?.leaf_supports, &child(*b)?.leaf_supports, id),
                    addition,
                )?;
                let a_max = child(*a)?.max_budget.hi;
                let b_max = child(*b)?.max_budget.hi;
                let mut path = if a_max >= b_max {
                    child(*a)?.maximum_path.clone()
                } else {
                    child(*b)?.maximum_path.clone()
                };
                path.push(id);
                let a_value = values.get(graph.fields.get(*a)?.scalar_value)?;
                let b_value = values.get(graph.fields.get(*b)?.scalar_value)?;
                // These are the actual operands passed to source smooth-min.
                // Intersection negates both operands; subtraction negates only
                // its left operand. Joint negation preserves the gap, but the
                // one-sided subtraction negation does not.
                let (left_negated, right_negated, left_value, right_value) = match &node.kind {
                    FieldKind::SmoothUnion { .. } => (false, false, a_value, b_value),
                    FieldKind::SmoothIntersection { .. } => {
                        (true, true, a_value.neg(), b_value.neg())
                    }
                    FieldKind::SmoothSubtract { .. } => (true, false, a_value.neg(), b_value),
                    _ => unreachable!("closed smooth CSG kinds"),
                };
                SupportInfo {
                    max_budget: max_budget(&leaves)?,
                    max_value_to_distance: max_value_to_distance(&leaves)?,
                    leaf_supports: leaves,
                    gap_sensitive: gap_program(
                        *a,
                        *b,
                        left_negated,
                        right_negated,
                        left_value,
                        right_value,
                        k,
                    )?,
                    maximum_path: path,
                }
            }
            FieldKind::HardUnion { a, b }
            | FieldKind::HardIntersection { a, b }
            | FieldKind::HardSubtract { a, b } => {
                let leaves =
                    merged_occurrences(&child(*a)?.leaf_supports, &child(*b)?.leaf_supports, id);
                let maximum_path = if child(*a)?.max_budget.hi >= child(*b)?.max_budget.hi {
                    child(*a)?.maximum_path.clone()
                } else {
                    child(*b)?.maximum_path.clone()
                };
                SupportInfo {
                    max_budget: max_budget(&leaves)?,
                    max_value_to_distance: max_value_to_distance(&leaves)?,
                    leaf_supports: leaves,
                    gap_sensitive: None,
                    maximum_path,
                }
            }
            FieldKind::Transform {
                child: field,
                transform: super::graph::TransformProgram::UniformScale { scale },
            } => {
                let scale = values.get(*scale)?;
                if scale.lo <= 0.0 {
                    return Err(format!(
                        "P004: uniform scale must stay strictly positive, got {scale:?}"
                    ));
                }
                let reciprocal =
                    F64Interval::new(next_down(1.0 / scale.hi), next_up(1.0 / scale.lo))?;
                let mut info = child(*field)?.clone();
                for leaf in &mut info.leaf_supports {
                    leaf.value_to_distance = leaf.value_to_distance.mul_outward(reciprocal)?;
                    leaf.path.push(OccurrenceStep {
                        field: id,
                        child_slot: 0,
                    });
                }
                info.max_value_to_distance = max_value_to_distance(&info.leaf_supports)?;
                info
            }
            FieldKind::Neg { child: field }
            | FieldKind::Transform { child: field, .. }
            | FieldKind::FiniteRepeat { child: field, .. }
            | FieldKind::Mark { child: field, .. } => append_path(child(*field)?.clone(), id),
            FieldKind::BoundedDisplace { base, contract, .. } => {
                let mut info = child(*base)?.clone();
                let amplitude = values.get(contract.amplitude_bound)?.abs_upper();
                for leaf in &mut info.leaf_supports {
                    let expansion = leaf
                        .value_to_distance
                        .mul_outward(F64Interval::point(amplitude)?)?;
                    leaf.deformation_expand = leaf.deformation_expand.add_outward(expansion)?;
                    leaf.path.push(OccurrenceStep {
                        field: id,
                        child_slot: 0,
                    });
                }
                info
            }
        };
        table.fields.insert(id, info);
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_leaf_budget_adds_each_authored_support_radius() {
        let leaves = vec![LeafSupport {
            path: vec![OccurrenceStep {
                field: FieldId(0),
                child_slot: 0,
            }],
            smooth_budget: F64Interval::point(0.0).unwrap(),
            deformation_expand: F64Interval::point(0.0).unwrap(),
            value_to_distance: F64Interval::point(1.0).unwrap(),
        }];
        let first = add_budget(&leaves, F64Interval::point(0.25).unwrap()).unwrap();
        let second = add_budget(&first, F64Interval::point(0.5).unwrap()).unwrap();
        assert!(second[0].smooth_budget.contains(0.75));
    }

    #[test]
    fn gap_sensitive_bulge_never_exceeds_coarse_quarter_k() {
        for k in [0.125, 2.0, 100.0] {
            for gap in [0.0, k / 8.0, k / 2.0, k * 0.999] {
                let bulge = (k - gap) * (k - gap) / (4.0 * k);
                assert!(bulge <= k / 4.0);
            }
        }
    }

    #[test]
    fn smooth_subtract_gap_uses_the_negated_left_operand() {
        let authored_left = F64Interval::new(1.0, 1.2).unwrap();
        let authored_right = F64Interval::new(-0.2, 0.0).unwrap();
        let k = F64Interval::point(2.0).unwrap();
        let gap = gap_program(
            FieldId(0),
            FieldId(1),
            true,
            false,
            authored_left.neg(),
            authored_right,
            k,
        )
        .unwrap()
        .unwrap();

        assert!(gap.left_negated);
        assert!(!gap.right_negated);
        assert!(gap.gap_lower <= 0.8);
        assert!(gap.bulge_upper >= 0.18);
    }

    #[test]
    fn rounded_support_encloses_nested_equal_child_source_counterexample() {
        let k1 = F64Interval::point(f64::from(0.0014886133_f32)).unwrap();
        let k2 = F64Interval::point(f64::from(2.6871166_f32)).unwrap();
        let first = smooth_source_support_radius(k1, 0.0).unwrap();
        let second = smooth_source_support_radius(k2, first.hi).unwrap();
        let budget = first.add_outward(second).unwrap();
        let leaf_value = f64::from(0.6721513_f32);
        assert!(budget.hi >= leaf_value);
        assert!(k1.hi / 4.0 + k2.hi / 4.0 < leaf_value);
    }

    #[test]
    fn subunit_scale_converts_scalar_support_to_geometric_distance() {
        let mut leaf = LeafSupport {
            path: vec![OccurrenceStep {
                field: FieldId(0),
                child_slot: 0,
            }],
            smooth_budget: F64Interval::point(0.0).unwrap(),
            deformation_expand: F64Interval::point(0.0).unwrap(),
            value_to_distance: F64Interval::point(4.0).unwrap(),
        };
        leaf.deformation_expand = F64Interval::point(0.5).unwrap();
        let supported = add_budget(&[leaf], F64Interval::point(0.25).unwrap()).unwrap();
        assert!(supported[0].smooth_budget.contains(1.0));
        assert!(supported[0].total_expand().unwrap().contains(1.5));
    }

    #[test]
    fn shared_dag_children_retain_distinct_occurrence_slots() {
        let leaf = LeafSupport {
            path: vec![OccurrenceStep {
                field: FieldId(0),
                child_slot: 0,
            }],
            smooth_budget: F64Interval::point(0.0).unwrap(),
            deformation_expand: F64Interval::point(0.0).unwrap(),
            value_to_distance: F64Interval::point(1.0).unwrap(),
        };
        let occurrences = merged_occurrences(
            std::slice::from_ref(&leaf),
            std::slice::from_ref(&leaf),
            FieldId(1),
        );
        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].path[1].child_slot, 0);
        assert_eq!(occurrences[1].path[1].child_slot, 1);
    }
}
