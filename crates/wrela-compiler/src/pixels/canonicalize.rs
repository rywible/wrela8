//! Deterministic graph compaction after exact construction-time folding/CSE.

use std::collections::{BTreeMap, BTreeSet};

use super::arena::Arena;
#[cfg(test)]
use super::arena::NodeOrigin;
use super::graph::{FieldKind, FieldNode, Primitive, TransformProgram};
use super::ids::{FieldId, MaterialId, ParamId, ScalarId};
use super::material_graph::{MaterialKind, MaterialNode, NormalModel};
#[cfg(test)]
use super::scalar::SemanticOpId;
use super::scalar::{Dependency, ProofObligation, ScalarNode, ScalarOp};
use super::symbolic::{PendingObligation, SymbolicGraph};

pub fn run(graph: &mut SymbolicGraph) -> Result<(), String> {
    run_once(graph)
}

/// Test-only model of a tempting real-number lattice normalization. It is
/// retained as a regression oracle: authoritative f32 SmoothMin is not
/// distributive, so production canonicalization must never invoke this pass.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
enum HardNormalForm {
    Leaf { field: FieldId, negated: bool },
    And(Box<HardNormalForm>, Box<HardNormalForm>),
    Or(Box<HardNormalForm>, Box<HardNormalForm>),
}

#[cfg(test)]
fn is_plain_leaf(form: &HardNormalForm, field: FieldId) -> bool {
    matches!(
        form,
        HardNormalForm::Leaf {
            field: found,
            negated: false,
        } if *found == field
    )
}

#[cfg(test)]
struct HardNormalizer<'a> {
    graph: &'a mut SymbolicGraph,
    memo: BTreeMap<FieldId, HardNormalForm>,
}

