//! Compiler-derived bounded deformation templates.

use super::bounds::ValueBounds;
use super::graph::{ClosedDeformDerivation, FieldKind};
use super::ids::{FieldId, ScalarId};
use super::reference::interval::F64Interval;
use super::scalar::ScalarOp;
use super::symbolic::SymbolicGraph;

#[derive(Clone, Debug, PartialEq)]
pub struct DeformationTemplate {
    pub field: FieldId,
    pub displacement: ScalarId,
    pub derivation: ClosedDeformDerivation,
    pub amplitude: f64,
    pub gradient: f64,
    pub hessian: f64,
    pub third_derivative: f64,
    pub coordinate_x: ScalarId,
    pub frequency_scalar: ScalarId,
    pub phase_scalar: ScalarId,
    pub frequency: super::reference::interval::F64Interval,
    pub phase: super::reference::interval::F64Interval,
}

pub const SIN_COS_APPROXIMATION_REVISION_V1: u32 = 1;
pub const SIN_MINIMAX_ODD_COEFFICIENTS_V1: [f64; 7] = [
    1.0,
    -1.666_666_666_666_663_2e-1,
    8.333_333_333_322_49e-3,
    -1.984_126_982_985_795e-4,
    2.755_731_370_707_006_8e-6,
    -2.505_076_025_340_686_3e-8,
    1.589_690_995_211_55e-10,
];
pub const COS_MINIMAX_EVEN_COEFFICIENTS_V1: [f64; 7] = [
    1.0,
    -0.5,
    4.166_666_666_666_66e-2,
    -1.388_888_888_887_411e-3,
    2.480_158_728_947_673e-5,
    -2.755_731_435_139_066_3e-7,
    2.087_572_321_298_175e-9,
];
pub const SIN_COS_APPROXIMATION_REMAINDER_V1: f64 = 1.0e-12;

fn factorial(value: u32) -> f64 {
    (1..=value).fold(1.0, |product, factor| product * f64::from(factor))
}

fn pow_up(value: f64, exponent: u32) -> f64 {
    (0..exponent).fold(1.0, |power, _| {
        super::reference::interval::next_up(power * value)
    })
}

fn polynomial_abs_sum(coefficients: &[f64], argument_abs: f64) -> f64 {
    coefficients.iter().rev().fold(0.0, |bound, coefficient| {
        super::reference::interval::next_up(
            super::reference::interval::next_up(bound * argument_abs) + coefficient.abs(),
        )
    })
}

fn polynomial_argument_sensitivity(coefficients: &[f64], argument_abs: f64) -> f64 {
    coefficients
        .iter()
        .enumerate()
        .skip(1)
        .fold(0.0, |bound, (degree, coefficient)| {
            let term = super::reference::interval::next_up(
                degree as f64
                    * coefficient.abs()
                    * pow_up(argument_abs, u32::try_from(degree - 1).unwrap()),
            );
            super::reference::interval::next_up(bound + term)
        })
}

