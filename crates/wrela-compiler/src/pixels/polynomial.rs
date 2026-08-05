//! Deterministic bounded sparse polynomial programs.

use std::collections::BTreeMap;

use super::ids::{CoeffId, ParamId, PolyProgramId};
use super::program::{CoeffBuilder, CoeffProgram};

pub const MAX_PARAM_EXPONENTS: usize = 8;
pub const MAX_TERMS_PER_PROGRAM_V1: usize = 512;
pub const MAX_COMPOSITION_TEMPORARIES_V1: u16 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Var {
    U,
    V,
    Q,
    X,
    T,
    Param(ParamId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ParamExponent {
    pub param: ParamId,
    pub exponent: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SmallFixedParamExponents {
    pub terms: [ParamExponent; MAX_PARAM_EXPONENTS],
    pub count: u8,
}

impl Default for SmallFixedParamExponents {
    fn default() -> Self {
        Self {
            terms: [ParamExponent {
                param: ParamId(0),
                exponent: 0,
            }; MAX_PARAM_EXPONENTS],
            count: 0,
        }
    }
}

impl SmallFixedParamExponents {
    pub fn one(param: ParamId, exponent: u8) -> Result<Self, String> {
        let mut value = Self::default();
        if exponent != 0 {
            value.terms[0] = ParamExponent { param, exponent };
            value.count = 1;
        }
        Ok(value)
    }

    pub fn iter(&self) -> impl Iterator<Item = ParamExponent> + '_ {
        self.terms[..usize::from(self.count)].iter().copied()
    }

    fn checked_mul(self, other: Self) -> Result<Self, String> {
        let mut merged = BTreeMap::<ParamId, u8>::new();
        for term in self.iter().chain(other.iter()) {
            let next = merged
                .get(&term.param)
                .copied()
                .unwrap_or(0)
                .checked_add(term.exponent)
                .ok_or_else(|| "P004: polynomial parameter degree overflow".to_string())?;
            merged.insert(term.param, next);
        }
        if merged.len() > MAX_PARAM_EXPONENTS {
            return Err(format!(
                "P015: renderer capacity `polynomial_parameter_terms` needs {}, ceiling {}",
                merged.len(),
                MAX_PARAM_EXPONENTS
            ));
        }
        let mut result = Self::default();
        result.count = u8::try_from(merged.len())
            .map_err(|_| "P015: polynomial parameter exponent count overflow".to_string())?;
        for (slot, (param, exponent)) in result.terms.iter_mut().zip(merged) {
            *slot = ParamExponent { param, exponent };
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Exponents {
    pub u: u8,
    pub v: u8,
    pub q: u8,
    pub x: u8,
    pub t: u8,
    pub param_terms: SmallFixedParamExponents,
}

impl Exponents {
    pub fn one(variable: Var) -> Result<Self, String> {
        let mut result = Self::default();
        match variable {
            Var::U => result.u = 1,
            Var::V => result.v = 1,
            Var::Q => result.q = 1,
            Var::X => result.x = 1,
            Var::T => result.t = 1,
            Var::Param(param) => {
                result.param_terms = SmallFixedParamExponents::one(param, 1)?;
            }
        }
        Ok(result)
    }

    fn checked_mul(self, other: Self) -> Result<Self, String> {
        Ok(Self {
            u: self
                .u
                .checked_add(other.u)
                .ok_or_else(|| "P004: polynomial u degree overflow".to_string())?,
            v: self
                .v
                .checked_add(other.v)
                .ok_or_else(|| "P004: polynomial v degree overflow".to_string())?,
            q: self
                .q
                .checked_add(other.q)
                .ok_or_else(|| "P004: polynomial q degree overflow".to_string())?,
            x: self
                .x
                .checked_add(other.x)
                .ok_or_else(|| "P004: polynomial x degree overflow".to_string())?,
            t: self
                .t
                .checked_add(other.t)
                .ok_or_else(|| "P004: polynomial t degree overflow".to_string())?,
            param_terms: self.param_terms.checked_mul(other.param_terms)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolyTerm {
    pub exponents: Exponents,
    pub coefficient: CoeffId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolyProgram {
    pub id: PolyProgramId,
    pub terms: Vec<PolyTerm>,
    pub degree_u: u8,
    pub degree_v: u8,
    pub degree_q: u8,
    pub degree_x: u8,
    pub degree_t: u8,
    pub coefficient_program: u32,
}

impl PolyProgram {
    pub fn direct_sum(
        &self,
        coefficients: &CoeffProgram,
        coeff: &impl Fn(CoeffId) -> Result<f64, String>,
        variables: &impl Fn(Var) -> Result<f64, String>,
    ) -> Result<f64, String> {
        let mut sum = 0.0;
        for term in &self.terms {
            coefficients.get(term.coefficient)?;
            let mut value = coeff(term.coefficient)?;
            value *= variables(Var::U)?.powi(i32::from(term.exponents.u));
            value *= variables(Var::V)?.powi(i32::from(term.exponents.v));
            value *= variables(Var::Q)?.powi(i32::from(term.exponents.q));
            value *= variables(Var::X)?.powi(i32::from(term.exponents.x));
            value *= variables(Var::T)?.powi(i32::from(term.exponents.t));
            for param in term.exponents.param_terms.iter() {
                value *= variables(Var::Param(param.param))?.powi(i32::from(param.exponent));
            }
            sum += value;
        }
        Ok(sum)
    }

    pub fn horner_q(
        &self,
        coefficients: &CoeffProgram,
        coeff: &impl Fn(CoeffId) -> Result<f64, String>,
        variables: &impl Fn(Var) -> Result<f64, String>,
    ) -> Result<f64, String> {
        let mut by_q = BTreeMap::<u8, f64>::new();
        for term in &self.terms {
            coefficients.get(term.coefficient)?;
            let mut value = coeff(term.coefficient)?;
            value *= variables(Var::U)?.powi(i32::from(term.exponents.u));
            value *= variables(Var::V)?.powi(i32::from(term.exponents.v));
            value *= variables(Var::X)?.powi(i32::from(term.exponents.x));
            value *= variables(Var::T)?.powi(i32::from(term.exponents.t));
            for param in term.exponents.param_terms.iter() {
                value *= variables(Var::Param(param.param))?.powi(i32::from(param.exponent));
            }
            *by_q.entry(term.exponents.q).or_default() += value;
        }
        let q = variables(Var::Q)?;
        let mut result = 0.0;
        for degree in (0..=self.degree_q).rev() {
            result = result * q + by_q.get(&degree).copied().unwrap_or(0.0);
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramLimit {
    Feature,
    EventBivariate,
    EventUnivariate,
    UnrestrictedIntermediate,
}

#[derive(Clone, Debug)]
pub struct PolyBuilder {
    terms: BTreeMap<Exponents, CoeffId>,
}

impl PolyBuilder {
    pub fn zero() -> Self {
        Self {
            terms: BTreeMap::new(),
        }
    }

    pub fn from_program(program: &PolyProgram) -> Self {
        Self {
            terms: program
                .terms
                .iter()
                .map(|term| (term.exponents, term.coefficient))
                .collect(),
        }
    }

    pub fn constant(coefficient: CoeffId, coefficients: &CoeffProgram) -> Self {
        let mut result = Self::zero();
        if !coefficients.is_exact_zero(coefficient) {
            result.terms.insert(Exponents::default(), coefficient);
        }
        result
    }

    pub fn variable(
        coefficient: CoeffId,
        variable: Var,
        coefficients: &CoeffProgram,
    ) -> Result<Self, String> {
        let mut result = Self::zero();
        if !coefficients.is_exact_zero(coefficient) {
            result.terms.insert(Exponents::one(variable)?, coefficient);
        }
        Ok(result)
    }

    pub fn monomial(
        coefficient: CoeffId,
        exponents: Exponents,
        coefficients: &CoeffProgram,
    ) -> Self {
        let mut result = Self::zero();
        if !coefficients.is_exact_zero(coefficient) {
            result.terms.insert(exponents, coefficient);
        }
        result
    }

    pub fn add(self, other: Self, coefficients: &mut CoeffBuilder) -> Result<Self, String> {
        let mut terms = self.terms;
        for (exponents, coefficient) in other.terms {
            if let Some(previous) = terms.remove(&exponents) {
                let combined = coefficients.add(previous, coefficient)?;
                if !coefficients.program().is_exact_zero(combined) {
                    terms.insert(exponents, combined);
                }
            } else {
                terms.insert(exponents, coefficient);
            }
        }
        check_term_ceiling(terms.len())?;
        Ok(Self { terms })
    }

    pub fn neg(self, coefficients: &mut CoeffBuilder) -> Result<Self, String> {
        let terms = self
            .terms
            .into_iter()
            .map(|(exponents, coefficient)| {
                coefficients
                    .neg(coefficient)
                    .map(|coefficient| (exponents, coefficient))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        Ok(Self { terms })
    }

    pub fn sub(self, other: Self, coefficients: &mut CoeffBuilder) -> Result<Self, String> {
        let other = other.neg(coefficients)?;
        self.add(other, coefficients)
    }

    pub fn mul(self, other: Self, coefficients: &mut CoeffBuilder) -> Result<Self, String> {
        let mut terms = BTreeMap::new();
        for (a_exponents, a_coefficient) in &self.terms {
            for (b_exponents, b_coefficient) in &other.terms {
                let exponents = a_exponents.checked_mul(*b_exponents)?;
                let coefficient = coefficients.mul(*a_coefficient, *b_coefficient)?;
                if coefficients.program().is_exact_zero(coefficient) {
                    continue;
                }
                if let Some(previous) = terms.remove(&exponents) {
                    let combined = coefficients.add(previous, coefficient)?;
                    if !coefficients.program().is_exact_zero(combined) {
                        terms.insert(exponents, combined);
                    }
                } else {
                    terms.insert(exponents, coefficient);
                }
                check_term_ceiling(terms.len())?;
            }
        }
        Ok(Self { terms })
    }

    pub fn scale(self, scale: CoeffId, coefficients: &mut CoeffBuilder) -> Result<Self, String> {
        if coefficients.program().is_exact_zero(scale) {
            return Ok(Self::zero());
        }
        let constant = Self::constant(scale, coefficients.program());
        self.mul(constant, coefficients)
    }

    pub fn pow(self, mut exponent: u8, coefficients: &mut CoeffBuilder) -> Result<Self, String> {
        let one = coefficients.constant(1.0)?;
        let mut result = Self::constant(one, coefficients.program());
        let mut base = self;
        while exponent != 0 {
            if exponent & 1 == 1 {
                result = result.mul(base.clone(), coefficients)?;
            }
            exponent >>= 1;
            if exponent != 0 {
                base = base.clone().mul(base, coefficients)?;
            }
        }
        Ok(result)
    }

    pub fn differentiate(
        &self,
        variable: Var,
        coefficients: &mut CoeffBuilder,
    ) -> Result<Self, String> {
        let mut terms = BTreeMap::new();
        for (exponents, coefficient) in &self.terms {
            let mut derived = *exponents;
            let power = match variable {
                Var::U => {
                    let value = derived.u;
                    derived.u = derived.u.saturating_sub(1);
                    value
                }
                Var::V => {
                    let value = derived.v;
                    derived.v = derived.v.saturating_sub(1);
                    value
                }
                Var::Q => {
                    let value = derived.q;
                    derived.q = derived.q.saturating_sub(1);
                    value
                }
                Var::X => {
                    let value = derived.x;
                    derived.x = derived.x.saturating_sub(1);
                    value
                }
                Var::T => {
                    let value = derived.t;
                    derived.t = derived.t.saturating_sub(1);
                    value
                }
                Var::Param(param) => {
                    let mut power = 0;
                    let mut updated = SmallFixedParamExponents::default();
                    for term in exponents.param_terms.iter() {
                        let exponent = if term.param == param {
                            power = term.exponent;
                            term.exponent.saturating_sub(1)
                        } else {
                            term.exponent
                        };
                        if exponent != 0 {
                            updated.terms[usize::from(updated.count)] = ParamExponent {
                                param: term.param,
                                exponent,
                            };
                            updated.count += 1;
                        }
                    }
                    derived.param_terms = updated;
                    power
                }
            };
            if power == 0 {
                continue;
            }
            let multiplier = coefficients.constant(f64::from(power))?;
            let coefficient = coefficients.mul(*coefficient, multiplier)?;
            terms.insert(derived, coefficient);
        }
        Ok(Self { terms })
    }

    pub fn map_coefficients(
        &self,
        mut map: impl FnMut(CoeffId) -> Result<CoeffId, String>,
        coefficients: &CoeffProgram,
    ) -> Result<Self, String> {
        let mut terms = BTreeMap::new();
        for (exponents, coefficient) in &self.terms {
            let coefficient = map(*coefficient)?;
            if !coefficients.is_exact_zero(coefficient) {
                terms.insert(*exponents, coefficient);
            }
        }
        Ok(Self { terms })
    }

    pub fn finish(self, id: PolyProgramId, limit: ProgramLimit) -> Result<PolyProgram, String> {
        check_term_ceiling(self.terms.len())?;
        let degree_u = self.terms.keys().map(|term| term.u).max().unwrap_or(0);
        let degree_v = self.terms.keys().map(|term| term.v).max().unwrap_or(0);
        let degree_q = self.terms.keys().map(|term| term.q).max().unwrap_or(0);
        let degree_x = self.terms.keys().map(|term| term.x).max().unwrap_or(0);
        let degree_t = self.terms.keys().map(|term| term.t).max().unwrap_or(0);
        match limit {
            ProgramLimit::Feature if degree_q > 4 => {
                return Err(format!(
                    "P004: field operation `projective feature` is not available in `AaaByteExact`: q degree {degree_q} exceeds 4"
                ));
            }
            ProgramLimit::EventBivariate if degree_u > 6 || degree_v > 6 => {
                return Err(format!(
                    "P004: field operation `projective event` is not available in `AaaByteExact`: bivariate degree ({degree_u},{degree_v}) exceeds (6,6)"
                ));
            }
            ProgramLimit::EventUnivariate if degree_x > 8 => {
                return Err(format!(
                    "P004: field operation `projective event` is not available in `AaaByteExact`: univariate degree {degree_x} exceeds 8"
                ));
            }
            _ => {}
        }
        let terms = self
            .terms
            .into_iter()
            // BTreeMap is ascending. Stable dumps and encoding require
            // q/u/v/t/parameter degree descending.
            .map(|(exponents, coefficient)| PolyTerm {
                exponents,
                coefficient,
            })
            .collect::<Vec<_>>();
        let mut terms = terms;
        terms.sort_by(|a, b| {
            (
                std::cmp::Reverse(a.exponents.q),
                std::cmp::Reverse(a.exponents.u),
                std::cmp::Reverse(a.exponents.v),
                std::cmp::Reverse(a.exponents.x),
                std::cmp::Reverse(a.exponents.t),
                a.exponents.param_terms,
            )
                .cmp(&(
                    std::cmp::Reverse(b.exponents.q),
                    std::cmp::Reverse(b.exponents.u),
                    std::cmp::Reverse(b.exponents.v),
                    std::cmp::Reverse(b.exponents.x),
                    std::cmp::Reverse(b.exponents.t),
                    b.exponents.param_terms,
                ))
        });
        Ok(PolyProgram {
            id,
            terms,
            degree_u,
            degree_v,
            degree_q,
            degree_x,
            degree_t,
            coefficient_program: 0,
        })
    }
}

fn check_term_ceiling(count: usize) -> Result<(), String> {
    if count > MAX_TERMS_PER_PROGRAM_V1 {
        return Err(format!(
            "P015: renderer capacity `polynomial_terms` needs {count} terms, ceiling {MAX_TERMS_PER_PROGRAM_V1}"
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompositionStep {
    pub source_term: u16,
    pub u_power: u8,
    pub q_power: u8,
    pub lifted_power_offset: u16,
    pub coefficient_order: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrectionFaceSchedule {
    pub correction_sign: i8,
    pub output_coefficient_order: Vec<u8>,
    pub steps: Vec<CompositionStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompositionSchedule {
    pub source_degree_u: u8,
    pub source_degree_q: u8,
    pub source_degree_x: u8,
    pub q_hat_degree: u8,
    pub correction_face_count: u8,
    pub composed_degree: u8,
    pub source_term_count: u16,
    pub temporary_count: u16,
    pub coefficient_order: Vec<u16>,
    pub steps: Vec<CompositionStep>,
    pub correction_faces: Vec<CorrectionFaceSchedule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositionPlan {
    Specialized(CompositionSchedule),
    IntervalTaylorFallback {
        source_degree_u: u8,
        source_degree_q: u8,
        source_degree_x: u8,
        composed_degree: u16,
        source_term_count: u16,
    },
}

pub fn plan_quadratic_composition(program: &PolyProgram) -> Result<CompositionPlan, String> {
    // A run is parameterized by local scanline X, with U = u0 + X. Account
    // for both the authored U power and any already-local X power in each
    // source monomial rather than treating U as a frozen coefficient.
    let composed_degree = program.terms.iter().try_fold(0_u16, |maximum, term| {
        let degree = u16::from(term.exponents.u)
            .checked_add(u16::from(term.exponents.x))
            .and_then(|value| value.checked_add(u16::from(term.exponents.q) * 2))
            .ok_or_else(|| "P015: composition degree arithmetic overflow".to_string())?;
        Ok::<u16, String>(maximum.max(degree))
    })?;
    let source_term_count = u16::try_from(program.terms.len())
        .map_err(|_| "P015: composition source term count overflow".to_string())?;
    let temporary_count = u16::from(program.degree_q)
        .checked_add(1)
        .and_then(|q| {
            u16::from(program.degree_u)
                .checked_add(1)
                .and_then(|u| q.checked_mul(u))
        })
        .and_then(|value| value.checked_add(source_term_count))
        .ok_or_else(|| "P015: composition temporary count overflow".to_string())?;
    if composed_degree > 8
        || temporary_count > MAX_COMPOSITION_TEMPORARIES_V1
        || program.terms.len() > MAX_TERMS_PER_PROGRAM_V1
    {
        return Ok(CompositionPlan::IntervalTaylorFallback {
            source_degree_u: program.degree_u,
            source_degree_q: program.degree_q,
            source_degree_x: program.degree_x,
            composed_degree,
            source_term_count,
        });
    }
    let coefficient_order = (0..source_term_count).collect::<Vec<_>>();
    let steps = program
        .terms
        .iter()
        .enumerate()
        .map(|(index, term)| {
            let source_term = u16::try_from(index)
                .map_err(|_| "P015: composition step index overflow".to_string())?;
            Ok(CompositionStep {
                source_term,
                u_power: term.exponents.u,
                q_power: term.exponents.q,
                lifted_power_offset: u16::from(term.exponents.q)
                    * (u16::from(program.degree_u) + 1)
                    + u16::from(term.exponents.u),
                coefficient_order: source_term,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let output_coefficient_order = (0..=u8::try_from(composed_degree)
        .map_err(|_| "P015: specialized composition degree overflow".to_string())?)
        .collect::<Vec<_>>();
    let correction_faces = [-1_i8, 1_i8]
        .into_iter()
        .map(|correction_sign| CorrectionFaceSchedule {
            correction_sign,
            output_coefficient_order: output_coefficient_order.clone(),
            steps: steps.clone(),
        })
        .collect();
    Ok(CompositionPlan::Specialized(CompositionSchedule {
        source_degree_u: program.degree_u,
        source_degree_q: program.degree_q,
        source_degree_x: program.degree_x,
        q_hat_degree: 2,
        correction_face_count: 2,
        composed_degree: u8::try_from(composed_degree)
            .map_err(|_| "P015: specialized composition degree overflow".to_string())?,
        source_term_count,
        temporary_count,
        coefficient_order,
        steps,
        correction_faces,
    }))
}

fn convolve(a: &[f64], b: &[f64]) -> Result<Vec<f64>, String> {
    let mut result = vec![0.0; a.len() + b.len() - 1];
    for (a_degree, a) in a.iter().copied().enumerate() {
        for (b_degree, b) in b.iter().copied().enumerate() {
            let value = a * b;
            let slot = result
                .get_mut(a_degree + b_degree)
                .ok_or_else(|| "pixels::polynomial: convolution index overflow".to_string())?;
            *slot += value;
            if !slot.is_finite() {
                return Err("pixels::polynomial: non-finite composition coefficient".to_string());
            }
        }
    }
    Ok(result)
}

pub fn quadratic_correction_face_coefficients(
    program: &PolyProgram,
    schedule: &CompositionSchedule,
    face_index: usize,
    coefficients: &CoeffProgram,
    coefficient: &impl Fn(CoeffId) -> Result<f64, String>,
    variables: &impl Fn(Var) -> Result<f64, String>,
    q_hat: [f64; 3],
    correction: f64,
) -> Result<Vec<f64>, String> {
    let face = schedule
        .correction_faces
        .get(face_index)
        .ok_or_else(|| "pixels::polynomial: missing correction face".to_string())?;
    if face.steps != schedule.steps
        || face.output_coefficient_order != (0..=schedule.composed_degree).collect::<Vec<_>>()
        || !matches!(face.correction_sign, -1 | 1)
        || schedule.source_degree_u != program.degree_u
        || schedule.source_degree_q != program.degree_q
        || schedule.source_degree_x != program.degree_x
    {
        return Err("pixels::polynomial: malformed correction-face schedule".to_string());
    }
    let mut output = vec![0.0; usize::from(schedule.composed_degree) + 1];
    let q = [
        q_hat[0] + f64::from(face.correction_sign) * correction,
        q_hat[1],
        q_hat[2],
    ];
    let u = [variables(Var::U)?, 1.0];
    for step in &face.steps {
        let term = program
            .terms
            .get(usize::from(step.source_term))
            .ok_or_else(|| "pixels::polynomial: correction face names missing term".to_string())?;
        let expected_offset = u16::from(term.exponents.q)
            * (u16::from(schedule.source_degree_u) + 1)
            + u16::from(term.exponents.u);
        if step.u_power != term.exponents.u
            || step.q_power != term.exponents.q
            || step.lifted_power_offset != expected_offset
            || step.coefficient_order != step.source_term
        {
            return Err(
                "pixels::polynomial: correction step u/q degree differs from source".to_string(),
            );
        }
        let mut q_power = vec![1.0];
        for _ in 0..term.exponents.q {
            q_power = convolve(&q_power, &q)?;
        }
        let mut u_power = vec![1.0];
        for _ in 0..term.exponents.u {
            u_power = convolve(&u_power, &u)?;
        }
        let lifted = convolve(&q_power, &u_power)?;
        let mut scale = coefficient(term.coefficient)?;
        coefficients.get(term.coefficient)?;
        scale *= variables(Var::V)?.powi(i32::from(term.exponents.v));
        scale *= variables(Var::T)?.powi(i32::from(term.exponents.t));
        for param in term.exponents.param_terms.iter() {
            scale *= variables(Var::Param(param.param))?.powi(i32::from(param.exponent));
        }
        for (degree, value) in lifted.into_iter().enumerate() {
            let output_degree = degree
                .checked_add(usize::from(term.exponents.x))
                .ok_or_else(|| "P015: correction-face output degree overflow".to_string())?;
            let slot = output.get_mut(output_degree).ok_or_else(|| {
                "pixels::polynomial: correction-face degree exceeds schedule".to_string()
            })?;
            *slot += scale * value;
        }
    }
    if output.iter().any(|value| !value.is_finite()) {
        return Err("pixels::polynomial: non-finite correction-face coefficient".to_string());
    }
    Ok(output)
}

pub fn evaluate_quadratic_composition(
    program: &PolyProgram,
    schedule: &CompositionSchedule,
    coefficients: &CoeffProgram,
    coefficient: &impl Fn(CoeffId) -> Result<f64, String>,
    variables: &impl Fn(Var) -> Result<f64, String>,
    x: f64,
    q_hat: [f64; 3],
) -> Result<f64, String> {
    if schedule.source_term_count as usize != program.terms.len()
        || schedule.steps.len() != program.terms.len()
        || schedule.correction_face_count != 2
        || schedule.correction_faces.len() != 2
        || schedule.source_degree_u != program.degree_u
        || schedule.source_degree_q != program.degree_q
        || schedule.source_degree_x != program.degree_x
    {
        return Err("pixels::polynomial: composition schedule shape mismatch".to_string());
    }
    let q = q_hat[0] + x * (q_hat[1] + x * q_hat[2]);
    let u = variables(Var::U)? + x;
    let mut sum = 0.0;
    for step in &schedule.steps {
        let term = program
            .terms
            .get(usize::from(step.source_term))
            .ok_or_else(|| "pixels::polynomial: composition step names missing term".to_string())?;
        let expected_offset = u16::from(term.exponents.q)
            * (u16::from(schedule.source_degree_u) + 1)
            + u16::from(term.exponents.u);
        if step.u_power != term.exponents.u
            || step.q_power != term.exponents.q
            || step.lifted_power_offset != expected_offset
            || step.coefficient_order != step.source_term
        {
            return Err(
                "pixels::polynomial: composition step u/q degree differs from source".to_string(),
            );
        }
        coefficients.get(term.coefficient)?;
        let mut value = coefficient(term.coefficient)?;
        value *= u.powi(i32::from(term.exponents.u));
        value *= variables(Var::V)?.powi(i32::from(term.exponents.v));
        value *= q.powi(i32::from(term.exponents.q));
        value *= x.powi(i32::from(term.exponents.x));
        value *= variables(Var::T)?.powi(i32::from(term.exponents.t));
        for param in term.exponents.param_terms.iter() {
            value *= variables(Var::Param(param.param))?.powi(i32::from(param.exponent));
        }
        sum += value;
    }
    if !sum.is_finite() {
        return Err("pixels::polynomial: composition evaluation is non-finite".to_string());
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(order: &[Var]) -> (CoeffProgram, PolyProgram) {
        let mut coefficients = CoeffBuilder::new();
        let one = coefficients.constant(1.0).unwrap();
        let mut polynomial = PolyBuilder::zero();
        for variable in order {
            let term = PolyBuilder::variable(one, *variable, coefficients.program()).unwrap();
            polynomial = polynomial.add(term, &mut coefficients).unwrap();
        }
        let polynomial = polynomial
            .finish(PolyProgramId(0), ProgramLimit::Feature)
            .unwrap();
        (coefficients.finish(), polynomial)
    }

    #[test]
    fn construction_order_canonicalizes_identically() {
        assert_eq!(
            canonical(&[Var::Q, Var::U, Var::V]),
            canonical(&[Var::V, Var::Q, Var::U])
        );
    }

    #[test]
    fn equal_monomial_sums_ignore_grouping_even_under_cancellation() {
        fn build(left_grouped: bool) -> (CoeffProgram, CoeffId) {
            let mut coefficients = CoeffBuilder::new();
            let values = [1.0e16, 1.0, -1.0e16].map(|value| coefficients.constant(value).unwrap());
            let terms = values.map(|coefficient| {
                PolyBuilder::variable(coefficient, Var::Q, coefficients.program()).unwrap()
            });
            let polynomial = if left_grouped {
                terms[0]
                    .clone()
                    .add(terms[1].clone(), &mut coefficients)
                    .unwrap()
                    .add(terms[2].clone(), &mut coefficients)
                    .unwrap()
            } else {
                terms[0]
                    .clone()
                    .add(
                        terms[1]
                            .clone()
                            .add(terms[2].clone(), &mut coefficients)
                            .unwrap(),
                        &mut coefficients,
                    )
                    .unwrap()
            }
            .finish(PolyProgramId(0), ProgramLimit::Feature)
            .unwrap();
            let root = polynomial.terms[0].coefficient;
            let program = coefficients.finish();
            let (program, remap) = program.canonicalize_reachable([root]).unwrap();
            (program, remap[root.index()].unwrap())
        }

        let (left, left_root) = build(true);
        let (right, right_root) = build(false);
        assert_eq!(left, right);
        assert_eq!(left_root, right_root);
        let left_value = left
            .evaluate(&|_| unreachable!(), &|_| unreachable!())
            .unwrap()[left_root.index()];
        assert_eq!(left_value.to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn direct_sum_and_horner_agree() {
        let (coefficients, program) = canonical(&[Var::Q, Var::U, Var::V]);
        let coefficient = |id| {
            coefficients
                .exact_constant(id)
                .ok_or_else(|| "not constant".to_string())
        };
        let variables = |variable| {
            Ok(match variable {
                Var::U => 0.25,
                Var::V => -0.5,
                Var::Q => 2.0,
                _ => 0.0,
            })
        };
        assert_eq!(
            program
                .direct_sum(&coefficients, &coefficient, &variables)
                .unwrap(),
            program
                .horner_q(&coefficients, &coefficient, &variables)
                .unwrap()
        );
    }

    #[test]
    fn symbolic_derivatives_are_exact_and_mixed_partials_match() {
        let mut coefficients = CoeffBuilder::new();
        let one = coefficients.constant(1.0).unwrap();
        let u = PolyBuilder::variable(one, Var::U, coefficients.program()).unwrap();
        let v = PolyBuilder::variable(one, Var::V, coefficients.program()).unwrap();
        let q = PolyBuilder::variable(one, Var::Q, coefficients.program()).unwrap();
        let product = u
            .mul(v, &mut coefficients)
            .unwrap()
            .mul(q, &mut coefficients)
            .unwrap();
        let uv = product
            .differentiate(Var::U, &mut coefficients)
            .unwrap()
            .differentiate(Var::V, &mut coefficients)
            .unwrap()
            .finish(PolyProgramId(0), ProgramLimit::UnrestrictedIntermediate)
            .unwrap();
        let vu = product
            .differentiate(Var::V, &mut coefficients)
            .unwrap()
            .differentiate(Var::U, &mut coefficients)
            .unwrap()
            .finish(PolyProgramId(0), ProgramLimit::UnrestrictedIntermediate)
            .unwrap();
        assert_eq!(uv, vu);
    }

    #[test]
    fn composition_falls_back_without_rejecting_source() {
        let mut coefficients = CoeffBuilder::new();
        let one = coefficients.constant(1.0).unwrap();
        let polynomial = PolyBuilder::monomial(
            one,
            Exponents {
                q: 4,
                x: 1,
                ..Exponents::default()
            },
            coefficients.program(),
        )
        .finish(PolyProgramId(0), ProgramLimit::Feature)
        .unwrap();
        assert!(matches!(
            plan_quadratic_composition(&polynomial).unwrap(),
            CompositionPlan::IntervalTaylorFallback {
                composed_degree: 9,
                ..
            }
        ));
    }

    #[test]
    fn every_supported_composition_shape_matches_direct_substitution() {
        for q_degree in 0..=4 {
            for u_degree in 0..=8 - 2 * q_degree {
                for x_degree in 0..=8 - 2 * q_degree - u_degree {
                    let mut coefficients = CoeffBuilder::new();
                    let one = coefficients.constant(1.0).unwrap();
                    let two = coefficients.constant(2.0).unwrap();
                    let first = PolyBuilder::monomial(
                        one,
                        Exponents {
                            u: u_degree,
                            q: q_degree,
                            x: x_degree,
                            ..Exponents::default()
                        },
                        coefficients.program(),
                    );
                    let second = PolyBuilder::monomial(
                        two,
                        Exponents {
                            u: u_degree.saturating_sub(1),
                            q: q_degree.saturating_sub(1),
                            x: x_degree.saturating_sub(1),
                            ..Exponents::default()
                        },
                        coefficients.program(),
                    );
                    let polynomial = first
                        .add(second, &mut coefficients)
                        .unwrap()
                        .finish(PolyProgramId(0), ProgramLimit::UnrestrictedIntermediate)
                        .unwrap();
                    let CompositionPlan::Specialized(schedule) =
                        plan_quadratic_composition(&polynomial).unwrap()
                    else {
                        panic!("supported degree shape selected fallback")
                    };
                    let coefficients = coefficients.finish();
                    let coefficient_values =
                        coefficients.evaluate(&|_| Ok(0.0), &|_| Ok(0.0)).unwrap();
                    let coefficient = |id: CoeffId| Ok(coefficient_values[id.index()]);
                    let other = |variable| {
                        Ok(match variable {
                            // The composition API receives the scanline origin u0.
                            Var::U => 0.25,
                            Var::V => -0.5,
                            Var::T | Var::Param(_) => 0.125,
                            Var::Q | Var::X => {
                                return Err("q/x supplied by composition".to_string());
                            }
                        })
                    };
                    let q_hat = [0.75, -0.25, 0.125];
                    for x in [-0.75, 0.0, 0.5] {
                        let composed = evaluate_quadratic_composition(
                            &polynomial,
                            &schedule,
                            &coefficients,
                            &coefficient,
                            &other,
                            x,
                            q_hat,
                        )
                        .unwrap();
                        let q = q_hat[0] + x * (q_hat[1] + x * q_hat[2]);
                        let direct = polynomial
                            .direct_sum(&coefficients, &coefficient, &|variable| {
                                Ok(match variable {
                                    Var::Q => q,
                                    Var::U => other(Var::U)? + x,
                                    Var::X => x,
                                    _ => other(variable)?,
                                })
                            })
                            .unwrap();
                        assert_eq!(composed, direct);
                        for (face_index, sign) in [-1.0_f64, 1.0].into_iter().enumerate() {
                            let correction = 0.125;
                            let face = quadratic_correction_face_coefficients(
                                &polynomial,
                                &schedule,
                                face_index,
                                &coefficients,
                                &coefficient,
                                &other,
                                q_hat,
                                correction,
                            )
                            .unwrap();
                            let face_value = face
                                .iter()
                                .rev()
                                .fold(0.0, |value, coefficient| value * x + coefficient);
                            let direct_face = polynomial
                                .direct_sum(&coefficients, &coefficient, &|variable| {
                                    Ok(match variable {
                                        Var::Q => q + sign * correction,
                                        Var::U => other(Var::U)? + x,
                                        Var::X => x,
                                        _ => other(variable)?,
                                    })
                                })
                                .unwrap();
                            assert!(
                                (face_value - direct_face).abs()
                                    <= 64.0 * f64::EPSILON * direct_face.abs().max(1.0),
                                "face {face_index} differs for q degree {q_degree}, x degree {x_degree}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn degree_limit_fails_closed() {
        let mut coefficients = CoeffBuilder::new();
        let one = coefficients.constant(1.0).unwrap();
        let polynomial = PolyBuilder::monomial(
            one,
            Exponents {
                q: 5,
                ..Exponents::default()
            },
            coefficients.program(),
        );
        assert!(
            polynomial
                .finish(PolyProgramId(0), ProgramLimit::Feature)
                .unwrap_err()
                .starts_with("P004:")
        );
    }

    #[test]
    fn term_ceiling_accepts_512_and_rejects_513_without_truncation() {
        let mut coefficients = CoeffBuilder::new();
        let one = coefficients.constant(1.0).unwrap();
        let mut at_ceiling = PolyBuilder::zero();
        for index in 0..MAX_TERMS_PER_PROGRAM_V1 {
            let exponents = Exponents {
                u: u8::try_from(index % 8).unwrap(),
                v: u8::try_from(index / 8 % 8).unwrap(),
                q: u8::try_from(index / 64 % 8).unwrap(),
                ..Exponents::default()
            };
            at_ceiling = at_ceiling
                .add(
                    PolyBuilder::monomial(one, exponents, coefficients.program()),
                    &mut coefficients,
                )
                .unwrap();
        }
        let accepted = at_ceiling
            .clone()
            .finish(PolyProgramId(0), ProgramLimit::UnrestrictedIntermediate)
            .unwrap();
        assert_eq!(accepted.terms.len(), MAX_TERMS_PER_PROGRAM_V1);

        let above_ceiling = PolyBuilder::monomial(
            one,
            Exponents {
                x: 1,
                ..Exponents::default()
            },
            coefficients.program(),
        );
        let error = at_ceiling
            .add(above_ceiling, &mut coefficients)
            .unwrap_err();
        assert!(error.starts_with("P015:"));
        assert!(error.contains("needs 513 terms, ceiling 512"));
    }
}
