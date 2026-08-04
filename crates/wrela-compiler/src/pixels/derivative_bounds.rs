//! Conservative world/parameter derivative contracts for scalar nodes.

use std::collections::BTreeMap;

use super::bounds::ValueBounds;
use super::ids::{FieldId, ParamId, ScalarId};
use super::scalar::ScalarOp;
use super::symbolic::SymbolicGraph;

#[derive(Clone, Debug, PartialEq)]
pub struct DerivativeBound {
    pub world_components: [f64; 3],
    pub world_gradient_norm: f64,
    pub parameter: BTreeMap<ParamId, f64>,
    pub frame_delta: Option<f64>,
    pub frame_second_delta: Option<f64>,
    pub hessian_norm: f64,
    pub third_derivative_norm: f64,
    pub nonsmooth: bool,
    pub rule: &'static str,
    gradient_norm_override: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DerivativeBounds {
    pub scalar: BTreeMap<ScalarId, DerivativeBound>,
}

impl DerivativeBounds {
    pub fn get(&self, id: ScalarId) -> Result<&DerivativeBound, String> {
        self.scalar
            .get(&id)
            .ok_or_else(|| format!("pixels::derivative_bounds: missing predecessor {id}"))
    }
}

fn zero(rule: &'static str) -> DerivativeBound {
    DerivativeBound {
        world_components: [0.0; 3],
        world_gradient_norm: 0.0,
        parameter: BTreeMap::new(),
        frame_delta: Some(0.0),
        frame_second_delta: Some(0.0),
        hessian_norm: 0.0,
        third_derivative_norm: 0.0,
        nonsmooth: false,
        rule,
        gradient_norm_override: None,
    }
}

fn finite(value: f64, id: ScalarId, quantity: &str) -> Result<f64, String> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(format!(
            "pixels::derivative_bounds: {id} produced invalid {quantity} bound {value}"
        ))
    }
}

fn finalize(mut bound: DerivativeBound, id: ScalarId) -> Result<DerivativeBound, String> {
    let component_norm = bound
        .world_components
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    bound.world_gradient_norm = finite(
        bound.gradient_norm_override.unwrap_or(component_norm),
        id,
        "gradient",
    )?;
    bound.hessian_norm = finite(bound.hessian_norm, id, "Hessian")?;
    bound.third_derivative_norm = finite(bound.third_derivative_norm, id, "third-derivative")?;
    for value in bound.parameter.values() {
        finite(*value, id, "parameter derivative")?;
    }
    if let Some(value) = bound.frame_delta {
        finite(value, id, "frame delta")?;
    }
    if let Some(value) = bound.frame_second_delta {
        finite(value, id, "frame second delta")?;
    }
    Ok(bound)
}

fn gradient_norm(bound: &DerivativeBound) -> f64 {
    bound
        .gradient_norm_override
        .unwrap_or(bound.world_gradient_norm)
}

fn add_maps(a: &BTreeMap<ParamId, f64>, b: &BTreeMap<ParamId, f64>) -> BTreeMap<ParamId, f64> {
    let mut result = a.clone();
    for (param, value) in b {
        *result.entry(*param).or_default() += value;
    }
    result
}

fn sum(bounds: &[&DerivativeBound], rule: &'static str) -> DerivativeBound {
    let mut result = zero(rule);
    for bound in bounds {
        for component in 0..3 {
            result.world_components[component] += bound.world_components[component];
        }
        result.parameter = add_maps(&result.parameter, &bound.parameter);
        result.hessian_norm += bound.hessian_norm;
        result.third_derivative_norm += bound.third_derivative_norm;
        result.nonsmooth |= bound.nonsmooth;
        result.frame_delta = match (result.frame_delta, bound.frame_delta) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
        result.frame_second_delta = match (result.frame_second_delta, bound.frame_second_delta) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
    }
    result.gradient_norm_override = Some(bounds.iter().map(|bound| gradient_norm(bound)).sum());
    result
}

fn branch_max(bounds: &[&DerivativeBound], rule: &'static str) -> DerivativeBound {
    let mut result = zero(rule);
    for component in 0..3 {
        result.world_components[component] = bounds
            .iter()
            .map(|bound| bound.world_components[component])
            .fold(0.0, f64::max);
    }
    for bound in bounds {
        for (param, value) in &bound.parameter {
            result
                .parameter
                .entry(*param)
                .and_modify(|prior| *prior = prior.max(*value))
                .or_insert(*value);
        }
        result.hessian_norm = result.hessian_norm.max(bound.hessian_norm);
        result.third_derivative_norm = result
            .third_derivative_norm
            .max(bound.third_derivative_norm);
        result.nonsmooth |= bound.nonsmooth;
    }
    result.gradient_norm_override = Some(
        bounds
            .iter()
            .map(|bound| gradient_norm(bound))
            .fold(0.0, f64::max),
    );
    result.frame_delta = bounds
        .iter()
        .map(|bound| bound.frame_delta)
        .collect::<Option<Vec<_>>>()
        .map(|values| values.into_iter().fold(0.0, f64::max));
    result.frame_second_delta = bounds
        .iter()
        .map(|bound| bound.frame_second_delta)
        .collect::<Option<Vec<_>>>()
        .map(|values| values.into_iter().fold(0.0, f64::max));
    result
}