#[cfg(test)]
impl HardNormalizer<'_> {
    fn check_quota(&mut self) -> Result<(), String> {
        let total = self
            .graph
            .scalar
            .len()
            .checked_add(self.graph.fields.len())
            .and_then(|count| count.checked_add(self.graph.materials.len()))
            .ok_or_else(|| "P015: hard-CSG normalization size overflow".to_string())?;
        if total >= self.graph.quota.max_nodes {
            return Err(format!(
                "P015: hard-CSG normalization exceeds the symbolic node ceiling of {}",
                self.graph.quota.max_nodes
            ));
        }
        Ok(())
    }

    fn scalar(
        &mut self,
        op: ScalarOp,
        dependency: Dependency,
        origin: &NodeOrigin,
    ) -> Result<ScalarId, String> {
        self.check_quota()?;
        self.graph
            .scalar
            .push(ScalarNode { op, dependency }, origin.clone())
    }

    fn scalar_dependency(&self, id: ScalarId) -> Result<Dependency, String> {
        Ok(self.graph.scalar.get(id)?.dependency)
    }

    fn unary_scalar(
        &mut self,
        op: impl FnOnce(ScalarId) -> ScalarOp,
        value: ScalarId,
        origin: &NodeOrigin,
    ) -> Result<ScalarId, String> {
        self.scalar(op(value), self.scalar_dependency(value)?, origin)
    }

    fn binary_scalar(
        &mut self,
        op: impl FnOnce(ScalarId, ScalarId) -> ScalarOp,
        a: ScalarId,
        b: ScalarId,
        origin: &NodeOrigin,
    ) -> Result<ScalarId, String> {
        let dependency = self
            .scalar_dependency(a)?
            .combine(self.scalar_dependency(b)?);
        self.scalar(op(a, b), dependency, origin)
    }

    fn smooth_scalar(
        &mut self,
        a: ScalarId,
        b: ScalarId,
        k: ScalarId,
        origin: &NodeOrigin,
    ) -> Result<ScalarId, String> {
        let dependency = self
            .scalar_dependency(a)?
            .combine(self.scalar_dependency(b)?)
            .combine(self.scalar_dependency(k)?);
        self.scalar(
            ScalarOp::SmoothMin {
                a,
                b,
                k,
                semantic: SemanticOpId::SmoothMinF32V1,
            },
            dependency,
            origin,
        )
    }

    fn field(
        &mut self,
        kind: FieldKind,
        scalar_value: ScalarId,
        origin: &NodeOrigin,
    ) -> Result<FieldId, String> {
        self.check_quota()?;
        self.graph
            .fields
            .push(FieldNode { kind, scalar_value }, origin.clone())
    }

    fn negate_leaf(&mut self, child: FieldId, origin: &NodeOrigin) -> Result<FieldId, String> {
        let child_scalar = self.graph.fields.get(child)?.scalar_value;
        let scalar = self.unary_scalar(ScalarOp::Neg, child_scalar, origin)?;
        self.field(FieldKind::Neg { child }, scalar, origin)
    }

    fn negate(
        &mut self,
        form: HardNormalForm,
        origin: &NodeOrigin,
    ) -> Result<HardNormalForm, String> {
        Ok(match form {
            HardNormalForm::Leaf { field, negated } => HardNormalForm::Leaf {
                field,
                negated: !negated,
            },
            HardNormalForm::And(a, b) => HardNormalForm::Or(
                Box::new(self.negate(*a, origin)?),
                Box::new(self.negate(*b, origin)?),
            ),
            HardNormalForm::Or(a, b) => HardNormalForm::And(
                Box::new(self.negate(*a, origin)?),
                Box::new(self.negate(*b, origin)?),
            ),
        })
    }

    fn map_unary(
        &mut self,
        form: HardNormalForm,
        source: &FieldKind,
        origin: &NodeOrigin,
    ) -> Result<HardNormalForm, String> {
        Ok(match form {
            HardNormalForm::Leaf {
                field: child,
                negated,
            } => {
                let child_scalar = self.graph.fields.get(child)?.scalar_value;
                let (kind, scalar) = match source {
                    FieldKind::Transform { transform, .. } => {
                        let scalar = match transform {
                            TransformProgram::UniformScale { scale } => {
                                self.binary_scalar(ScalarOp::Mul, child_scalar, *scale, origin)?
                            }
                            _ => child_scalar,
                        };
                        (
                            FieldKind::Transform {
                                child,
                                transform: transform.clone(),
                            },
                            scalar,
                        )
                    }
                    FieldKind::FiniteRepeat {
                        axis,
                        first,
                        count,
                        period,
                        ..
                    } => (
                        FieldKind::FiniteRepeat {
                            child,
                            axis: *axis,
                            first: *first,
                            count: *count,
                            period: *period,
                        },
                        child_scalar,
                    ),
                    FieldKind::BoundedDisplace {
                        displacement,
                        contract,
                        ..
                    } => {
                        let displacement = if negated {
                            self.unary_scalar(ScalarOp::Neg, *displacement, origin)?
                        } else {
                            *displacement
                        };
                        (
                            FieldKind::BoundedDisplace {
                                base: child,
                                displacement,
                                contract: contract.clone(),
                            },
                            self.binary_scalar(ScalarOp::Add, child_scalar, displacement, origin)?,
                        )
                    }
                    FieldKind::Mark {
                        object_source,
                        material_source,
                        ..
                    } => (
                        FieldKind::Mark {
                            child,
                            object_source: object_source.clone(),
                            material_source: material_source.clone(),
                        },
                        child_scalar,
                    ),
                    _ => {
                        return Err(
                            "pixels::canonicalize: non-unary field passed to hard normalizer"
                                .to_string(),
                        );
                    }
                };
                HardNormalForm::Leaf {
                    field: self.field(kind, scalar, origin)?,
                    negated,
                }
            }
            HardNormalForm::And(a, b) => HardNormalForm::And(
                Box::new(self.map_unary(*a, source, origin)?),
                Box::new(self.map_unary(*b, source, origin)?),
            ),
            HardNormalForm::Or(a, b) => HardNormalForm::Or(
                Box::new(self.map_unary(*a, source, origin)?),
                Box::new(self.map_unary(*b, source, origin)?),
            ),
        })
    }

    fn smooth_min_leaf(
        &mut self,
        a: FieldId,
        a_negated: bool,
        b: FieldId,
        b_negated: bool,
        k: ScalarId,
        origin: &NodeOrigin,
    ) -> Result<HardNormalForm, String> {
        let a_scalar = self.graph.fields.get(a)?.scalar_value;
        let b_scalar = self.graph.fields.get(b)?.scalar_value;
        let (field_kind, scalar, negated) = match (a_negated, b_negated) {
            (false, false) => (
                FieldKind::SmoothUnion { a, b, k },
                self.smooth_scalar(a_scalar, b_scalar, k, origin)?,
                false,
            ),
            (true, true) => {
                let neg_a = self.unary_scalar(ScalarOp::Neg, a_scalar, origin)?;
                let neg_b = self.unary_scalar(ScalarOp::Neg, b_scalar, origin)?;
                let smooth = self.smooth_scalar(neg_a, neg_b, k, origin)?;
                (
                    FieldKind::SmoothIntersection { a, b, k },
                    self.unary_scalar(ScalarOp::Neg, smooth, origin)?,
                    true,
                )
            }
            (true, false) => {
                let neg_a = self.unary_scalar(ScalarOp::Neg, a_scalar, origin)?;
                let smooth = self.smooth_scalar(neg_a, b_scalar, k, origin)?;
                (
                    FieldKind::SmoothSubtract { a, b, k },
                    self.unary_scalar(ScalarOp::Neg, smooth, origin)?,
                    true,
                )
            }
            (false, true) => {
                let neg_b = self.unary_scalar(ScalarOp::Neg, b_scalar, origin)?;
                let smooth = self.smooth_scalar(a_scalar, neg_b, k, origin)?;
                (
                    FieldKind::SmoothSubtract { a: b, b: a, k },
                    self.unary_scalar(ScalarOp::Neg, smooth, origin)?,
                    true,
                )
            }
        };
        Ok(HardNormalForm::Leaf {
            field: self.field(field_kind, scalar, origin)?,
            negated,
        })
    }

    fn smooth_leaf(
        &mut self,
        kind: u8,
        a: FieldId,
        a_negated: bool,
        b: FieldId,
        b_negated: bool,
        k: ScalarId,
        origin: &NodeOrigin,
    ) -> Result<HardNormalForm, String> {
        if kind == 0 {
            self.smooth_min_leaf(a, a_negated, b, b_negated, k, origin)
        } else if kind == 1 {
            let result = self.smooth_min_leaf(a, !a_negated, b, !b_negated, k, origin)?;
            self.negate(result, origin)
        } else {
            Err("pixels::canonicalize: unknown smooth normal form".to_string())
        }
    }

    fn lift_smooth(
        &mut self,
        kind: u8,
        a: HardNormalForm,
        b: HardNormalForm,
        k: ScalarId,
        origin: &NodeOrigin,
    ) -> Result<HardNormalForm, String> {
        Ok(match a {
            HardNormalForm::And(left, right) => HardNormalForm::And(
                Box::new(self.lift_smooth(kind, *left, b.clone(), k, origin)?),
                Box::new(self.lift_smooth(kind, *right, b, k, origin)?),
            ),
            HardNormalForm::Or(left, right) => HardNormalForm::Or(
                Box::new(self.lift_smooth(kind, *left, b.clone(), k, origin)?),
                Box::new(self.lift_smooth(kind, *right, b, k, origin)?),
            ),
            HardNormalForm::Leaf {
                field: a,
                negated: a_negated,
            } => match b {
                HardNormalForm::And(left, right) => HardNormalForm::And(
                    Box::new(self.lift_smooth(
                        kind,
                        HardNormalForm::Leaf {
                            field: a,
                            negated: a_negated,
                        },
                        *left,
                        k,
                        origin,
                    )?),
                    Box::new(self.lift_smooth(
                        kind,
                        HardNormalForm::Leaf {
                            field: a,
                            negated: a_negated,
                        },
                        *right,
                        k,
                        origin,
                    )?),
                ),
                HardNormalForm::Or(left, right) => HardNormalForm::Or(
                    Box::new(self.lift_smooth(
                        kind,
                        HardNormalForm::Leaf {
                            field: a,
                            negated: a_negated,
                        },
                        *left,
                        k,
                        origin,
                    )?),
                    Box::new(self.lift_smooth(
                        kind,
                        HardNormalForm::Leaf {
                            field: a,
                            negated: a_negated,
                        },
                        *right,
                        k,
                        origin,
                    )?),
                ),
                HardNormalForm::Leaf {
                    field: b,
                    negated: b_negated,
                } => self.smooth_leaf(kind, a, a_negated, b, b_negated, k, origin)?,
            },
        })
    }

    fn normalize(&mut self, id: FieldId) -> Result<HardNormalForm, String> {
        if let Some(form) = self.memo.get(&id) {
            return Ok(form.clone());
        }
        let node = self.graph.fields.get(id)?.clone();
        let origin = self.graph.fields.origin(id)?.clone();
        let form = match &node.kind {
            FieldKind::Primitive(_) => HardNormalForm::Leaf {
                field: id,
                negated: false,
            },
            FieldKind::HardUnion { a, b } => {
                HardNormalForm::Or(Box::new(self.normalize(*a)?), Box::new(self.normalize(*b)?))
            }
            FieldKind::HardIntersection { a, b } => {
                HardNormalForm::And(Box::new(self.normalize(*a)?), Box::new(self.normalize(*b)?))
            }
            FieldKind::HardSubtract { a, b } => {
                let right = self.normalize(*b)?;
                HardNormalForm::And(
                    Box::new(self.normalize(*a)?),
                    Box::new(self.negate(right, &origin)?),
                )
            }
            FieldKind::Neg { child } => {
                let child = self.normalize(*child)?;
                self.negate(child, &origin)?
            }
            FieldKind::SmoothUnion { a, b, k } => {
                let a_form = self.normalize(*a)?;
                let b_form = self.normalize(*b)?;
                if is_plain_leaf(&a_form, *a) && is_plain_leaf(&b_form, *b) {
                    HardNormalForm::Leaf {
                        field: id,
                        negated: false,
                    }
                } else {
                    self.lift_smooth(0, a_form, b_form, *k, &origin)?
                }
            }
            FieldKind::SmoothIntersection { a, b, k } => {
                let a_form = self.normalize(*a)?;
                let b_form = self.normalize(*b)?;
                if is_plain_leaf(&a_form, *a) && is_plain_leaf(&b_form, *b) {
                    HardNormalForm::Leaf {
                        field: id,
                        negated: false,
                    }
                } else {
                    self.lift_smooth(1, a_form, b_form, *k, &origin)?
                }
            }
            FieldKind::SmoothSubtract { a, b, k } => {
                let a_form = self.normalize(*a)?;
                let b_form = self.normalize(*b)?;
                if is_plain_leaf(&a_form, *a) && is_plain_leaf(&b_form, *b) {
                    HardNormalForm::Leaf {
                        field: id,
                        negated: false,
                    }
                } else {
                    let b_form = self.negate(b_form, &origin)?;
                    self.lift_smooth(1, a_form, b_form, *k, &origin)?
                }
            }
            FieldKind::Transform { child, .. }
            | FieldKind::FiniteRepeat { child, .. }
            | FieldKind::Mark { child, .. } => {
                let child_form = self.normalize(*child)?;
                if is_plain_leaf(&child_form, *child) {
                    HardNormalForm::Leaf {
                        field: id,
                        negated: false,
                    }
                } else {
                    self.map_unary(child_form, &node.kind, &origin)?
                }
            }
            FieldKind::BoundedDisplace { base, .. } => {
                let base_form = self.normalize(*base)?;
                if is_plain_leaf(&base_form, *base) {
                    HardNormalForm::Leaf {
                        field: id,
                        negated: false,
                    }
                } else {
                    self.map_unary(base_form, &node.kind, &origin)?
                }
            }
        };
        self.memo.insert(id, form.clone());
        Ok(form)
    }

    fn reify(&mut self, form: HardNormalForm, origin: &NodeOrigin) -> Result<FieldId, String> {
        match form {
            HardNormalForm::Leaf { field, negated } => {
                if negated {
                    self.negate_leaf(field, origin)
                } else {
                    Ok(field)
                }
            }
            HardNormalForm::And(a, b) => {
                let a = self.reify(*a, origin)?;
                let b = self.reify(*b, origin)?;
                let scalar = self.binary_scalar(
                    ScalarOp::Max,
                    self.graph.fields.get(a)?.scalar_value,
                    self.graph.fields.get(b)?.scalar_value,
                    origin,
                )?;
                self.field(FieldKind::HardIntersection { a, b }, scalar, origin)
            }
            HardNormalForm::Or(a, b) => {
                let a = self.reify(*a, origin)?;
                let b = self.reify(*b, origin)?;
                let scalar = self.binary_scalar(
                    ScalarOp::Min,
                    self.graph.fields.get(a)?.scalar_value,
                    self.graph.fields.get(b)?.scalar_value,
                    origin,
                )?;
                self.field(FieldKind::HardUnion { a, b }, scalar, origin)
            }
        }
    }
}