fn folded_polynomial_error_bound(coefficients: &[f64], sine: bool) -> Result<f64, String> {
    let radius = super::reference::interval::next_up(std::f64::consts::FRAC_PI_4);
    let mut approximation = 0.0_f64;
    for (index, coefficient) in coefficients.iter().copied().enumerate() {
        let power = if sine { 2 * index + 1 } else { 2 * index };
        let sign = if index % 2 == 0 { 1.0 } else { -1.0 };
        let exact_taylor = sign
            / factorial(
                u32::try_from(power)
                    .map_err(|_| "P015: trig approximation power overflow".to_string())?,
            );
        // The factorials here are below 2^53. The division is one rounded
        // operation, so one epsilon of the exact Taylor coefficient encloses
        // the conversion from the real reciprocal to f64.
        let coefficient_error = super::reference::interval::next_up(
            (coefficient - exact_taylor).abs()
                + super::reference::interval::next_up(
                    f64::EPSILON * exact_taylor.abs().max(f64::MIN_POSITIVE),
                ),
        );
        approximation = super::reference::interval::next_up(
            approximation
                + super::reference::interval::next_up(
                    coefficient_error
                        * pow_up(
                            radius,
                            u32::try_from(power).map_err(|_| {
                                "P015: trig approximation power overflow".to_string()
                            })?,
                        ),
                ),
        );
    }
    let next_power = if sine {
        2 * coefficients.len() + 1
    } else {
        2 * coefficients.len()
    };
    approximation = super::reference::interval::next_up(
        approximation
            + super::reference::interval::next_up(
                pow_up(
                    radius,
                    u32::try_from(next_power)
                        .map_err(|_| "P015: trig Taylor remainder power overflow".to_string())?,
                ) / factorial(
                    u32::try_from(next_power)
                        .map_err(|_| "P015: trig Taylor remainder power overflow".to_string())?,
                ),
            ),
    );

    // Bound the prescribed f64 evaluation: x*x, Horner multiply/add pairs,
    // and (for sine) the final multiplication by x. This is an analytic
    // complete-domain bound, not a sampled validation.
    let squared_abs = super::reference::interval::next_up(radius * radius);
    let squared_rounding =
        super::reference::interval::next_up(f64::EPSILON * squared_abs + f64::from_bits(1));
    let rounded_squared_abs = super::reference::interval::next_up(squared_abs + squared_rounding);
    let polynomial_abs = polynomial_abs_sum(coefficients, rounded_squared_abs);
    let sensitivity = polynomial_argument_sensitivity(coefficients, rounded_squared_abs);
    let input_error = super::reference::interval::next_up(sensitivity * squared_rounding);
    let operations = 2.0
        * f64::from(
            u32::try_from(coefficients.len().saturating_sub(1))
                .map_err(|_| "P015: trig Horner operation count overflow".to_string())?,
        );
    let gamma = operations * f64::EPSILON / (1.0 - operations * f64::EPSILON);
    let horner_rounding =
        super::reference::interval::next_up(gamma * polynomial_abs + f64::from_bits(1));
    let polynomial_evaluation_error =
        super::reference::interval::next_up(input_error + horner_rounding);
    let evaluation = if sine {
        let product_abs = super::reference::interval::next_up(
            radius
                * super::reference::interval::next_up(polynomial_abs + polynomial_evaluation_error),
        );
        super::reference::interval::next_up(
            super::reference::interval::next_up(radius * polynomial_evaluation_error)
                + super::reference::interval::next_up(
                    f64::EPSILON * product_abs + f64::from_bits(1),
                ),
        )
    } else {
        polynomial_evaluation_error
    };
    let total = super::reference::interval::next_up(approximation + evaluation);
    if !total.is_finite() || total < 0.0 {
        return Err("P015: trig approximation certificate is non-finite".to_string());
    }
    Ok(total)
}