fn scaled(bound: &DerivativeBound, scale: f64, rule: &'static str) -> DerivativeBound {
    let mut result = bound.clone();
    for value in &mut result.world_components {
        *value *= scale;
    }
    for value in result.parameter.values_mut() {
        *value *= scale;
    }
    result.hessian_norm *= scale;
    result.third_derivative_norm *= scale;
    result.gradient_norm_override = Some(gradient_norm(bound) * scale);
    result.frame_delta = result.frame_delta.map(|value| value * scale);
    result.frame_second_delta = result.frame_second_delta.map(|value| value * scale);
    result.rule = rule;
    result
}

fn jacobian_norm(rows: &[&DerivativeBound]) -> f64 {
    let one_norm = (0..3)
        .map(|column| {
            rows.iter()
                .map(|row| row.world_components[column])
                .sum::<f64>()
        })
        .fold(0.0, f64::max);
    let infinity_norm = rows
        .iter()
        .map(|row| row.world_components.iter().sum::<f64>())
        .fold(0.0, f64::max);
    (one_norm * infinity_norm).sqrt()
}

fn length_derivative(
    rows: &[&DerivativeBound],
    minimum_length: f64,
    rule: &'static str,
) -> DerivativeBound {
    let mut result = sum(rows, rule);
    for component in 0..3 {
        result.world_components[component] = rows
            .iter()
            .map(|row| row.world_components[component].powi(2))
            .sum::<f64>()
            .sqrt();
    }
    let jacobian = jacobian_norm(rows);
    result.gradient_norm_override = Some(jacobian);
    if minimum_length > 0.0 {
        let component_hessian = rows.iter().map(|row| row.hessian_norm).sum::<f64>();
        let component_third = rows
            .iter()
            .map(|row| row.third_derivative_norm)
            .sum::<f64>();
        result.hessian_norm = component_hessian + jacobian.powi(2) / minimum_length;
        result.third_derivative_norm = component_third
            + 3.0 * jacobian * component_hessian / minimum_length
            + 3.0 * jacobian.powi(3) / minimum_length.powi(2);
        let temporal_first = rows
            .iter()
            .map(|row| row.frame_delta)
            .collect::<Option<Vec<_>>>()
            .map(|values| values.iter().map(|value| value.powi(2)).sum::<f64>().sqrt());
        let temporal_second = rows
            .iter()
            .map(|row| row.frame_second_delta)
            .collect::<Option<Vec<_>>>()
            .map(|values| values.iter().map(|value| value.powi(2)).sum::<f64>().sqrt());
        result.frame_delta = temporal_first;
        result.frame_second_delta = match (temporal_first, temporal_second) {
            (Some(first), Some(second)) => Some(second + first.powi(2) / minimum_length),
            _ => None,
        };
    } else {
        result.nonsmooth = true;
    }
    result
}

fn minimum_vector_length(values: &ValueBounds, components: &[ScalarId]) -> Result<f64, String> {
    let mut squared = super::reference::interval::F64Interval::point(0.0)?;
    for component in components {
        squared = squared.add_f32(values.get(*component)?.square_f32()?)?;
    }
    Ok(squared.sqrt_source_f32()?.lo)
}

fn multiplication(
    a: &DerivativeBound,
    b: &DerivativeBound,
    abs_a: f64,
    abs_b: f64,
) -> DerivativeBound {
    let mut result = zero("product-chain");
    for component in 0..3 {
        result.world_components[component] =
            a.world_components[component] * abs_b + b.world_components[component] * abs_a;
    }
    for param in a.parameter.keys().chain(b.parameter.keys()) {
        result.parameter.insert(
            *param,
            a.parameter.get(param).copied().unwrap_or(0.0) * abs_b
                + b.parameter.get(param).copied().unwrap_or(0.0) * abs_a,
        );
    }
    result.hessian_norm =
        a.hessian_norm * abs_b + b.hessian_norm * abs_a + 2.0 * gradient_norm(a) * gradient_norm(b);
    result.third_derivative_norm = a.third_derivative_norm * abs_b
        + b.third_derivative_norm * abs_a
        + 3.0 * (a.hessian_norm * gradient_norm(b) + b.hessian_norm * gradient_norm(a));
    result.nonsmooth = a.nonsmooth || b.nonsmooth;
    result.gradient_norm_override = Some(gradient_norm(a) * abs_b + gradient_norm(b) * abs_a);
    result.frame_delta = match (a.frame_delta, b.frame_delta) {
        (Some(a_delta), Some(b_delta)) => Some(a_delta * abs_b + b_delta * abs_a),
        _ => None,
    };
    result.frame_second_delta = match (
        a.frame_delta,
        b.frame_delta,
        a.frame_second_delta,
        b.frame_second_delta,
    ) {
        (Some(a_delta), Some(b_delta), Some(a_second), Some(b_second)) => {
            Some(a_second * abs_b + b_second * abs_a + 2.0 * a_delta * b_delta)
        }
        _ => None,
    };
    result
}

fn unary_chain(
    input: &DerivativeBound,
    first: f64,
    second: f64,
    third: f64,
    rule: &'static str,
) -> DerivativeBound {
    let mut result = scaled(input, first, rule);
    let input_gradient = gradient_norm(input);
    result.hessian_norm = first * input.hessian_norm + second * input_gradient.powi(2);
    result.third_derivative_norm = first * input.third_derivative_norm
        + 3.0 * second * input_gradient * input.hessian_norm
        + third * input_gradient.powi(3);
    result.frame_second_delta = match (input.frame_delta, input.frame_second_delta) {
        (Some(delta), Some(second_delta)) => Some(first * second_delta + second * delta.powi(2)),
        _ => None,
    };
    result
}

#[derive(Clone, Copy)]
struct FieldGradient {
    components: [f64; 3],
    norm: f64,
    rule: &'static str,
}