#[cfg(test)]
fn normalize_hard_boundaries(graph: &mut SymbolicGraph) -> Result<(), String> {
    let original_root = graph.field_root;
    let root_origin = graph.fields.origin(original_root)?.clone();
    let mut normalizer = HardNormalizer {
        graph,
        memo: BTreeMap::new(),
    };
    let form = normalizer.normalize(original_root)?;
    normalizer.graph.field_root = normalizer.reify(form, &root_origin)?;
    Ok(())
}

fn run_once(graph: &mut SymbolicGraph) -> Result<(), String> {
    verify_well_formed(graph, false)?;

    let field_order = postorder(graph.field_root, |id| {
        Ok(field_children(&graph.fields.get(id)?.kind))
    })?;
    let material_order = postorder(graph.material_root, |id| {
        Ok(material_children(&graph.materials.get(id)?.kind))
    })?;

    let mut scalar_seeds = Vec::new();
    for id in &field_order {
        field_scalars(&graph.fields.get(*id)?.kind, &mut scalar_seeds);
        scalar_seeds.push(graph.fields.get(*id)?.scalar_value);
    }
    for id in &material_order {
        material_scalars(&graph.materials.get(*id)?.kind, &mut scalar_seeds);
    }
    for obligation in &graph.obligations {
        match obligation {
            PendingObligation::Scalar(ProofObligation::DenominatorNonZero { denominator }) => {
                scalar_seeds.push(*denominator);
            }
            PendingObligation::Scalar(ProofObligation::GuardedDenominatorNonZero {
                denominator,
                predicate,
            }) => {
                scalar_seeds.extend([*denominator, *predicate]);
            }
            PendingObligation::Scalar(ProofObligation::RestrictedTrigDomain { argument }) => {
                scalar_seeds.push(*argument);
            }
            PendingObligation::Scalar(ProofObligation::DynamicIndexInBounds { index, .. })
            | PendingObligation::MaterialEvent { predicate: index } => {
                scalar_seeds.push(*index);
            }
        }
    }
    let scalar_order = postorder_many(&scalar_seeds, |id| {
        Ok(scalar_children(&graph.scalar.get(id)?.op))
    })?;

    let used_params = scalar_order
        .iter()
        .filter_map(|id| match graph.scalar.get(*id).ok()?.op {
            ScalarOp::Param(param) => Some(param),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut param_map = BTreeMap::new();
    let mut params = Vec::new();
    for record in &graph.params {
        if used_params.contains(&record.id) {
            let id = ParamId(
                u32::try_from(params.len())
                    .map_err(|_| "pixels::canonicalize: parameter ID overflow".to_string())?,
            );
            param_map.insert(record.id, id);
            let mut record = record.clone();
            record.id = id;
            params.push(record);
        }
    }

    let mut scalar_map = BTreeMap::new();
    let mut scalar = Arena::new(graph.scalar.serial());
    let mut scalar_cse = BTreeMap::new();
    for old in scalar_order {
        let node = graph.scalar.get(old)?;
        let remapped = remap_scalar(&node.op, &scalar_map, &param_map)?;
        if let ScalarOp::Min(a, b) | ScalarOp::Max(a, b) = remapped
            && a == b
        {
            scalar.origin_mut(a)?.merge(graph.scalar.origin(old)?);
            scalar_map.insert(old, a);
            continue;
        }
        let op = super::scalar::fold_constant(&scalar, &remapped).unwrap_or(remapped);
        let dependency = if matches!(op, ScalarOp::ConstF32(_) | ScalarOp::ConstF64(_)) {
            Dependency::Constant
        } else {
            node.dependency
        };
        let id = if let Some(id) = scalar_cse.get(&op).copied() {
            scalar.origin_mut(id)?.merge(graph.scalar.origin(old)?);
            id
        } else {
            let id = scalar.push(
                ScalarNode {
                    op: op.clone(),
                    dependency,
                },
                graph.scalar.origin(old)?.clone(),
            )?;
            scalar_cse.insert(op, id);
            id
        };
        scalar_map.insert(old, id);
    }

    let mut field_map = BTreeMap::new();
    let mut fields = Arena::new(graph.fields.serial());
    let mut field_cse = BTreeMap::new();
    for old in field_order {
        let node = graph.fields.get(old)?;
        let kind = remap_field(&node.kind, &field_map, &scalar_map, &scalar)?;
        let scalar_value = mapped(&scalar_map, node.scalar_value, "scalar")?;
        if let FieldKind::Transform { child, transform } = &kind
            && is_identity_transform(transform, &scalar)
        {
            fields.origin_mut(*child)?.merge(graph.fields.origin(old)?);
            field_map.insert(old, *child);
            continue;
        }
        let key = (kind.clone(), scalar_value);
        let id = if let Some(id) = field_cse.get(&key).copied() {
            fields.origin_mut(id)?.merge(graph.fields.origin(old)?);
            id
        } else {
            let id = fields.push(
                FieldNode { kind, scalar_value },
                graph.fields.origin(old)?.clone(),
            )?;
            field_cse.insert(key, id);
            id
        };
        field_map.insert(old, id);
    }

    let mut material_map = BTreeMap::new();
    let mut materials = Arena::new(graph.materials.serial());
    let mut material_cse = BTreeMap::new();
    for old in material_order {
        let node = graph.materials.get(old)?;
        let kind = remap_material(&node.kind, &material_map, &scalar_map)?;
        if let MaterialKind::Select { a, b, .. } = &kind
            && a == b
        {
            materials
                .origin_mut(*a)?
                .merge(graph.materials.origin(old)?);
            material_map.insert(old, *a);
            continue;
        }
        let id = if let Some(id) = material_cse.get(&kind).copied() {
            materials
                .origin_mut(id)?
                .merge(graph.materials.origin(old)?);
            id
        } else {
            let id = materials.push(
                MaterialNode { kind: kind.clone() },
                graph.materials.origin(old)?.clone(),
            )?;
            material_cse.insert(kind, id);
            id
        };
        material_map.insert(old, id);
    }
    let active_material_predicates = materials
        .iter()
        .filter_map(|(_, node)| match &node.kind {
            MaterialKind::Select { predicate, .. } => Some(*predicate),
            _ => None,
        })
        .collect::<BTreeSet<_>>();

    let mut obligations = Vec::new();
    let mut obligation_keys = BTreeSet::new();
    for obligation in &graph.obligations {
        let remapped = match obligation {
            PendingObligation::Scalar(ProofObligation::DenominatorNonZero { denominator }) => {
                scalar_map.get(denominator).copied().map(|denominator| {
                    PendingObligation::Scalar(ProofObligation::DenominatorNonZero { denominator })
                })
            }
            PendingObligation::Scalar(ProofObligation::GuardedDenominatorNonZero {
                denominator,
                predicate,
            }) => scalar_map
                .get(denominator)
                .copied()
                .zip(scalar_map.get(predicate).copied())
                .map(|(denominator, predicate)| {
                    PendingObligation::Scalar(ProofObligation::GuardedDenominatorNonZero {
                        denominator,
                        predicate,
                    })
                }),
            PendingObligation::Scalar(ProofObligation::RestrictedTrigDomain { argument }) => {
                scalar_map.get(argument).copied().map(|argument| {
                    PendingObligation::Scalar(ProofObligation::RestrictedTrigDomain { argument })
                })
            }
            PendingObligation::Scalar(ProofObligation::DynamicIndexInBounds { index, extent }) => {
                scalar_map.get(index).copied().map(|index| {
                    PendingObligation::Scalar(ProofObligation::DynamicIndexInBounds {
                        index,
                        extent: *extent,
                    })
                })
            }
            PendingObligation::MaterialEvent { predicate } => scalar_map
                .get(predicate)
                .copied()
                .filter(|predicate| active_material_predicates.contains(predicate))
                .map(|predicate| PendingObligation::MaterialEvent { predicate }),
        };
        if let Some(remapped) = remapped
            && obligation_keys.insert(remapped.clone())
        {
            obligations.push(remapped);
        }
    }

    graph.field_root = mapped(&field_map, graph.field_root, "field")?;
    graph.material_root = mapped(&material_map, graph.material_root, "material")?;
    graph.params = params;
    graph.scalar = scalar;
    graph.fields = fields;
    graph.materials = materials;
    graph.obligations = obligations;
    verify_well_formed(graph, true)
}

fn verify_well_formed(graph: &SymbolicGraph, require_unique: bool) -> Result<(), String> {
    let mut scalar = BTreeSet::new();
    for (id, node) in graph.scalar.iter() {
        if require_unique && !scalar.insert(node.op.clone()) {
            return Err(format!(
                "pixels::canonicalize: duplicate scalar structural key at {id}"
            ));
        }
        for child in scalar_children(&node.op) {
            if child.0 >= id.0 {
                return Err(format!(
                    "pixels::canonicalize: scalar child {child} is not before parent {id}"
                ));
            }
        }
        graph.scalar.origin(id)?;
    }
    let mut fields = BTreeSet::new();
    for (id, node) in graph.fields.iter() {
        if require_unique && !fields.insert((node.kind.clone(), node.scalar_value)) {
            return Err(format!(
                "pixels::canonicalize: duplicate field structural key at {id}"
            ));
        }
        for child in field_children(&node.kind) {
            if child.0 >= id.0 {
                return Err(format!(
                    "pixels::canonicalize: field child {child} is not before parent {id}"
                ));
            }
        }
        graph.fields.origin(id)?;
    }
    let mut materials = BTreeSet::new();
    for (id, node) in graph.materials.iter() {
        if require_unique && !materials.insert(node.kind.clone()) {
            return Err(format!(
                "pixels::canonicalize: duplicate material structural key at {id}"
            ));
        }
        for child in material_children(&node.kind) {
            if child.0 >= id.0 {
                return Err(format!(
                    "pixels::canonicalize: material child {child} is not before parent {id}"
                ));
            }
        }
        graph.materials.origin(id)?;
    }
    graph.fields.get(graph.field_root)?;
    graph.materials.get(graph.material_root)?;
    Ok(())
}

fn is_identity_transform(
    transform: &TransformProgram,
    scalar: &Arena<ScalarId, ScalarNode>,
) -> bool {
    let is = |id: ScalarId, expected: f32| {
        super::scalar::constant_value(scalar, id)
            .is_some_and(|value| value.to_bits() == expected.to_bits())
    };
    let zero = |values: &[ScalarId; 3]| values.iter().all(|id| is(*id, 0.0));
    let rows = |row_x: &[ScalarId; 3], row_y: &[ScalarId; 3], row_z: &[ScalarId; 3]| {
        row_x
            .iter()
            .chain(row_y)
            .chain(row_z)
            .copied()
            .zip([1.0_f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
            .all(|(id, expected)| is(id, expected))
    };
    match transform {
        TransformProgram::Translate { by } => zero(by),
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => rows(row_x, row_y, row_z),
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => zero(translation) && rows(row_x, row_y, row_z),
        TransformProgram::UniformScale { scale } => is(*scale, 1.0),
        // A sequence is evaluated step by step. Even when its geometric
        // composition is identity, intermediate f32 rounding can be visible.
        TransformProgram::SourceRigidSequence { .. } | TransformProgram::RigidSequence { .. } => {
            false
        }
    }
}

fn postorder<I, F>(root: I, children: F) -> Result<Vec<I>, String>
where
    I: Copy + Ord,
    F: Fn(I) -> Result<Vec<I>, String>,
{
    postorder_many(&[root], children)
}

fn postorder_many<I, F>(roots: &[I], children: F) -> Result<Vec<I>, String>
where
    I: Copy + Ord,
    F: Fn(I) -> Result<Vec<I>, String>,
{
    let mut complete = BTreeSet::new();
    let mut order = Vec::new();
    let mut stack = roots
        .iter()
        .rev()
        .copied()
        .map(|id| (id, false))
        .collect::<Vec<_>>();
    while let Some((id, expanded)) = stack.pop() {
        if complete.contains(&id) {
            continue;
        }
        if expanded {
            complete.insert(id);
            order.push(id);
            continue;
        }
        stack.push((id, true));
        let children = children(id)?;
        for child in children.into_iter().rev() {
            if !complete.contains(&child) {
                stack.push((child, false));
            }
        }
    }
    Ok(order)
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

fn material_children(kind: &MaterialKind) -> Vec<MaterialId> {
    match kind {
        MaterialKind::Sample(_) => Vec::new(),
        MaterialKind::Select { a, b, .. } => vec![*a, *b],
        MaterialKind::IdentityTable { cases, .. } => {
            cases.iter().map(|(_, material)| *material).collect()
        }
    }
}

fn field_scalars(kind: &FieldKind, out: &mut Vec<ScalarId>) {
    match kind {
        FieldKind::Primitive(primitive) => primitive_scalars(primitive, out),
        FieldKind::SmoothUnion { k, .. }
        | FieldKind::SmoothIntersection { k, .. }
        | FieldKind::SmoothSubtract { k, .. } => out.push(*k),
        FieldKind::Transform { transform, .. } => transform_scalars(transform, out),
        FieldKind::FiniteRepeat { period, .. } => out.push(*period),
        FieldKind::BoundedDisplace {
            displacement,
            contract,
            ..
        } => {
            out.extend([
                *displacement,
                contract.amplitude_bound,
                contract.gradient_bound,
                contract.hessian_bound,
                contract.third_derivative_bound,
            ]);
        }
        FieldKind::HardUnion { .. }
        | FieldKind::HardIntersection { .. }
        | FieldKind::HardSubtract { .. }
        | FieldKind::Neg { .. }
        | FieldKind::Mark { .. } => {}
    }
}

fn primitive_scalars(primitive: &Primitive, out: &mut Vec<ScalarId>) {
    match primitive {
        Primitive::Plane { normal, offset } => {
            out.extend(*normal);
            out.push(*offset);
        }
        Primitive::Sphere { center, radius }
        | Primitive::Capsule {
            a: center, radius, ..
        }
        | Primitive::FiniteCylinder {
            a: center, radius, ..
        } => {
            out.extend(*center);
            if let Primitive::Capsule { b, .. } | Primitive::FiniteCylinder { b, .. } = primitive {
                out.extend(*b);
            }
            out.push(*radius);
        }
        Primitive::Box { center, half } => {
            out.extend(*center);
            out.extend(*half);
        }
        Primitive::RoundBox {
            center,
            half,
            radius,
        } => {
            out.extend(*center);
            out.extend(*half);
            out.push(*radius);
        }
        Primitive::FiniteCone {
            a,
            b,
            radius_a,
            radius_b,
        } => {
            out.extend(*a);
            out.extend(*b);
            out.extend([*radius_a, *radius_b]);
        }
        Primitive::Torus {
            center,
            axis,
            major,
            minor,
        } => {
            out.extend(*center);
            out.extend(*axis);
            out.extend([*major, *minor]);
        }
    }
}

fn transform_scalars(transform: &TransformProgram, out: &mut Vec<ScalarId>) {
    match transform {
        TransformProgram::Translate { by } => out.extend(*by),
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => {
            out.extend(*row_x);
            out.extend(*row_y);
            out.extend(*row_z);
        }
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => {
            out.extend(*translation);
            out.extend(*row_x);
            out.extend(*row_y);
            out.extend(*row_z);
        }
        TransformProgram::UniformScale { scale } => out.push(*scale),
        TransformProgram::SourceRigidSequence { steps, composed }
        | TransformProgram::RigidSequence { steps, composed } => {
            for step in steps {
                transform_scalars(step, out);
            }
            transform_scalars(composed, out);
        }
    }
}

fn material_scalars(kind: &MaterialKind, out: &mut Vec<ScalarId>) {
    match kind {
        MaterialKind::Sample(sample) => {
            out.extend(sample.base_color);
            out.push(sample.opacity);
            out.extend(sample.emissive);
            out.extend([
                sample.roughness,
                sample.metallic,
                sample.specular_level,
                sample.ior,
            ]);
            if let NormalModel::AnalyticSlope { x, y } = sample.normal {
                out.extend([x, y]);
            }
        }
        MaterialKind::Select { predicate, .. } => out.push(*predicate),
        MaterialKind::IdentityTable { .. } => {}
    }
}

fn scalar_children(op: &ScalarOp) -> Vec<ScalarId> {
    match op {
        ScalarOp::ConstF32(_)
        | ScalarOp::ConstF64(_)
        | ScalarOp::CoordX
        | ScalarOp::CoordY
        | ScalarOp::CoordZ
        | ScalarOp::SurfacePosition(_)
        | ScalarOp::SurfaceNormal(_)
        | ScalarOp::Param(_) => Vec::new(),
        ScalarOp::Neg(value)
        | ScalarOp::Abs(value)
        | ScalarOp::Sqrt(value, _)
        | ScalarOp::Rsqrt(value, _)
        | ScalarOp::SinRestricted(value, _)
        | ScalarOp::CosRestricted(value, _)
        | ScalarOp::MaterialRoughness { value, .. } => vec![*value],
        ScalarOp::Add(a, b)
        | ScalarOp::Sub(a, b)
        | ScalarOp::Mul(a, b)
        | ScalarOp::Div(a, b)
        | ScalarOp::Min(a, b)
        | ScalarOp::Max(a, b)
        | ScalarOp::Compare { a, b, .. }
        | ScalarOp::FiniteOr {
            value: a,
            fallback: b,
            ..
        } => vec![*a, *b],
        ScalarOp::Clamp { value, lo, hi } => vec![*value, *lo, *hi],
        ScalarOp::Dot3(a, b) => a.iter().chain(b).copied().collect(),
        ScalarOp::Cross3Component { a, b, .. } => a.iter().chain(b).copied().collect(),
        ScalarOp::Length2(value) => value.to_vec(),
        ScalarOp::Length3(value) | ScalarOp::Normalize3Component { value, .. } => value.to_vec(),
        ScalarOp::Select { predicate, a, b } => vec![*predicate, *a, *b],
        ScalarOp::SelectIndex { index, options } => std::iter::once(*index)
            .chain(options.iter().copied())
            .collect(),
        ScalarOp::SmoothMin { a, b, k, .. } => vec![*a, *b, *k],
    }
}

fn mapped<I: Copy + Ord>(map: &BTreeMap<I, I>, old: I, kind: &str) -> Result<I, String> {
    map.get(&old)
        .copied()
        .ok_or_else(|| format!("pixels::canonicalize: missing {kind} remap"))
}

fn map_scalar_array<const N: usize>(
    map: &BTreeMap<ScalarId, ScalarId>,
    values: [ScalarId; N],
) -> Result<[ScalarId; N], String> {
    let values = values
        .into_iter()
        .map(|value| mapped(map, value, "scalar"))
        .collect::<Result<Vec<_>, _>>()?;
    values
        .try_into()
        .map_err(|_| "pixels::canonicalize: scalar array arity changed".to_string())
}

fn remap_scalar(
    op: &ScalarOp,
    scalar: &BTreeMap<ScalarId, ScalarId>,
    params: &BTreeMap<ParamId, ParamId>,
) -> Result<ScalarOp, String> {
    let one = |id| mapped(scalar, id, "scalar");
    let pair = |a, b| Ok::<_, String>((one(a)?, one(b)?));
    Ok(match op {
        ScalarOp::ConstF32(bits) => ScalarOp::ConstF32(*bits),
        ScalarOp::ConstF64(bits) => ScalarOp::ConstF64(*bits),
        ScalarOp::CoordX => ScalarOp::CoordX,
        ScalarOp::CoordY => ScalarOp::CoordY,
        ScalarOp::CoordZ => ScalarOp::CoordZ,
        ScalarOp::SurfacePosition(component) => ScalarOp::SurfacePosition(*component),
        ScalarOp::SurfaceNormal(component) => ScalarOp::SurfaceNormal(*component),
        ScalarOp::Param(param) => ScalarOp::Param(
            params
                .get(param)
                .copied()
                .ok_or_else(|| "pixels::canonicalize: missing parameter remap".to_string())?,
        ),
        ScalarOp::Add(a, b) => {
            let (a, b) = pair(*a, *b)?;
            ScalarOp::Add(a, b)
        }
        ScalarOp::Sub(a, b) => {
            let (a, b) = pair(*a, *b)?;
            ScalarOp::Sub(a, b)
        }
        ScalarOp::Mul(a, b) => {
            let (a, b) = pair(*a, *b)?;
            ScalarOp::Mul(a, b)
        }
        ScalarOp::Div(a, b) => {
            let (a, b) = pair(*a, *b)?;
            ScalarOp::Div(a, b)
        }
        ScalarOp::Neg(value) => ScalarOp::Neg(one(*value)?),
        ScalarOp::Abs(value) => ScalarOp::Abs(one(*value)?),
        ScalarOp::Min(a, b) => {
            let (a, b) = pair(*a, *b)?;
            ScalarOp::Min(a, b)
        }
        ScalarOp::Max(a, b) => {
            let (a, b) = pair(*a, *b)?;
            ScalarOp::Max(a, b)
        }
        ScalarOp::Clamp { value, lo, hi } => ScalarOp::Clamp {
            value: one(*value)?,
            lo: one(*lo)?,
            hi: one(*hi)?,
        },
        ScalarOp::Sqrt(value, semantic) => ScalarOp::Sqrt(one(*value)?, *semantic),
        ScalarOp::Rsqrt(value, semantic) => ScalarOp::Rsqrt(one(*value)?, *semantic),
        ScalarOp::SinRestricted(value, semantic) => {
            ScalarOp::SinRestricted(one(*value)?, *semantic)
        }
        ScalarOp::CosRestricted(value, semantic) => {
            ScalarOp::CosRestricted(one(*value)?, *semantic)
        }
        ScalarOp::Dot3(a, b) => {
            ScalarOp::Dot3(map_scalar_array(scalar, *a)?, map_scalar_array(scalar, *b)?)
        }
        ScalarOp::Cross3Component { component, a, b } => ScalarOp::Cross3Component {
            component: *component,
            a: map_scalar_array(scalar, *a)?,
            b: map_scalar_array(scalar, *b)?,
        },
        ScalarOp::Length2(value) => ScalarOp::Length2(map_scalar_array(scalar, *value)?),
        ScalarOp::Length3(value) => ScalarOp::Length3(map_scalar_array(scalar, *value)?),
        ScalarOp::Normalize3Component {
            component,
            value,
            semantic,
        } => ScalarOp::Normalize3Component {
            component: *component,
            value: map_scalar_array(scalar, *value)?,
            semantic: *semantic,
        },
        ScalarOp::Compare { op, a, b } => ScalarOp::Compare {
            op: *op,
            a: one(*a)?,
            b: one(*b)?,
        },
        ScalarOp::Select { predicate, a, b } => ScalarOp::Select {
            predicate: one(*predicate)?,
            a: one(*a)?,
            b: one(*b)?,
        },
        ScalarOp::SelectIndex { index, options } => ScalarOp::SelectIndex {
            index: one(*index)?,
            options: options
                .iter()
                .map(|option| one(*option))
                .collect::<Result<Vec<_>, _>>()?,
        },
        ScalarOp::SmoothMin { a, b, k, semantic } => ScalarOp::SmoothMin {
            a: one(*a)?,
            b: one(*b)?,
            k: one(*k)?,
            semantic: *semantic,
        },
        ScalarOp::FiniteOr {
            value,
            fallback,
            semantic,
        } => ScalarOp::FiniteOr {
            value: one(*value)?,
            fallback: one(*fallback)?,
            semantic: *semantic,
        },
        ScalarOp::MaterialRoughness { value, semantic } => ScalarOp::MaterialRoughness {
            value: one(*value)?,
            semantic: *semantic,
        },
    })
}

fn remap_primitive(
    primitive: &Primitive,
    scalar: &BTreeMap<ScalarId, ScalarId>,
    scalar_arena: &super::scalar::ScalarArena,
) -> Result<Primitive, String> {
    let one = |id| mapped(scalar, id, "scalar");
    Ok(match primitive {
        Primitive::Plane { normal, offset } => Primitive::Plane {
            normal: map_scalar_array(scalar, *normal)?,
            offset: one(*offset)?,
        },
        Primitive::Sphere { center, radius } => Primitive::Sphere {
            center: map_scalar_array(scalar, *center)?,
            radius: one(*radius)?,
        },
        Primitive::Box { center, half } => Primitive::Box {
            center: map_scalar_array(scalar, *center)?,
            half: map_scalar_array(scalar, *half)?,
        },
        Primitive::RoundBox {
            center,
            half,
            radius,
        } => Primitive::RoundBox {
            center: map_scalar_array(scalar, *center)?,
            half: map_scalar_array(scalar, *half)?,
            radius: one(*radius)?,
        },
        Primitive::Capsule { a, b, radius } => {
            let a = map_scalar_array(scalar, *a)?;
            let b = map_scalar_array(scalar, *b)?;
            let radius = one(*radius)?;
            let same_constant_point = a.iter().zip(&b).all(|(a, b)| {
                super::scalar::constant_value(scalar_arena, *a)
                    .zip(super::scalar::constant_value(scalar_arena, *b))
                    .is_some_and(|(a, b)| a.to_bits() == b.to_bits())
            });
            let same_scalar_point = a.iter().zip(&b).all(|(a, b)| {
                matches!(
                    (scalar_arena.get(*a), scalar_arena.get(*b)),
                    (Ok(a), Ok(b)) if a.op == b.op
                )
            });
            if a == b || same_constant_point || same_scalar_point {
                Primitive::Sphere { center: a, radius }
            } else {
                Primitive::Capsule { a, b, radius }
            }
        }
        Primitive::FiniteCylinder { a, b, radius } => Primitive::FiniteCylinder {
            a: map_scalar_array(scalar, *a)?,
            b: map_scalar_array(scalar, *b)?,
            radius: one(*radius)?,
        },
        Primitive::FiniteCone {
            a,
            b,
            radius_a,
            radius_b,
        } => Primitive::FiniteCone {
            a: map_scalar_array(scalar, *a)?,
            b: map_scalar_array(scalar, *b)?,
            radius_a: one(*radius_a)?,
            radius_b: one(*radius_b)?,
        },
        Primitive::Torus {
            center,
            axis,
            major,
            minor,
        } => Primitive::Torus {
            center: map_scalar_array(scalar, *center)?,
            axis: map_scalar_array(scalar, *axis)?,
            major: one(*major)?,
            minor: one(*minor)?,
        },
    })
}

fn remap_transform(
    transform: &TransformProgram,
    scalar: &BTreeMap<ScalarId, ScalarId>,
) -> Result<TransformProgram, String> {
    let one = |id| mapped(scalar, id, "scalar");
    Ok(match transform {
        TransformProgram::Translate { by } => TransformProgram::Translate {
            by: map_scalar_array(scalar, *by)?,
        },
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => TransformProgram::Rotate {
            row_x: map_scalar_array(scalar, *row_x)?,
            row_y: map_scalar_array(scalar, *row_y)?,
            row_z: map_scalar_array(scalar, *row_z)?,
        },
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => TransformProgram::Rigid {
            translation: map_scalar_array(scalar, *translation)?,
            row_x: map_scalar_array(scalar, *row_x)?,
            row_y: map_scalar_array(scalar, *row_y)?,
            row_z: map_scalar_array(scalar, *row_z)?,
        },
        TransformProgram::UniformScale { scale } => TransformProgram::UniformScale {
            scale: one(*scale)?,
        },
        TransformProgram::SourceRigidSequence { steps, composed }
        | TransformProgram::RigidSequence { steps, composed } => TransformProgram::RigidSequence {
            steps: steps
                .iter()
                .map(|step| remap_transform(step, scalar))
                .collect::<Result<Vec<_>, _>>()?,
            composed: Box::new(remap_transform(composed, scalar)?),
        },
    })
}

fn remap_field(
    kind: &FieldKind,
    field: &BTreeMap<FieldId, FieldId>,
    scalar: &BTreeMap<ScalarId, ScalarId>,
    scalar_arena: &super::scalar::ScalarArena,
) -> Result<FieldKind, String> {
    let f = |id| mapped(field, id, "field");
    let s = |id| mapped(scalar, id, "scalar");
    Ok(match kind {
        FieldKind::Primitive(primitive) => {
            FieldKind::Primitive(remap_primitive(primitive, scalar, scalar_arena)?)
        }
        FieldKind::HardUnion { a, b } => FieldKind::HardUnion {
            a: f(*a)?,
            b: f(*b)?,
        },
        FieldKind::HardIntersection { a, b } => FieldKind::HardIntersection {
            a: f(*a)?,
            b: f(*b)?,
        },
        FieldKind::HardSubtract { a, b } => FieldKind::HardSubtract {
            a: f(*a)?,
            b: f(*b)?,
        },
        FieldKind::SmoothUnion { a, b, k } => FieldKind::SmoothUnion {
            a: f(*a)?,
            b: f(*b)?,
            k: s(*k)?,
        },
        FieldKind::SmoothIntersection { a, b, k } => FieldKind::SmoothIntersection {
            a: f(*a)?,
            b: f(*b)?,
            k: s(*k)?,
        },
        FieldKind::SmoothSubtract { a, b, k } => FieldKind::SmoothSubtract {
            a: f(*a)?,
            b: f(*b)?,
            k: s(*k)?,
        },
        FieldKind::Neg { child } => FieldKind::Neg { child: f(*child)? },
        FieldKind::Transform { child, transform } => FieldKind::Transform {
            child: f(*child)?,
            transform: remap_transform(transform, scalar)?,
        },
        FieldKind::FiniteRepeat {
            child,
            axis,
            first,
            count,
            period,
        } => FieldKind::FiniteRepeat {
            child: f(*child)?,
            axis: *axis,
            first: *first,
            count: *count,
            period: s(*period)?,
        },
        FieldKind::BoundedDisplace {
            base,
            displacement,
            contract,
        } => FieldKind::BoundedDisplace {
            base: f(*base)?,
            displacement: s(*displacement)?,
            contract: super::graph::DerivedDeformContract {
                amplitude_bound: s(contract.amplitude_bound)?,
                gradient_bound: s(contract.gradient_bound)?,
                hessian_bound: s(contract.hessian_bound)?,
                third_derivative_bound: s(contract.third_derivative_bound)?,
                coordinate_x: s(contract.coordinate_x)?,
                frequency: s(contract.frequency)?,
                phase: s(contract.phase)?,
                derivation: contract.derivation,
            },
        },
        FieldKind::Mark {
            child,
            object_source,
            material_source,
        } => FieldKind::Mark {
            child: f(*child)?,
            object_source: object_source.clone(),
            material_source: material_source.clone(),
        },
    })
}

fn remap_material(
    kind: &MaterialKind,
    material: &BTreeMap<MaterialId, MaterialId>,
    scalar: &BTreeMap<ScalarId, ScalarId>,
) -> Result<MaterialKind, String> {
    let m = |id| mapped(material, id, "material");
    let s = |id| mapped(scalar, id, "scalar");
    Ok(match kind {
        MaterialKind::Sample(sample) => {
            MaterialKind::Sample(super::material_graph::MaterialSampleNode {
                base_color: map_scalar_array(scalar, sample.base_color)?,
                opacity: s(sample.opacity)?,
                emissive: map_scalar_array(scalar, sample.emissive)?,
                roughness: s(sample.roughness)?,
                metallic: s(sample.metallic)?,
                specular_level: s(sample.specular_level)?,
                ior: s(sample.ior)?,
                normal: match sample.normal {
                    NormalModel::Geometric => NormalModel::Geometric,
                    NormalModel::AnalyticSlope { x, y } => {
                        NormalModel::AnalyticSlope { x: s(x)?, y: s(y)? }
                    }
                },
                pattern: sample.pattern.clone(),
            })
        }
        MaterialKind::Select { predicate, a, b } => MaterialKind::Select {
            predicate: s(*predicate)?,
            a: m(*a)?,
            b: m(*b)?,
        },
        MaterialKind::IdentityTable { enum_key, cases } => MaterialKind::IdentityTable {
            enum_key: enum_key.clone(),
            cases: cases
                .iter()
                .map(|(identity, value)| Ok((identity.clone(), m(*value)?)))
                .collect::<Result<Vec<_>, String>>()?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::arena::NodeOrigin;
    use crate::pixels::graph::{CanonicalIdentity, FieldArena};
    use crate::pixels::material_graph::{MaterialArena, MaterialSampleNode};
    use crate::pixels::quota::SymbolicQuota;
    use crate::pixels::scalar::{Dependency, ScalarArena, SemanticOpId, source_smooth_min};
    use crate::sema::types::Type;

    fn eval_scalar(arena: &ScalarArena, id: ScalarId, point: [f32; 3]) -> f32 {
        match arena.get(id).unwrap().op {
            ScalarOp::ConstF32(bits) => f32::from_bits(bits),
            ScalarOp::CoordX => point[0],
            ScalarOp::CoordY => point[1],
            ScalarOp::CoordZ => point[2],
            ScalarOp::Add(a, b) => eval_scalar(arena, a, point) + eval_scalar(arena, b, point),
            ScalarOp::Mul(a, b) => eval_scalar(arena, a, point) * eval_scalar(arena, b, point),
            ScalarOp::Neg(value) => -eval_scalar(arena, value, point),
            ScalarOp::Min(a, b) => {
                let a = eval_scalar(arena, a, point);
                let b = eval_scalar(arena, b, point);
                if a < b { a } else { b }
            }
            ScalarOp::Max(a, b) => {
                let a = eval_scalar(arena, a, point);
                let b = eval_scalar(arena, b, point);
                if a > b { a } else { b }
            }
            ScalarOp::SmoothMin { a, b, k, .. } => source_smooth_min(
                eval_scalar(arena, a, point),
                eval_scalar(arena, b, point),
                eval_scalar(arena, k, point),
            ),
            _ => panic!("test evaluator lacks {:?}", arena.get(id).unwrap().op),
        }
    }

    #[test]
    fn canonicalization_uses_typed_structural_keys() {
        let source = include_str!("canonicalize.rs");
        assert!(!source.contains("format!(\"{:?}\""));
        assert!(!source.contains(&["Hash", "Map"].concat()));
    }

    #[test]
    fn nested_hard_union_preserves_authored_f32_scalar_order() {
        let mut scalar = ScalarArena::new(1);
        let mut push_scalar = |op, dependency, label| {
            scalar
                .push(ScalarNode { op, dependency }, NodeOrigin::synthetic(label))
                .unwrap()
        };
        let zero = push_scalar(
            ScalarOp::ConstF32(0.0f32.to_bits()),
            Dependency::Constant,
            "zero",
        );
        let one = push_scalar(
            ScalarOp::ConstF32(1.0f32.to_bits()),
            Dependency::Constant,
            "one",
        );
        let k = push_scalar(ScalarOp::ConstF32(0x3bf0_eb6b), Dependency::Constant, "k");
        let x = push_scalar(ScalarOp::CoordX, Dependency::Coordinate, "x");
        let y = push_scalar(ScalarOp::CoordY, Dependency::Coordinate, "y");
        let z = push_scalar(ScalarOp::CoordZ, Dependency::Coordinate, "z");
        let hard_scalar = push_scalar(ScalarOp::Min(x, y), Dependency::Coordinate, "hard");
        let smooth_scalar = push_scalar(
            ScalarOp::SmoothMin {
                a: hard_scalar,
                b: z,
                k,
                semantic: SemanticOpId::SmoothMinF32V1,
            },
            Dependency::Coordinate,
            "smooth",
        );

        let mut fields = FieldArena::new(2);
        let primitive = |value| Primitive::Plane {
            normal: [one, zero, zero],
            offset: value,
        };
        let x_field = fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(primitive(x)),
                    scalar_value: x,
                },
                NodeOrigin::synthetic("x-field"),
            )
            .unwrap();
        let y_field = fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(primitive(y)),
                    scalar_value: y,
                },
                NodeOrigin::synthetic("y-field"),
            )
            .unwrap();
        let z_field = fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(primitive(z)),
                    scalar_value: z,
                },
                NodeOrigin::synthetic("z-field"),
            )
            .unwrap();
        let hard = fields
            .push(
                FieldNode {
                    kind: FieldKind::HardUnion {
                        a: x_field,
                        b: y_field,
                    },
                    scalar_value: hard_scalar,
                },
                NodeOrigin::synthetic("hard-field"),
            )
            .unwrap();
        let root = fields
            .push(
                FieldNode {
                    kind: FieldKind::SmoothUnion {
                        a: hard,
                        b: z_field,
                        k,
                    },
                    scalar_value: smooth_scalar,
                },
                NodeOrigin::synthetic("smooth-field"),
            )
            .unwrap();

        let mut materials = MaterialArena::new(3);
        let material_root = materials
            .push(
                MaterialNode {
                    kind: MaterialKind::Sample(MaterialSampleNode {
                        base_color: [zero; 3],
                        opacity: one,
                        emissive: [zero; 3],
                        roughness: zero,
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
        let mut graph = SymbolicGraph {
            renderer_index: 0,
            field_key: "scene::world".to_string(),
            material_key: "scene::shade".to_string(),
            params_type: Type::Unit,
            material_type: Type::Unit,
            params: Vec::new(),
            scalar,
            fields,
            materials,
            field_root: root,
            material_root,
            obligations: Vec::new(),
            quota: SymbolicQuota::default(),
        };
        let before = graph.clone();
        let mut rejected_real_lattice_rewrite = before.clone();
        normalize_hard_boundaries(&mut rejected_real_lattice_rewrite).unwrap();
        run_once(&mut rejected_real_lattice_rewrite).unwrap();
        run(&mut graph).unwrap();

        let root = graph.fields.get(graph.field_root).unwrap();
        let FieldKind::SmoothUnion { a, b: _, .. } = root.kind else {
            panic!("canonicalization must retain the authored smooth root")
        };
        assert!(matches!(
            graph.fields.get(a).unwrap().kind,
            FieldKind::HardUnion { .. }
        ));
        for x in [-1.0_f32, -0.125, 0.0, 0.125, 1.0] {
            for y in [-1.0_f32, -0.125, 0.0, 0.125, 1.0] {
                for z in [-1.0_f32, -0.125, 0.0, 0.125, 1.0] {
                    let expected = eval_scalar(
                        &before.scalar,
                        before.fields.get(before.field_root).unwrap().scalar_value,
                        [x, y, z],
                    );
                    let actual = eval_scalar(
                        &graph.scalar,
                        graph.fields.get(graph.field_root).unwrap().scalar_value,
                        [x, y, z],
                    );
                    assert_eq!(expected.to_bits(), actual.to_bits());
                }
            }
        }
        let adjacent = [
            f32::from_bits(0x3af0_ebb3),
            f32::from_bits(0x3af0_ebb4),
            f32::from_bits(0x3af0_eb23),
        ];
        let expected = eval_scalar(
            &before.scalar,
            before.fields.get(before.field_root).unwrap().scalar_value,
            adjacent,
        );
        let actual = eval_scalar(
            &graph.scalar,
            graph.fields.get(graph.field_root).unwrap().scalar_value,
            adjacent,
        );
        assert_eq!(expected.to_bits(), 0x0000_0000);
        assert_eq!(actual.to_bits(), expected.to_bits());
        let rewritten = eval_scalar(
            &rejected_real_lattice_rewrite.scalar,
            rejected_real_lattice_rewrite
                .fields
                .get(rejected_real_lattice_rewrite.field_root)
                .unwrap()
                .scalar_value,
            adjacent,
        );
        assert_eq!(rewritten.to_bits(), 0xaf00_0000);
        let once = graph.clone();
        run(&mut graph).unwrap();
        assert_eq!(graph, once);
    }

    #[test]
    fn canonicalization_is_idempotent_and_keeps_dense_child_first_ids() {
        let mut scalar = ScalarArena::new(1);
        let zero = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.0f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("zero"),
            )
            .unwrap();
        let one = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(1.0f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("one"),
            )
            .unwrap();
        let half = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.5f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("half"),
            )
            .unwrap();
        let duplicate_zero = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0.0f32.to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("duplicate-zero"),
            )
            .unwrap();
        let negative_zero = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32((-0.0f32).to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("negative-zero-denominator"),
            )
            .unwrap();
        let x = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::CoordX,
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("x"),
            )
            .unwrap();
        let scaled = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Mul(x, one),
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("scaled"),
            )
            .unwrap();
        let shifted = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Add(scaled, one),
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("shifted"),
            )
            .unwrap();
        let folded_constant = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Add(zero, one),
                    // Deliberately stale input metadata: canonicalization must
                    // derive the dependency of its newly folded constant.
                    dependency: Dependency::Parameter,
                },
                NodeOrigin::synthetic("folded-constant"),
            )
            .unwrap();
        let hard_value = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::Min(shifted, zero),
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("field-value"),
            )
            .unwrap();
        let field_value = scalar
            .push(
                ScalarNode {
                    op: ScalarOp::SmoothMin {
                        a: hard_value,
                        b: zero,
                        k: half,
                        semantic: SemanticOpId::SmoothMinF32V1,
                    },
                    dependency: Dependency::Coordinate,
                },
                NodeOrigin::synthetic("smooth-field-value"),
            )
            .unwrap();
        let mut fields = FieldArena::new(2);
        let plane = fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Plane {
                        normal: [zero, one, zero],
                        offset: zero,
                    }),
                    scalar_value: field_value,
                },
                NodeOrigin::synthetic("plane"),
            )
            .unwrap();
        let root = fields
            .push(
                FieldNode {
                    kind: FieldKind::Mark {
                        child: plane,
                        object_source: CanonicalIdentity {
                            enum_key: "Object".to_string(),
                            variant: "Only".to_string(),
                        },
                        material_source: CanonicalIdentity {
                            enum_key: "Material".to_string(),
                            variant: "Only".to_string(),
                        },
                    },
                    scalar_value: field_value,
                },
                NodeOrigin::synthetic("mark"),
            )
            .unwrap();
        let identity_root = fields
            .push(
                FieldNode {
                    kind: FieldKind::Transform {
                        child: root,
                        transform: TransformProgram::Translate {
                            by: [zero, zero, zero],
                        },
                    },
                    scalar_value: field_value,
                },
                NodeOrigin::synthetic("identity-transform"),
            )
            .unwrap();
        let mut materials = MaterialArena::new(3);
        let material_sample = materials
            .push(
                MaterialNode {
                    kind: MaterialKind::Sample(MaterialSampleNode {
                        base_color: [zero; 3],
                        opacity: one,
                        emissive: [duplicate_zero; 3],
                        roughness: folded_constant,
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
        let material_root = materials
            .push(
                MaterialNode {
                    kind: MaterialKind::Select {
                        predicate: field_value,
                        a: material_sample,
                        b: material_sample,
                    },
                },
                NodeOrigin::synthetic("redundant-material-select"),
            )
            .unwrap();
        let mut graph = SymbolicGraph {
            renderer_index: 0,
            field_key: "scene::world".to_string(),
            material_key: "scene::shade".to_string(),
            params_type: Type::Unit,
            material_type: Type::Unit,
            params: Vec::new(),
            scalar,
            fields,
            materials,
            field_root: identity_root,
            material_root,
            obligations: vec![
                PendingObligation::Scalar(ProofObligation::DenominatorNonZero {
                    denominator: negative_zero,
                }),
                PendingObligation::MaterialEvent {
                    predicate: field_value,
                },
            ],
            quota: SymbolicQuota::default(),
        };
        let before = graph.clone();
        run(&mut graph).unwrap();
        let saturation = -1.5_f32;
        let x_values = [
            -8.0_f32,
            f32::from_bits(saturation.to_bits() - 1),
            saturation,
            f32::from_bits(saturation.to_bits() + 1),
            -1.0,
            -0.0,
            0.0,
            0.5,
            32.0,
        ];
        let yz_values = [-3.0_f32, -0.0, 0.25, 1.0, 7.5];
        for x in x_values {
            for y in yz_values {
                for z in yz_values {
                    let before_value = eval_scalar(
                        &before.scalar,
                        before.fields.get(before.field_root).unwrap().scalar_value,
                        [x, y, z],
                    );
                    let after_value = eval_scalar(
                        &graph.scalar,
                        graph.fields.get(graph.field_root).unwrap().scalar_value,
                        [x, y, z],
                    );
                    assert_eq!(before_value.to_bits(), after_value.to_bits());
                }
            }
        }
        let once = graph.clone();
        run(&mut graph).unwrap();
        assert_eq!(graph, once);
        assert!(matches!(
            graph.fields.get(graph.field_root).unwrap().kind,
            FieldKind::Mark { .. }
        ));
        assert_eq!(
            graph
                .scalar
                .iter()
                .filter(|(_, node)| node.op == ScalarOp::ConstF32(0.0f32.to_bits()))
                .count(),
            1
        );
        let MaterialKind::Sample(sample) = &graph.materials.get(graph.material_root).unwrap().kind
        else {
            panic!("test material remains a sample")
        };
        assert_eq!(
            graph.scalar.get(sample.roughness).unwrap().dependency,
            Dependency::Constant
        );
        assert_eq!(
            graph.obligations.len(),
            1,
            "folding identical material branches must remove their hard event obligation"
        );
        let PendingObligation::Scalar(ProofObligation::DenominatorNonZero { denominator }) =
            graph.obligations[0]
        else {
            panic!("denominator obligation was not preserved")
        };
        assert_eq!(
            graph.scalar.get(denominator).unwrap().op,
            ScalarOp::ConstF32((-0.0f32).to_bits())
        );
    }

    #[test]
    fn rigid_sequence_canonicalization_preserves_source_order_rounding() {
        let mut scalar = ScalarArena::new(1);
        let mut push = |value: f32, label: &str| {
            scalar
                .push(
                    ScalarNode {
                        op: ScalarOp::ConstF32(value.to_bits()),
                        dependency: Dependency::Constant,
                    },
                    NodeOrigin::synthetic(label),
                )
                .unwrap()
        };
        let zero = push(0.0, "zero");
        let one = push(1.0, "one");
        let tiny = push(2.0_f32.powi(-25), "tiny");
        let scalar_map = scalar
            .iter()
            .map(|(id, _)| (id, id))
            .collect::<BTreeMap<_, _>>();
        let source = TransformProgram::SourceRigidSequence {
            steps: vec![
                TransformProgram::Translate {
                    by: [one, zero, zero],
                },
                TransformProgram::Translate {
                    by: [tiny, zero, zero],
                },
            ],
            // 1 + 2^-25 rounds back to 1 in f32.
            composed: Box::new(TransformProgram::Translate {
                by: [one, zero, zero],
            }),
        };

        let canonical = remap_transform(&source, &scalar_map).unwrap();
        let TransformProgram::RigidSequence { steps, composed } = &canonical else {
            panic!("canonicalization must retain the ordered source sequence");
        };
        let value = |id| super::super::scalar::constant_value(&scalar, id).unwrap();
        let apply_translate = |point: f32, transform: &TransformProgram| {
            let TransformProgram::Translate { by } = transform else {
                panic!("test sequence contains translations")
            };
            point - value(by[0])
        };
        let sequential = steps
            .iter()
            .fold(1.0_f32, |point, step| apply_translate(point, step));
        let composed = apply_translate(1.0, composed);
        assert_eq!(sequential.to_bits(), (-2.0_f32.powi(-25)).to_bits());
        assert_ne!(sequential.to_bits(), composed.to_bits());
        assert!(!is_identity_transform(&canonical, &scalar));
    }
}