pub(crate) fn certified_sin_cos_remainders() -> Result<(f64, f64), String> {
    Ok((
        folded_polynomial_error_bound(&SIN_MINIMAX_ODD_COEFFICIENTS_V1, true)?,
        folded_polynomial_error_bound(&COS_MINIMAX_EVEN_COEFFICIENTS_V1, false)?,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ApproximationContract {
    pub revision: u32,
    pub folded_domain: F64Interval,
    pub sine_remainder: f64,
    pub cosine_remainder: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhaseRecurrenceProgram {
    pub coordinate_x: ScalarId,
    pub frequency_scalar: ScalarId,
    pub phase_scalar: ScalarId,
    pub frequency: F64Interval,
    pub phase: F64Interval,
    pub sine_coefficients: [u64; 7],
    pub cosine_coefficients: [u64; 7],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectiveDeformationProgram {
    pub feature: super::ids::FeatureId,
    pub deformation_field: FieldId,
    pub predictor: super::ids::PolyProgramId,
    pub residual: ScalarId,
    pub coordinate_x: ScalarId,
    pub frequency: F64Interval,
    pub phase: F64Interval,
    pub value_bound: f64,
    pub first_derivative_bound: f64,
    pub second_derivative_bound: f64,
    pub third_derivative_bound: f64,
    pub taylor_order: u8,
    pub approximation: ApproximationContract,
    pub tube_method: &'static str,
    pub maximum_root_count: u8,
    pub phase_recurrence: PhaseRecurrenceProgram,
}

pub(crate) fn oscillation_bound(
    template: &DeformationTemplate,
    values: &ValueBounds,
) -> Result<u64, String> {
    let coordinate = values.get(template.coordinate_x)?;
    let angle = coordinate
        .mul_outward(template.frequency)?
        .add_outward(template.phase)?;
    let crossings = (angle.width() / std::f64::consts::PI).ceil();
    if !crossings.is_finite() || crossings < 0.0 || crossings > u64::MAX as f64 - 2.0 {
        return Err(format!(
            "P015: deformation {} has no finite oscillation bound",
            template.field
        ));
    }
    Ok(crossings as u64 + 2)
}

pub fn compile_projective(
    structural: &[DeformationTemplate],
    features: &[super::features::FeatureRecord],
    projective: &super::projective::ProjectiveEquations,
    values: &ValueBounds,
) -> Result<Vec<ProjectiveDeformationProgram>, String> {
    let (certified_sine, certified_cosine) = certified_sin_cos_remainders()?;
    if certified_sine > SIN_COS_APPROXIMATION_REMAINDER_V1
        || certified_cosine > SIN_COS_APPROXIMATION_REMAINDER_V1
    {
        return Err(format!(
            "P015: trig approximation certificate [{certified_sine},{certified_cosine}] exceeds versioned remainder {SIN_COS_APPROXIMATION_REMAINDER_V1}"
        ));
    }
    let mut result = Vec::new();
    for (feature, lowered) in features.iter().zip(&projective.features) {
        if feature.id != lowered.feature {
            return Err(format!(
                "pixels::deform: structural/projective feature order differs at {}",
                feature.id
            ));
        }
        let mut matches = feature
            .occurrence_path
            .iter()
            .filter_map(|step| {
                structural
                    .iter()
                    .find(|template| template.field == step.field)
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|template| template.field);
        matches.dedup_by_key(|template| template.field);
        if matches.len() > 1 {
            return Err(format!(
                "P004: field operation `nested bounded deformation` is not available in `AaaByteExact`: feature {} crosses {} deformation templates",
                feature.id,
                matches.len()
            ));
        }
        let Some(template) = matches.first() else {
            if lowered.deformed_predictor {
                return Err(format!(
                    "pixels::deform: feature {} is marked deformed without a closed template",
                    feature.id
                ));
            }
            continue;
        };
        if !lowered.deformed_predictor {
            return Err(format!(
                "pixels::deform: feature {} has a closed template but no deformed predictor",
                feature.id
            ));
        }
        let base_roots = u64::from(lowered.max_root_count);
        let maximum_root_count = base_roots
            .checked_add(
                oscillation_bound(template, values)?
                    .checked_mul(2)
                    .ok_or_else(|| "P015: deformation root bound overflow".to_string())?,
            )
            .ok_or_else(|| "P015: deformation root bound overflow".to_string())?;
        let maximum_root_count = u8::try_from(maximum_root_count).map_err(|_| {
            format!(
                "P015: renderer capacity `deformation_roots` needs {maximum_root_count} roots, ceiling 255"
            )
        })?;
        let remainder = SIN_COS_APPROXIMATION_REMAINDER_V1;
        result.push(ProjectiveDeformationProgram {
            feature: feature.id,
            deformation_field: template.field,
            predictor: lowered.root_equation,
            residual: template.displacement,
            coordinate_x: template.coordinate_x,
            frequency: template.frequency,
            phase: template.phase,
            value_bound: super::reference::interval::next_up(template.amplitude + remainder),
            first_derivative_bound: super::reference::interval::next_up(
                template.gradient + remainder,
            ),
            second_derivative_bound: super::reference::interval::next_up(
                template.hessian + remainder,
            ),
            third_derivative_bound: super::reference::interval::next_up(
                template.third_derivative + remainder,
            ),
            taylor_order: 3,
            approximation: ApproximationContract {
                revision: SIN_COS_APPROXIMATION_REVISION_V1,
                folded_domain: F64Interval::new(
                    -std::f64::consts::FRAC_PI_4,
                    std::f64::consts::FRAC_PI_4,
                )?,
                sine_remainder: remainder,
                cosine_remainder: remainder,
            },
            tube_method: "monotone-krawczyk",
            maximum_root_count,
            phase_recurrence: PhaseRecurrenceProgram {
                coordinate_x: template.coordinate_x,
                frequency_scalar: template.frequency_scalar,
                phase_scalar: template.phase_scalar,
                frequency: template.frequency,
                phase: template.phase,
                sine_coefficients: SIN_MINIMAX_ODD_COEFFICIENTS_V1.map(f64::to_bits),
                cosine_coefficients: COS_MINIMAX_EVEN_COEFFICIENTS_V1.map(f64::to_bits),
            },
        });
    }
    Ok(result)
}

fn validate_sinusoidal_contract_links(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    displacement: ScalarId,
    contract: &super::graph::DerivedDeformContract,
) -> Result<[f64; 4], String> {
    let mut source_displacement = displacement;
    while let ScalarOp::Neg(inner) = graph.scalar.get(source_displacement)?.op {
        source_displacement = inner;
    }
    let ScalarOp::Mul(amplitude, wave) = graph.scalar.get(source_displacement)?.op else {
        return Err(format!(
            "pixels::deform: sinusoidal displacement {displacement} is not amplitude times wave"
        ));
    };
    let ScalarOp::SinRestricted(angle, _) = graph.scalar.get(wave)?.op else {
        return Err(format!(
            "pixels::deform: sinusoidal displacement {displacement} has no source-sine wave"
        ));
    };
    let ScalarOp::Add(frequency_x, phase) = graph.scalar.get(angle)?.op else {
        return Err(format!(
            "pixels::deform: sinusoidal displacement {displacement} has no frequency/phase angle"
        ));
    };
    let ScalarOp::Mul(frequency, coordinate_x) = graph.scalar.get(frequency_x)?.op else {
        return Err(format!(
            "pixels::deform: sinusoidal displacement {displacement} has no frequency/coordinate product"
        ));
    };
    if frequency != contract.frequency
        || coordinate_x != contract.coordinate_x
        || phase != contract.phase
    {
        return Err(format!(
            "pixels::deform: sinusoidal displacement {displacement} contract links do not match its scalar program"
        ));
    }

    let point = |value: f32| F64Interval::point(f64::from(value));
    let amplitude_value = values.get(amplitude)?;
    let frequency_value = values.get(frequency)?;
    let frequency2 = frequency_value.mul_f32(frequency_value)?;
    let frequency3 = frequency2.mul_f32(frequency_value)?;
    let expected = [
        amplitude_value
            .abs_f32()?
            .mul_f32(point(super::scalar::SOURCE_TRIG_VALUE_FACTOR_V2)?)?,
        amplitude_value
            .mul_f32(frequency_value)?
            .abs_f32()?
            .mul_f32(point(super::scalar::SOURCE_TRIG_GRADIENT_FACTOR_V2)?)?,
        amplitude_value
            .mul_f32(frequency2)?
            .abs_f32()?
            .mul_f32(point(super::scalar::SOURCE_TRIG_HESSIAN_FACTOR_V2)?)?,
        amplitude_value
            .mul_f32(frequency3)?
            .abs_f32()?
            .mul_f32(point(super::scalar::SOURCE_TRIG_THIRD_FACTOR_V2)?)?,
    ];
    let declared_ids = [
        contract.amplitude_bound,
        contract.gradient_bound,
        contract.hessian_bound,
        contract.third_derivative_bound,
    ];
    for (id, expected) in declared_ids.iter().copied().zip(expected) {
        let declared = values.get(id)?;
        if declared != expected {
            return Err(format!(
                "pixels::deform: contract scalar {id} has {declared:?}, expected exact closed derivation {expected:?}"
            ));
        }
    }

    let declared = declared_ids.map(|id| values.get(id).map(|value| value.abs_upper()));
    let declared: [f64; 4] = declared
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "pixels::deform: closed contract arity mismatch".to_string())?;
    Ok(declared)
}

pub fn compile(
    graph: &SymbolicGraph,
    values: &ValueBounds,
) -> Result<Vec<DeformationTemplate>, String> {
    let mut templates = Vec::new();
    for (field, node) in graph.fields.iter() {
        let FieldKind::BoundedDisplace {
            displacement,
            contract,
            ..
        } = &node.kind
        else {
            continue;
        };
        let derived = validate_sinusoidal_contract_links(graph, values, *displacement, contract)?;
        if derived.iter().any(|value| !value.is_finite()) {
            return Err(format!(
                "P013: deformation `sinusoidal_displace` lacks a conservative amplitude/derivative contract at {field}"
            ));
        }
        templates.push(DeformationTemplate {
            field,
            displacement: *displacement,
            derivation: contract.derivation,
            amplitude: derived[0],
            gradient: derived[1],
            hessian: derived[2],
            third_derivative: derived[3],
            coordinate_x: contract.coordinate_x,
            frequency_scalar: contract.frequency,
            phase_scalar: contract.phase,
            frequency: values.get(contract.frequency)?,
            phase: values.get(contract.phase)?,
        });
    }
    Ok(templates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimax_remainder_encloses_folded_sine_and_cosine() {
        let (sine_certificate, cosine_certificate) =
            certified_sin_cos_remainders().expect("analytic complete-domain certificate");
        assert!(sine_certificate <= SIN_COS_APPROXIMATION_REMAINDER_V1);
        assert!(cosine_certificate <= SIN_COS_APPROXIMATION_REMAINDER_V1);
        let mut corrupted = SIN_MINIMAX_ODD_COEFFICIENTS_V1;
        corrupted[3] += 1.0e-8;
        assert!(
            folded_polynomial_error_bound(&corrupted, true).unwrap()
                > SIN_COS_APPROXIMATION_REMAINDER_V1,
            "the analytic certificate must reject a between-sample coefficient regression"
        );

        // Deterministic point samples remain only as an independent bug
        // finder; the production gate above proves the complete folded box.
        let evaluate = |coefficients: &[f64], x2: f64| {
            coefficients
                .iter()
                .rev()
                .fold(0.0, |value, coefficient| value * x2 + coefficient)
        };
        for step in 0..=4096 {
            let x = -std::f64::consts::FRAC_PI_4
                + std::f64::consts::FRAC_PI_2 * f64::from(step) / 4096.0;
            let x2 = x * x;
            let sine = x * evaluate(&SIN_MINIMAX_ODD_COEFFICIENTS_V1, x2);
            let cosine = evaluate(&COS_MINIMAX_EVEN_COEFFICIENTS_V1, x2);
            assert!(
                (sine - x.sin()).abs() <= SIN_COS_APPROXIMATION_REMAINDER_V1,
                "sine error at {x}"
            );
            assert!(
                (cosine - x.cos()).abs() <= SIN_COS_APPROXIMATION_REMAINDER_V1,
                "cosine error at {x}"
            );
        }
    }
}