fn max_field_gradient(a: FieldGradient, b: FieldGradient, rule: &'static str) -> FieldGradient {
    FieldGradient {
        components: std::array::from_fn(|component| {
            a.components[component].max(b.components[component])
        }),
        norm: a.norm.max(b.norm),
        rule,
    }
}

fn rigid_transform_norm(
    transform: &super::graph::TransformProgram,
    values: &ValueBounds,
) -> Result<f64, String> {
    use super::graph::TransformProgram;

    let rows_norm = |rows: [[ScalarId; 3]; 3]| -> Result<f64, String> {
        // For G=MᵀM, λmax(G) ≤ 1 + ||G-I||∞. Interval Gram entries preserve
        // exact isometries at factor one while conservatively accounting for
        // source-f32 orthonormality error and parameterized coefficient boxes.
        let mut coefficients = [[super::reference::interval::F64Interval::point(0.0)?; 3]; 3];
        for row in 0..3 {
            for column in 0..3 {
                coefficients[row][column] = values.get(rows[row][column])?;
            }
        }
        let exact_signed_permutation = (0..3).all(|row| {
            (0..3)
                .filter(|column| coefficients[row][*column].lo.abs() == 1.0)
                .count()
                == 1
                && (0..3).all(|column| {
                    let value = coefficients[row][column];
                    value.lo == value.hi && matches!(value.lo, -1.0 | 0.0 | 1.0)
                })
        }) && (0..3).all(|column| {
            (0..3)
                .filter(|row| coefficients[*row][column].lo.abs() == 1.0)
                .count()
                == 1
        });
        if exact_signed_permutation {
            return Ok(1.0);
        }
        let mut error_norm = 0.0_f64;
        for gram_row in 0..3 {
            let mut row_sum = 0.0;
            for gram_column in 0..3 {
                let mut entry = super::reference::interval::F64Interval::point(0.0)?;
                for source_row in 0..3 {
                    entry = entry.add_outward(
                        coefficients[source_row][gram_row]
                            .mul_outward(coefficients[source_row][gram_column])?,
                    )?;
                }
                if gram_row == gram_column {
                    entry =
                        entry.sub_outward(super::reference::interval::F64Interval::point(1.0)?)?;
                }
                row_sum = super::reference::interval::next_up(row_sum + entry.abs_upper());
            }
            error_norm = error_norm.max(row_sum);
        }
        Ok(super::reference::interval::next_up(
            (1.0 + error_norm).sqrt(),
        ))
    };

    match transform {
        TransformProgram::Translate { .. } => Ok(1.0),
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        }
        | TransformProgram::Rigid {
            row_x,
            row_y,
            row_z,
            ..
        } => rows_norm([*row_x, *row_y, *row_z]),
        TransformProgram::UniformScale { .. } => Ok(1.0),
        TransformProgram::SourceRigidSequence { steps, .. }
        | TransformProgram::RigidSequence { steps, .. } => {
            steps.iter().try_fold(1.0, |product, step| {
                Ok(super::reference::interval::next_up(
                    product * rigid_transform_norm(step, values)?,
                ))
            })
        }
    }
}

