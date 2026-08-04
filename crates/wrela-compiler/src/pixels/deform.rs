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
    pub frequency: super::reference::interval::F64Interval,
    pub phase: super::reference::interval::F64Interval,
}

fn validate_sinusoidal_contract_links(
    graph: &SymbolicGraph,
    values: &ValueBounds,
    displacement: ScalarId,
    contract: &super::graph::DerivedDeformContract,
) -> Result<[f64; 4], String> {
    let ScalarOp::Mul(amplitude, wave) = graph.scalar.get(displacement)?.op else {
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
            .mul_f32(point(super::scalar::SOURCE_TRIG_VALUE_FACTOR_V1)?)?,
        amplitude_value
            .mul_f32(frequency_value)?
            .abs_f32()?
            .mul_f32(point(super::scalar::SOURCE_TRIG_GRADIENT_FACTOR_V1)?)?,
        amplitude_value
            .mul_f32(frequency2)?
            .abs_f32()?
            .mul_f32(point(super::scalar::SOURCE_TRIG_HESSIAN_FACTOR_V1)?)?,
        amplitude_value
            .mul_f32(frequency3)?
            .abs_f32()?
            .mul_f32(point(super::scalar::SOURCE_TRIG_THIRD_FACTOR_V1)?)?,
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
            frequency: values.get(contract.frequency)?,
            phase: values.get(contract.phase)?,
        });
    }
    Ok(templates)
}