fn refine_field_lipschitz(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    derivatives: &mut DerivativeBounds,
) -> Result<(), String> {
    let scalar_derivatives = derivatives.scalar.clone();
    fn visit(
        graph: &SymbolicGraph,
        values: &ValueBounds,
        scalar_derivatives: &BTreeMap<ScalarId, DerivativeBound>,
        id: FieldId,
        memo: &mut BTreeMap<FieldId, FieldGradient>,
    ) -> Result<FieldGradient, String> {
        if let Some(bound) = memo.get(&id) {
            return Ok(*bound);
        }
        let node = graph.fields.get(id)?;
        let child =
            |id, memo: &mut BTreeMap<_, _>| visit(graph, values, scalar_derivatives, id, memo);
        let bound = match &node.kind {
            super::graph::FieldKind::Primitive(super::graph::Primitive::Plane {
                normal, ..
            }) => {
                let components = [
                    values.get(normal[0])?.abs_upper(),
                    values.get(normal[1])?.abs_upper(),
                    values.get(normal[2])?.abs_upper(),
                ];
                FieldGradient {
                    norm: components
                        .iter()
                        .map(|value| value * value)
                        .sum::<f64>()
                        .sqrt(),
                    components,
                    rule: "fused-plane-gradient",
                }
            }
            super::graph::FieldKind::Primitive(super::graph::Primitive::FiniteCone { .. }) => {
                // The authored finite-cone scalar is not a unit-distance
                // function when the radius varies along its axis. Preserve
                // the complete scalar-chain derivative instead of replacing
                // it with the one-Lipschitz primitive shortcut.
                let scalar = scalar_derivatives.get(&node.scalar_value).ok_or_else(|| {
                    format!(
                        "pixels::derivative_bounds: missing finite-cone scalar {}",
                        node.scalar_value
                    )
                })?;
                FieldGradient {
                    components: scalar.world_components,
                    norm: gradient_norm(scalar),
                    rule: "fused-finite-cone-scalar-gradient",
                }
            }
            super::graph::FieldKind::Primitive(_) => FieldGradient {
                components: [1.0; 3],
                norm: 1.0,
                rule: "fused-one-lipschitz-primitive",
            },
            super::graph::FieldKind::HardUnion { a, b }
            | super::graph::FieldKind::HardIntersection { a, b }
            | super::graph::FieldKind::HardSubtract { a, b } => {
                max_field_gradient(child(*a, memo)?, child(*b, memo)?, "fused-hard-branch-max")
            }
            super::graph::FieldKind::SmoothUnion { a, b, k }
            | super::graph::FieldKind::SmoothIntersection { a, b, k }
            | super::graph::FieldKind::SmoothSubtract { a, b, k } => {
                let mut bound = max_field_gradient(
                    child(*a, memo)?,
                    child(*b, memo)?,
                    "fused-smooth-convex-gradient",
                );
                let k_derivative = scalar_derivatives.get(k).ok_or_else(|| {
                    format!("pixels::derivative_bounds: missing smooth coefficient {k}")
                })?;
                for component in 0..3 {
                    bound.components[component] += k_derivative.world_components[component] / 4.0;
                }
                bound.norm += gradient_norm(k_derivative) / 4.0;
                bound
            }
            super::graph::FieldKind::Neg { child: field }
            | super::graph::FieldKind::FiniteRepeat { child: field, .. }
            | super::graph::FieldKind::Mark { child: field, .. } => child(*field, memo)?,
            super::graph::FieldKind::Transform {
                child: field,
                transform,
            } => {
                let mut bound = child(*field, memo)?;
                match transform {
                    super::graph::TransformProgram::UniformScale { scale } => {
                        let factor = values.get(*scale)?.abs_upper();
                        for component in &mut bound.components {
                            *component *= factor;
                        }
                        bound.norm *= factor;
                        bound.rule = "fused-uniform-scale-gradient";
                    }
                    _ => {
                        let factor = rigid_transform_norm(transform, values)?;
                        for component in &mut bound.components {
                            *component *= factor;
                        }
                        bound.norm *= factor;
                        bound.rule = "fused-rigid-matrix-gradient";
                    }
                }
                bound
            }
            super::graph::FieldKind::BoundedDisplace {
                base, displacement, ..
            } => {
                let mut bound = child(*base, memo)?;
                let displacement = scalar_derivatives
                    .get(displacement)
                    .ok_or_else(|| {
                        format!(
                            "pixels::derivative_bounds: missing displacement derivative {displacement}"
                        )
                    })?
                    .world_gradient_norm;
                for component in &mut bound.components {
                    *component += displacement;
                }
                bound.norm += displacement;
                bound.rule = "fused-bounded-displacement-gradient";
                bound
            }
        };
        memo.insert(id, bound);
        Ok(bound)
    }

    let mut memo = BTreeMap::new();
    for (id, _) in graph.fields.iter() {
        visit(graph, values, &scalar_derivatives, id, &mut memo)?;
    }
    let mut scalar_gradients = BTreeMap::<ScalarId, FieldGradient>::new();
    for (field, bound) in memo {
        let scalar = graph.fields.get(field)?.scalar_value;
        scalar_gradients
            .entry(scalar)
            .and_modify(|prior| {
                *prior = max_field_gradient(*prior, bound, "fused-field-occurrence-max")
            })
            .or_insert(bound);
    }
    for (scalar, bound) in scalar_gradients {
        let derivative = derivatives
            .scalar
            .get_mut(&scalar)
            .ok_or_else(|| format!("pixels::derivative_bounds: missing field scalar {scalar}"))?;
        derivative.world_components = bound.components;
        derivative.world_gradient_norm = bound.norm;
        derivative.gradient_norm_override = Some(bound.norm);
        derivative.rule = bound.rule;
    }
    Ok(())
}

fn smooth_min_crosses_saturation(
    a: super::reference::interval::F64Interval,
    b: super::reference::interval::F64Interval,
    k: super::reference::interval::F64Interval,
) -> Result<bool, String> {
    Ok(a.sub_f32(b)?.sub_f32(k)?.contains_zero() || b.sub_f32(a)?.sub_f32(k)?.contains_zero())
}

pub fn propagate(graph: &SymbolicGraph, values: &ValueBounds) -> Result<DerivativeBounds, String> {
    let mut result = DerivativeBounds {
        scalar: BTreeMap::new(),
    };
    for (id, node) in graph.scalar.iter() {
        let get = |id| result.get(id);
        let value = |id| values.get(id);
        let bound = match &node.op {
            ScalarOp::ConstF32(_) | ScalarOp::ConstF64(_) => zero("constant"),
            ScalarOp::CoordX | ScalarOp::CoordY | ScalarOp::CoordZ => {
                let mut bound = zero("world-coordinate");
                let component = match node.op {
                    ScalarOp::CoordX => 0,
                    ScalarOp::CoordY => 1,
                    _ => 2,
                };
                bound.world_components[component] = 1.0;
                bound
            }
            ScalarOp::SurfacePosition(component) => {
                let mut bound = zero("surface-position");
                if *component >= 3 {
                    return Err(format!(
                        "pixels::derivative_bounds: invalid surface component {component}"
                    ));
                }
                bound.world_components[*component as usize] = 1.0;
                bound
            }
            ScalarOp::SurfaceNormal(_) => {
                let mut bound = zero("surface-normal-deferred");
                bound.nonsmooth = true;
                bound.frame_delta = None;
                bound.frame_second_delta = None;
                bound
            }
            ScalarOp::Param(param) => {
                let mut bound = zero("parameter");
                bound.parameter.insert(*param, 1.0);
                let record = graph.params.get(param.0 as usize).ok_or_else(|| {
                    format!("pixels::derivative_bounds: missing parameter {param}")
                })?;
                if let Some((max_delta, max_second_delta)) = record.rate {
                    bound.frame_delta = Some(max_delta);
                    bound.frame_second_delta = Some(max_second_delta);
                } else {
                    bound.frame_delta = None;
                    bound.frame_second_delta = None;
                }
                bound
            }
            ScalarOp::Add(a, b) | ScalarOp::Sub(a, b) => {
                sum(&[get(*a)?, get(*b)?], "additive-chain")
            }
            ScalarOp::Mul(a, b) => multiplication(
                get(*a)?,
                get(*b)?,
                value(*a)?.abs_upper(),
                value(*b)?.abs_upper(),
            ),
            ScalarOp::Div(a, b) => {
                let denominator = value(*b)?;
                if denominator.contains_zero() {
                    let mut bound = zero("guarded-division");
                    let maximum = f32::MAX as f64;
                    bound.world_components = [maximum; 3];
                    for param in get(*a)?.parameter.keys().chain(get(*b)?.parameter.keys()) {
                        bound.parameter.insert(*param, maximum);
                    }
                    bound.hessian_norm = maximum;
                    bound.third_derivative_norm = maximum;
                    bound.nonsmooth = true;
                    bound.gradient_norm_override = Some(maximum);
                    bound.frame_delta = None;
                    bound.frame_second_delta = None;
                    bound
                } else {
                    let minimum_abs = denominator.lo.abs().min(denominator.hi.abs());
                    let reciprocal = unary_chain(
                        get(*b)?,
                        1.0 / minimum_abs.powi(2),
                        2.0 / minimum_abs.powi(3),
                        6.0 / minimum_abs.powi(4),
                        "reciprocal",
                    );
                    multiplication(
                        get(*a)?,
                        &reciprocal,
                        value(*a)?.abs_upper(),
                        1.0 / minimum_abs,
                    )
                }
            }
            ScalarOp::Neg(value) => scaled(get(*value)?, 1.0, "neg"),
            ScalarOp::Abs(value) => {
                let mut bound = scaled(get(*value)?, 1.0, "abs");
                bound.nonsmooth |= values.get(*value)?.contains_zero();
                bound
            }
            ScalarOp::Min(a, b) | ScalarOp::Max(a, b) => {
                let a = get(*a)?;
                let b = get(*b)?;
                let mut bound = branch_max(&[a, b], "hard-extremum");
                bound.nonsmooth = true;
                bound
            }
            ScalarOp::Clamp { value, lo, hi } => {
                let mut bound = branch_max(&[get(*value)?, get(*lo)?, get(*hi)?], "clamp-branches");
                bound.nonsmooth = true;
                bound
            }
            ScalarOp::Sqrt(value_id, _) => {
                let input = values.get(*value_id)?;
                if input.lo <= 0.0 {
                    let maximum = f32::MAX as f64;
                    let mut bound = get(*value_id)?.clone();
                    bound.rule = "sqrt-zero-boundary";
                    bound.world_components = [maximum; 3];
                    bound.gradient_norm_override = Some(maximum);
                    for value in bound.parameter.values_mut() {
                        *value = maximum;
                    }
                    bound.hessian_norm = maximum;
                    bound.third_derivative_norm = maximum;
                    bound.nonsmooth = true;
                    bound.frame_delta = None;
                    bound.frame_second_delta = None;
                    bound
                } else {
                    unary_chain(
                        get(*value_id)?,
                        0.5 / input.lo.sqrt(),
                        0.25 / input.lo.powf(1.5),
                        0.375 / input.lo.powf(2.5),
                        "sqrt-chain",
                    )
                }
            }
            ScalarOp::Rsqrt(value_id, _) => {
                let input = values.get(*value_id)?;
                if input.lo <= 0.0 {
                    return Err(format!(
                        "pixels::derivative_bounds: reciprocal sqrt {id} may reach zero"
                    ));
                }
                unary_chain(
                    get(*value_id)?,
                    0.5 / (input.lo * input.lo.sqrt()),
                    0.75 / input.lo.powf(2.5),
                    1.875 / input.lo.powf(3.5),
                    "rsqrt-chain",
                )
            }
            ScalarOp::SinRestricted(value, _) | ScalarOp::CosRestricted(value, _) => {
                let cosine = matches!(&node.op, ScalarOp::CosRestricted(_, _));
                let mut bound = unary_chain(
                    get(*value)?,
                    f64::from(super::scalar::SOURCE_TRIG_GRADIENT_FACTOR_V1),
                    f64::from(super::scalar::SOURCE_TRIG_HESSIAN_FACTOR_V1),
                    f64::from(super::scalar::SOURCE_TRIG_THIRD_FACTOR_V1),
                    "source-folded-polynomial-trig-chain",
                );
                if !super::reference::interval::source_trig_is_smooth_on(
                    values.get(*value)?,
                    cosine,
                )? {
                    bound.nonsmooth = true;
                    bound.rule = "source-folded-polynomial-trig-kink-domain";
                }
                bound
            }
            ScalarOp::Dot3(a, b) => {
                let products = (0..3)
                    .map(|component| {
                        Ok(multiplication(
                            get(a[component])?,
                            get(b[component])?,
                            value(a[component])?.abs_upper(),
                            value(b[component])?.abs_upper(),
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                sum(&products.iter().collect::<Vec<_>>(), "dot3-fused")
            }
            ScalarOp::Cross3Component { component, a, b } => {
                let (j, k) = match component {
                    0 => (1, 2),
                    1 => (2, 0),
                    2 => (0, 1),
                    _ => {
                        return Err(format!(
                            "pixels::derivative_bounds: invalid cross component {component}"
                        ));
                    }
                };
                let left = multiplication(
                    get(a[j])?,
                    get(b[k])?,
                    value(a[j])?.abs_upper(),
                    value(b[k])?.abs_upper(),
                );
                let right = multiplication(
                    get(a[k])?,
                    get(b[j])?,
                    value(a[k])?.abs_upper(),
                    value(b[j])?.abs_upper(),
                );
                sum(&[&left, &right], "cross3-fused")
            }
            ScalarOp::Length2(components) => length_derivative(
                &[get(components[0])?, get(components[1])?],
                minimum_vector_length(values, components)?,
                "length-fused",
            ),
            ScalarOp::Length3(components) => length_derivative(
                &[
                    get(components[0])?,
                    get(components[1])?,
                    get(components[2])?,
                ],
                minimum_vector_length(values, components)?,
                "length-fused",
            ),
            ScalarOp::Normalize3Component {
                component, value, ..
            } => {
                let minimum_length = minimum_vector_length(values, value)?;
                let combined = sum(
                    &[get(value[0])?, get(value[1])?, get(value[2])?],
                    "normalize3-fused",
                );
                if minimum_length > 0.0 {
                    let length = length_derivative(
                        &[get(value[0])?, get(value[1])?, get(value[2])?],
                        minimum_length,
                        "normalize3-length",
                    );
                    let reciprocal = unary_chain(
                        &length,
                        1.0 / minimum_length.powi(2),
                        2.0 / minimum_length.powi(3),
                        6.0 / minimum_length.powi(4),
                        "normalize3-reciprocal-length",
                    );
                    let component_bound = get(value[*component as usize])?;
                    multiplication(
                        component_bound,
                        &reciprocal,
                        values.get(value[*component as usize])?.abs_upper(),
                        1.0 / minimum_length,
                    )
                } else {
                    let maximum = f32::MAX as f64;
                    let mut bound = zero("normalize3-zero-safe");
                    bound.world_components = [maximum; 3];
                    bound.gradient_norm_override = Some(maximum);
                    bound.hessian_norm = maximum;
                    bound.third_derivative_norm = maximum;
                    bound.nonsmooth = true;
                    bound.parameter = combined
                        .parameter
                        .into_keys()
                        .map(|param| (param, maximum))
                        .collect();
                    bound.frame_delta = None;
                    bound.frame_second_delta = None;
                    bound
                }
            }
            ScalarOp::Compare { a, b, .. } => {
                let mut bound = sum(&[get(*a)?, get(*b)?], "comparison-predicate");
                bound.nonsmooth = true;
                bound
            }
            ScalarOp::Select { predicate, a, b } => {
                let predicate_value = values.get(*predicate)?;
                if predicate_value.lo == 0.0 && predicate_value.hi == 0.0 {
                    let mut bound = get(*b)?.clone();
                    bound.rule = "select-stable-false";
                    bound
                } else if !predicate_value.contains_zero() {
                    let mut bound = get(*a)?.clone();
                    bound.rule = "select-stable-true";
                    bound
                } else {
                    let mut bound = sum(&[get(*predicate)?, get(*a)?, get(*b)?], "select-union");
                    bound.nonsmooth = true;
                    bound
                }
            }
            ScalarOp::SelectIndex { index, options } => {
                let index_value = values.get(*index)?;
                if index_value.lo == index_value.hi
                    && index_value.lo.fract() == 0.0
                    && index_value.lo >= 0.0
                    && index_value.lo < options.len() as f64
                {
                    let mut bound = get(options[index_value.lo as usize])?.clone();
                    bound.rule = "select-index-stable";
                    bound
                } else {
                    let mut all = vec![get(*index)?];
                    for option in options {
                        all.push(get(*option)?);
                    }
                    let mut bound = sum(&all, "select-index-union");
                    bound.nonsmooth = true;
                    bound
                }
            }
            ScalarOp::SmoothMin { a, b, k, .. } => {
                let a_value = values.get(*a)?;
                let b_value = values.get(*b)?;
                let k_value = values.get(*k)?;
                let minimum_k = k_value.lo;
                let a = get(*a)?;
                let b = get(*b)?;
                let k = get(*k)?;
                let mut bound = zero("smooth-min-convex-gradient");
                for component in 0..3 {
                    bound.world_components[component] = a.world_components[component]
                        .max(b.world_components[component])
                        + k.world_components[component] / 4.0;
                }
                for param in a
                    .parameter
                    .keys()
                    .chain(b.parameter.keys())
                    .chain(k.parameter.keys())
                {
                    bound.parameter.insert(
                        *param,
                        a.parameter
                            .get(param)
                            .copied()
                            .unwrap_or(0.0)
                            .max(b.parameter.get(param).copied().unwrap_or(0.0))
                            + k.parameter.get(param).copied().unwrap_or(0.0) / 4.0,
                    );
                }
                bound.hessian_norm = a.hessian_norm.max(b.hessian_norm) + k.hessian_norm / 4.0;
                let input_gradient =
                    a.world_gradient_norm + b.world_gradient_norm + k.world_gradient_norm;
                let input_hessian = a.hessian_norm + b.hessian_norm + k.hessian_norm;
                bound.hessian_norm += 2.0 * input_gradient.powi(2) / minimum_k;
                bound.third_derivative_norm = a.third_derivative_norm.max(b.third_derivative_norm)
                    + k.third_derivative_norm / 4.0
                    + 6.0 * input_gradient * input_hessian / minimum_k
                    + 12.0 * input_gradient.powi(3) / minimum_k.powi(2);
                bound.gradient_norm_override = Some(
                    a.world_gradient_norm.max(b.world_gradient_norm) + k.world_gradient_norm / 4.0,
                );
                let temporal_first = match (a.frame_delta, b.frame_delta, k.frame_delta) {
                    (Some(a), Some(b), Some(k)) => Some(a + b + k),
                    _ => None,
                };
                bound.frame_delta = match (a.frame_delta, b.frame_delta, k.frame_delta) {
                    (Some(a), Some(b), Some(k)) => Some(a.max(b) + k / 4.0),
                    _ => None,
                };
                bound.frame_second_delta = match (
                    a.frame_second_delta,
                    b.frame_second_delta,
                    k.frame_second_delta,
                    temporal_first,
                ) {
                    (Some(a_second), Some(b_second), Some(k_second), Some(first)) => Some(
                        a_second.max(b_second) + k_second / 4.0 + 2.0 * first.powi(2) / minimum_k,
                    ),
                    _ => None,
                };
                let crosses_saturation = smooth_min_crosses_saturation(a_value, b_value, k_value)?;
                bound.nonsmooth = a.nonsmooth || b.nonsmooth || k.nonsmooth || crosses_saturation;
                if crosses_saturation {
                    bound.rule = "smooth-min-saturation-kink-domain";
                }
                bound
            }
            ScalarOp::FiniteOr {
                value, fallback, ..
            } => {
                let mut bound = sum(&[get(*value)?, get(*fallback)?], "finite-or");
                bound.nonsmooth = true;
                bound
            }
            ScalarOp::MaterialRoughness { value, .. } => {
                let mut bound = get(*value)?.clone();
                bound.rule = "material-roughness";
                bound.nonsmooth = true;
                bound
            }
        };
        let bound = finalize(bound, id)?;
        result.scalar.insert(id, bound);
    }
    refine_field_lipschitz(graph, values, &mut result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(
        intervals: &[(ScalarId, super::super::reference::interval::F64Interval)],
    ) -> ValueBounds {
        ValueBounds {
            scalar: intervals
                .iter()
                .map(|(id, value)| {
                    (
                        *id,
                        super::super::bounds::ScalarBound {
                            value: *value,
                            rule: "test",
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn length_rule_uses_component_norm_not_division_at_zero() {
        let x = finalize(
            DerivativeBound {
                world_components: [1.0, 0.0, 0.0],
                ..zero("x")
            },
            ScalarId(0),
        )
        .unwrap();
        let y = finalize(
            DerivativeBound {
                world_components: [0.0, 1.0, 0.0],
                ..zero("y")
            },
            ScalarId(1),
        )
        .unwrap();
        let mut length = sum(&[&x, &y], "length");
        for component in 0..3 {
            length.world_components[component] =
                x.world_components[component].hypot(y.world_components[component]);
        }
        length.gradient_norm_override = Some(jacobian_norm(&[&x, &y]));
        let length = finalize(length, ScalarId(2)).unwrap();
        assert_eq!(length.world_gradient_norm, 1.0);
        let radius = finalize(zero("radius"), ScalarId(3)).unwrap();
        let primitive = finalize(sum(&[&length, &radius], "subtract"), ScalarId(4)).unwrap();
        assert_eq!(primitive.world_gradient_norm, 1.0);
    }

    #[test]
    fn rigid_norm_preserves_exact_identity_and_charges_roundoff_and_parameters() {
        let ids = (0_u32..9).map(ScalarId).collect::<Vec<_>>();
        let point = |value| super::super::reference::interval::F64Interval::point(value).unwrap();
        let identity_values = values(&[
            (ids[0], point(1.0)),
            (ids[1], point(0.0)),
            (ids[2], point(0.0)),
            (ids[3], point(0.0)),
            (ids[4], point(1.0)),
            (ids[5], point(0.0)),
            (ids[6], point(0.0)),
            (ids[7], point(0.0)),
            (ids[8], point(1.0)),
        ]);
        let transform = super::super::graph::TransformProgram::Rotate {
            row_x: [ids[0], ids[1], ids[2]],
            row_y: [ids[3], ids[4], ids[5]],
            row_z: [ids[6], ids[7], ids[8]],
        };
        assert_eq!(
            rigid_transform_norm(&transform, &identity_values).unwrap(),
            1.0
        );

        let approximate = f64::from(0.70710678_f32);
        let approximate_values = values(&[
            (ids[0], point(approximate)),
            (ids[1], point(-approximate)),
            (ids[2], point(0.0)),
            (ids[3], point(approximate)),
            (ids[4], point(approximate)),
            (ids[5], point(0.0)),
            (ids[6], point(0.0)),
            (ids[7], point(0.0)),
            (ids[8], point(1.0)),
        ]);
        assert!(rigid_transform_norm(&transform, &approximate_values).unwrap() > 1.0);

        let ranged_values = values(&[
            (ids[0], point(1.0)),
            (
                ids[1],
                super::super::reference::interval::F64Interval::new(-0.01, 0.01).unwrap(),
            ),
            (ids[2], point(0.0)),
            (ids[3], point(0.0)),
            (ids[4], point(1.0)),
            (ids[5], point(0.0)),
            (ids[6], point(0.0)),
            (ids[7], point(0.0)),
            (ids[8], point(1.0)),
        ]);
        assert!(rigid_transform_norm(&transform, &ranged_values).unwrap() > 1.0);
    }

    #[test]
    fn sinusoidal_gradient_hessian_and_third_derivative_samples_are_bounded() {
        let pi = 3.1415927_f32;
        let below = super::super::reference::interval::next_down_f32(pi);
        let above = super::super::reference::interval::next_up_f32(pi);
        let slope = f64::from(
            (super::super::scalar::source_sin(above) - super::super::scalar::source_sin(below))
                / (above - below),
        )
        .abs();
        assert!(
            slope <= f64::from(super::super::scalar::SOURCE_TRIG_GRADIENT_FACTOR_V1),
            "folded source polynomial must not retain a modulo jump: slope={slope}"
        );

        for bits in (0_u32..=u32::MAX).step_by(65_537) {
            let angle = f32::from_bits(bits);
            if !angle.is_finite() {
                continue;
            }
            let value = super::super::scalar::source_sin(angle);
            assert!(
                value.abs() <= super::super::scalar::SOURCE_TRIG_VALUE_FACTOR_V1,
                "source value factor misses angle={angle} value={value}"
            );
        }
    }

    #[test]
    fn randomized_folded_polynomial_derivatives_fit_production_contract_factors() {
        let coefficients = [
            -0.16666666666666666_f32 as f64,
            0.008333333333333333_f32 as f64,
            -0.0001984126984126984_f32 as f64,
            0.0000027557319223985893_f32 as f64,
            -0.00000002505210838544172_f32 as f64,
        ];
        let derivatives = |x: f64| {
            let [c3, c5, c7, c9, c11] = coefficients;
            (
                1.0 + 3.0 * c3 * x.powi(2)
                    + 5.0 * c5 * x.powi(4)
                    + 7.0 * c7 * x.powi(6)
                    + 9.0 * c9 * x.powi(8)
                    + 11.0 * c11 * x.powi(10),
                6.0 * c3 * x
                    + 20.0 * c5 * x.powi(3)
                    + 42.0 * c7 * x.powi(5)
                    + 72.0 * c9 * x.powi(7)
                    + 110.0 * c11 * x.powi(9),
                6.0 * c3
                    + 60.0 * c5 * x.powi(2)
                    + 210.0 * c7 * x.powi(4)
                    + 504.0 * c9 * x.powi(6)
                    + 990.0 * c11 * x.powi(8),
            )
        };
        let mut state = 0x243f_6a88_85a3_08d3_u64;
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = (state >> 11) as f64 / (u64::MAX >> 11) as f64;
            let x = -1.5 + 3.0 * unit;
            state = state.rotate_left(23).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let amplitude = 0.01 + 7.99 * ((state >> 11) as f64 / (u64::MAX >> 11) as f64);
            state = state.rotate_left(19).wrapping_mul(0xd134_2543_de82_ef95);
            let composed_frequency =
                0.01 + 19.99 * ((state >> 11) as f64 / (u64::MAX >> 11) as f64);
            let (first, second, third) = derivatives(x);
            let frequency_abs = composed_frequency.abs();
            assert!(
                amplitude * first.abs() * frequency_abs
                    <= amplitude
                        * f64::from(super::super::scalar::SOURCE_TRIG_GRADIENT_FACTOR_V1)
                        * frequency_abs
            );
            assert!(
                amplitude * second.abs() * frequency_abs.powi(2)
                    <= amplitude
                        * f64::from(super::super::scalar::SOURCE_TRIG_HESSIAN_FACTOR_V1)
                        * frequency_abs.powi(2)
            );
            assert!(
                amplitude * third.abs() * frequency_abs.powi(3)
                    <= amplitude
                        * f64::from(super::super::scalar::SOURCE_TRIG_THIRD_FACTOR_V1)
                        * frequency_abs.powi(3)
            );
        }
    }

    #[test]
    fn smooth_min_saturation_boundaries_are_classified_nonsmooth() {
        let interval = super::super::reference::interval::F64Interval::new;
        assert!(
            smooth_min_crosses_saturation(
                interval(0.9, 1.1).unwrap(),
                interval(0.0, 0.0).unwrap(),
                interval(1.0, 1.0).unwrap(),
            )
            .unwrap()
        );
        assert!(
            !smooth_min_crosses_saturation(
                interval(-0.25, 0.25).unwrap(),
                interval(0.0, 0.0).unwrap(),
                interval(1.0, 1.0).unwrap(),
            )
            .unwrap()
        );
    }

    #[test]
    fn product_and_unary_temporal_second_order_chain_terms_are_retained() {
        let mut input = zero("parameter");
        input.frame_delta = Some(3.0);
        input.frame_second_delta = Some(5.0);
        input.world_components[0] = 1.0;
        let input = finalize(input, ScalarId(0)).unwrap();
        let square = multiplication(&input, &input, 4.0, 4.0);
        assert_eq!(square.frame_delta, Some(24.0));
        assert_eq!(square.frame_second_delta, Some(58.0));

        let sine = unary_chain(&input, 1.0, 1.0, 1.0, "trig");
        assert_eq!(sine.frame_delta, Some(3.0));
        assert_eq!(sine.frame_second_delta, Some(14.0));
        assert!(sine.hessian_norm >= 1.0);
        assert!(sine.third_derivative_norm >= 1.0);
    }

    #[test]
    fn moving_clamp_bounds_participate_in_the_derivative_bound() {
        let value = finalize(zero("value"), ScalarId(0)).unwrap();
        let mut lower = zero("lower");
        lower.world_components[0] = 2.0;
        lower.frame_delta = Some(3.0);
        let lower = finalize(lower, ScalarId(1)).unwrap();
        let mut upper = zero("upper");
        upper.world_components[1] = 4.0;
        upper.frame_delta = Some(5.0);
        let upper = finalize(upper, ScalarId(2)).unwrap();
        let clamp = finalize(
            branch_max(&[&value, &lower, &upper], "clamp-branches"),
            ScalarId(3),
        )
        .unwrap();
        assert_eq!(clamp.world_components, [2.0, 4.0, 0.0]);
        assert_eq!(clamp.frame_delta, Some(5.0));
        assert!(clamp.world_gradient_norm >= 4.0);
    }
}
