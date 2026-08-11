//! Auditable compile-time omission and strict-sign proof records.

use std::collections::BTreeMap;

use super::competition::{CompetitionPrograms, CompetitionSubject, PairOmissionHint};
use super::events::{EventPrograms, EventSubject, OmissionHint};
use super::ids::{CoeffId, DomainId, ExclusionId, FeatureId, PolyProgramId, ProofRecordId};
use super::reference::interval::{F64Interval, next_down, next_up};

pub const MAX_BERNSTEIN_VARIABLES_V1: usize = 4;
pub const MAX_BERNSTEIN_SUBDIVISION_DEPTH_V1: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExclusionReason {
    WorldBoundsDisjoint,
    ProjectedBoundsDisjoint,
    QRangesDisjoint,
    OutsideNearFar,
    CsgNonInfluential,
    FeatureValidityImpossible,
    SupportShellDisjoint,
    MaterialClassIrrelevant,
    FullDomainProjection,
    StaticStrictOrder,
    GlobalParameterBoxStrictSign,
    GlobalSpatialBoxStrictSign,
    DuplicateCanonicalFeature,
}

impl ExclusionReason {
    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::WorldBoundsDisjoint => "world-bounds-disjoint",
            Self::ProjectedBoundsDisjoint => "projected-bounds-disjoint",
            Self::QRangesDisjoint => "q-ranges-disjoint",
            Self::OutsideNearFar => "outside-near-far",
            Self::CsgNonInfluential => "csg-noninfluential",
            Self::FeatureValidityImpossible => "feature-validity-impossible",
            Self::SupportShellDisjoint => "support-shell-disjoint",
            Self::MaterialClassIrrelevant => "material-class-irrelevant",
            Self::FullDomainProjection => "full-domain-projection",
            Self::StaticStrictOrder => "static-strict-order",
            Self::GlobalParameterBoxStrictSign => "global-parameter-box-strict-sign",
            Self::GlobalSpatialBoxStrictSign => "global-spatial-box-strict-sign",
            Self::DuplicateCanonicalFeature => "duplicate-canonical-feature",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExclusionSubject {
    Candidate(FeatureId),
    Event(EventSubject),
    Competition(CompetitionSubject),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvenSign {
    Negative,
    Positive,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedAxis {
    pub lo: f64,
    pub hi: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumericTerm {
    pub coefficient: f64,
    pub exponents: [u8; MAX_BERNSTEIN_VARIABLES_V1],
}

#[derive(Clone, Debug, PartialEq)]
pub struct NumericPolynomial {
    pub program: Option<PolyProgramId>,
    pub variable_count: u8,
    pub terms: Vec<NumericTerm>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BernsteinSubdivisionNode {
    pub path_bits: u32,
    pub depth: u8,
    pub split_variable: Option<u8>,
    pub sign: Option<ProvenSign>,
    pub margin: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BernsteinProofPayload {
    pub normalized_box: Vec<NormalizedAxis>,
    pub polynomial: Option<PolyProgramId>,
    pub coefficient_program_root: Option<CoeffId>,
    pub degrees: Vec<u8>,
    pub coefficient_order: Vec<Vec<u8>>,
    pub outward_conversion_radius: f64,
    pub subdivision_tree: Vec<BernsteinSubdivisionNode>,
    pub strict_sign: ProvenSign,
    pub minimum_margin: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProofPayload {
    PositiveMargin {
        rule: &'static str,
        facts: Vec<String>,
    },
    Bernstein(BernsteinProofPayload),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProofRecord {
    pub id: ProofRecordId,
    pub payload: ProofPayload,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExclusionRecord {
    pub id: ExclusionId,
    pub subject: ExclusionSubject,
    pub domain: DomainId,
    pub reason: ExclusionReason,
    pub margin: F64Interval,
    pub proof: ProofRecordId,
    pub dependencies: Vec<ProofRecordId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExclusionPrograms {
    pub proofs: Vec<ProofRecord>,
    pub records: Vec<ExclusionRecord>,
}

fn enumerate_indices(degrees: &[u8]) -> Vec<Vec<u8>> {
    fn recurse(degrees: &[u8], variable: usize, current: &mut Vec<u8>, output: &mut Vec<Vec<u8>>) {
        if variable == degrees.len() {
            output.push(current.clone());
            return;
        }
        for exponent in 0..=degrees[variable] {
            current.push(exponent);
            recurse(degrees, variable + 1, current, output);
            current.pop();
        }
    }
    let mut output = Vec::new();
    recurse(degrees, 0, &mut Vec::new(), &mut output);
    output
}

fn binomial(n: u8, k: u8) -> f64 {
    if k > n {
        return 0.0;
    }
    let k = k.min(n - k);
    (0..k).fold(1.0, |value, index| {
        value * f64::from(n - index) / f64::from(index + 1)
    })
}

fn interval_power(value: F64Interval, exponent: u8) -> Result<F64Interval, String> {
    let mut result = F64Interval::point(1.0)?;
    for _ in 0..exponent {
        result = result.mul_outward(value)?;
    }
    Ok(result)
}

fn interval_range(
    polynomial: &NumericPolynomial,
    domain: &[NormalizedAxis],
) -> Result<F64Interval, String> {
    let mut result = F64Interval::point(0.0)?;
    for term in &polynomial.terms {
        let mut value = F64Interval::point(term.coefficient)?;
        for (variable, axis) in domain.iter().enumerate() {
            value = value.mul_outward(interval_power(
                F64Interval::new(axis.lo, axis.hi)?,
                term.exponents[variable],
            )?)?;
        }
        result = result.add_outward(value)?;
    }
    Ok(result)
}

fn normalized_power_coefficients(
    polynomial: &NumericPolynomial,
    domain: &[NormalizedAxis],
    degrees: &[u8],
) -> Result<BTreeMap<Vec<u8>, F64Interval>, String> {
    let zero = F64Interval::point(0.0)?;
    let mut power = BTreeMap::<Vec<u8>, F64Interval>::new();
    for term in &polynomial.terms {
        let mut partial =
            BTreeMap::from([(Vec::<u8>::new(), F64Interval::point(term.coefficient)?)]);
        for variable in 0..usize::from(polynomial.variable_count) {
            let exponent = term.exponents[variable];
            let axis = domain[variable];
            let width_value = axis.hi - axis.lo;
            let width = F64Interval::new(next_down(width_value), next_up(width_value))?;
            let axis_lo = F64Interval::point(axis.lo)?;
            let mut next = BTreeMap::new();
            for (prefix, coefficient) in partial {
                for t_power in 0..=exponent {
                    let scale = F64Interval::point(binomial(exponent, t_power))?
                        .mul_outward(interval_power(axis_lo, exponent - t_power)?)?
                        .mul_outward(interval_power(width, t_power)?)?;
                    let mut index = prefix.clone();
                    index.push(t_power);
                    let contribution = coefficient.mul_outward(scale)?;
                    let current = next.remove(&index).unwrap_or(zero);
                    next.insert(index, current.add_outward(contribution)?);
                }
            }
            partial = next;
        }
        for (index, coefficient) in partial {
            let current = power.remove(&index).unwrap_or(zero);
            power.insert(index, current.add_outward(coefficient)?);
        }
    }
    for index in enumerate_indices(degrees) {
        power.entry(index).or_insert(zero);
    }
    Ok(power)
}

fn bernstein_coefficients(
    polynomial: &NumericPolynomial,
    domain: &[NormalizedAxis],
    degrees: &[u8],
) -> Result<Vec<(Vec<u8>, F64Interval)>, String> {
    let power = normalized_power_coefficients(polynomial, domain, degrees)?;
    let zero = F64Interval::point(0.0)?;
    enumerate_indices(degrees)
        .into_iter()
        .map(|index| {
            let mut value = zero;
            for power_index in enumerate_indices(&index) {
                let mut ratio = 1.0;
                for variable in 0..index.len() {
                    ratio *= binomial(index[variable], power_index[variable])
                        / binomial(degrees[variable], power_index[variable]);
                }
                let ratio = F64Interval::new(next_down(ratio), next_up(ratio))?;
                value = value.add_outward(
                    power
                        .get(&power_index)
                        .copied()
                        .unwrap_or(zero)
                        .mul_outward(ratio)?,
                )?;
            }
            Ok((index, value))
        })
        .collect()
}

fn scan_sign(coefficients: &[(Vec<u8>, F64Interval)]) -> Option<(ProvenSign, f64)> {
    let positive = coefficients
        .iter()
        .map(|(_, coefficient)| coefficient.lo)
        .fold(f64::INFINITY, f64::min);
    if positive > 0.0 {
        return Some((ProvenSign::Positive, positive));
    }
    let negative = coefficients
        .iter()
        .map(|(_, coefficient)| -coefficient.hi)
        .fold(f64::INFINITY, f64::min);
    (negative > 0.0).then_some((ProvenSign::Negative, negative))
}

pub fn prove_global_strict_sign(
    polynomial: &NumericPolynomial,
    domain: &[NormalizedAxis],
) -> Result<Option<BernsteinProofPayload>, String> {
    if usize::from(polynomial.variable_count) != domain.len()
        || domain.len() > MAX_BERNSTEIN_VARIABLES_V1
    {
        return Err(format!(
            "P004: field operation `global Bernstein exclusion` is not available in `AaaByteExact`: variable count {}",
            domain.len()
        ));
    }
    if domain
        .iter()
        .any(|axis| !axis.lo.is_finite() || !axis.hi.is_finite() || axis.lo > axis.hi)
    {
        return Err("pixels::exclusions: invalid global strict-sign domain".to_string());
    }
    if polynomial
        .terms
        .iter()
        .any(|term| !term.coefficient.is_finite())
    {
        return Err("pixels::exclusions: non-finite strict-sign coefficient".to_string());
    }
    let mut degrees = vec![0_u8; domain.len()];
    for term in &polynomial.terms {
        for (degree, exponent) in degrees.iter_mut().zip(term.exponents) {
            *degree = (*degree).max(exponent);
        }
    }
    if degrees.iter().any(|degree| *degree > 6) {
        return Ok(None);
    }
    // Fixed hierarchy stage 1: a single canonical constant is exact. Multiple
    // floating terms are deliberately left to the outward interval stage;
    // summing them in f64 could manufacture a false strict sign through
    // cancellation.
    if degrees.iter().all(|degree| *degree == 0) {
        if let [term] = polynomial.terms.as_slice() {
            let value = term.coefficient;
            let Some(sign) = (value != 0.0).then_some(if value > 0.0 {
                ProvenSign::Positive
            } else {
                ProvenSign::Negative
            }) else {
                return Ok(None);
            };
            return Ok(Some(BernsteinProofPayload {
                normalized_box: domain.to_vec(),
                polynomial: polynomial.program,
                coefficient_program_root: None,
                degrees,
                coefficient_order: vec![Vec::new()],
                outward_conversion_radius: 0.0,
                subdivision_tree: vec![BernsteinSubdivisionNode {
                    path_bits: 0,
                    depth: 0,
                    split_variable: None,
                    sign: Some(sign),
                    margin: value.abs(),
                }],
                strict_sign: sign,
                minimum_margin: value.abs(),
            }));
        }
    }
    // Fixed hierarchy stage 2: outward interval range.
    let interval = interval_range(polynomial, domain)?;
    if interval.lo > 0.0 || interval.hi < 0.0 {
        let (sign, margin) = if interval.lo > 0.0 {
            (ProvenSign::Positive, interval.lo)
        } else {
            (ProvenSign::Negative, -interval.hi)
        };
        return Ok(Some(BernsteinProofPayload {
            normalized_box: domain.to_vec(),
            polynomial: polynomial.program,
            coefficient_program_root: None,
            degrees: degrees.clone(),
            coefficient_order: enumerate_indices(&degrees),
            outward_conversion_radius: next_up(interval.width() * f64::EPSILON),
            subdivision_tree: vec![BernsteinSubdivisionNode {
                path_bits: 0,
                depth: 0,
                split_variable: None,
                sign: Some(sign),
                margin,
            }],
            strict_sign: sign,
            minimum_margin: margin,
        }));
    }
    if degrees.iter().all(|degree| *degree == 0) {
        return Ok(None);
    }
    // Fixed hierarchy stages 3 and 4: complete Bernstein scan and bounded
    // deterministic subdivision. Inconclusive leaves return None so the
    // caller retains the runtime predicate.
    let mut pending = vec![(domain.to_vec(), 0_u8, 0_u32)];
    let mut tree = Vec::new();
    let mut final_sign = None;
    let mut minimum_margin = f64::INFINITY;
    let coefficient_order = enumerate_indices(&degrees);
    while let Some((box_domain, depth, path_bits)) = pending.pop() {
        let coefficients = bernstein_coefficients(polynomial, &box_domain, &degrees)?;
        if let Some((sign, margin)) = scan_sign(&coefficients) {
            if final_sign.is_some_and(|existing| existing != sign) {
                return Ok(None);
            }
            final_sign = Some(sign);
            minimum_margin = minimum_margin.min(margin);
            tree.push(BernsteinSubdivisionNode {
                path_bits,
                depth,
                split_variable: None,
                sign: Some(sign),
                margin,
            });
            continue;
        }
        if depth >= MAX_BERNSTEIN_SUBDIVISION_DEPTH_V1 {
            return Ok(None);
        }
        let split_variable = box_domain
            .iter()
            .enumerate()
            .max_by(|(a_index, a), (b_index, b)| {
                (a.hi - a.lo)
                    .total_cmp(&(b.hi - b.lo))
                    .then_with(|| b_index.cmp(a_index))
            })
            .map(|(index, _)| index)
            .ok_or_else(|| {
                "pixels::exclusions: cannot split zero-variable polynomial".to_string()
            })?;
        let midpoint = box_domain[split_variable].lo
            + (box_domain[split_variable].hi - box_domain[split_variable].lo) * 0.5;
        if midpoint <= box_domain[split_variable].lo || midpoint >= box_domain[split_variable].hi {
            return Ok(None);
        }
        tree.push(BernsteinSubdivisionNode {
            path_bits,
            depth,
            split_variable: Some(split_variable as u8),
            sign: None,
            margin: 0.0,
        });
        let mut left = box_domain.clone();
        let mut right = box_domain;
        left[split_variable].hi = midpoint;
        right[split_variable].lo = midpoint;
        pending.push((right, depth + 1, (path_bits << 1) | 1));
        pending.push((left, depth + 1, path_bits << 1));
    }
    let Some(strict_sign) = final_sign else {
        return Ok(None);
    };
    Ok(Some(BernsteinProofPayload {
        normalized_box: domain.to_vec(),
        polynomial: polynomial.program,
        coefficient_program_root: None,
        degrees,
        coefficient_order,
        outward_conversion_radius: tree
            .iter()
            .map(|node| node.margin * f64::EPSILON)
            .fold(0.0, f64::max),
        subdivision_tree: tree,
        strict_sign,
        minimum_margin,
    }))
}

pub(crate) fn prove_projective_program_strict_sign(
    polynomial: &super::polynomial::PolyProgram,
    coefficient_intervals: &[F64Interval],
    domain: [NormalizedAxis; 2],
) -> Result<Option<BernsteinProofPayload>, String> {
    prove_projective_program_strict_sign_on_box(polynomial, coefficient_intervals, &domain, false)
}

pub(crate) fn prove_projective_program_strict_sign_with_q(
    polynomial: &super::polynomial::PolyProgram,
    coefficient_intervals: &[F64Interval],
    domain: [NormalizedAxis; 3],
) -> Result<Option<BernsteinProofPayload>, String> {
    prove_projective_program_strict_sign_on_box(polynomial, coefficient_intervals, &domain, true)
}

fn prove_projective_program_strict_sign_on_box(
    polynomial: &super::polynomial::PolyProgram,
    coefficient_intervals: &[F64Interval],
    domain: &[NormalizedAxis],
    allow_q: bool,
) -> Result<Option<BernsteinProofPayload>, String> {
    if domain.len() != 2 + usize::from(allow_q) {
        return Err("pixels::exclusions: projective strict-sign domain arity differs".to_string());
    }
    if polynomial.terms.iter().any(|term| {
        (!allow_q && term.exponents.q != 0)
            || term.exponents.x != 0
            || term.exponents.t != 0
            || term.exponents.param_terms.count != 0
    }) {
        return Err(format!(
            "pixels::exclusions: projective strict-sign candidate {} has unsupported variables",
            polynomial.id,
        ));
    }
    let mut interval = F64Interval::point(0.0)?;
    let mut numeric_terms = Vec::with_capacity(polynomial.terms.len());
    let mut all_point = true;
    let mut degrees = vec![0_u8; domain.len()];
    let mut conversion_radius = 0.0_f64;
    let mut coefficient_uncertainty = 0.0_f64;
    for term in &polynomial.terms {
        let coefficient = coefficient_intervals
            .get(term.coefficient.index())
            .copied()
            .ok_or_else(|| {
                format!(
                    "pixels::exclusions: strict-sign polynomial {} names missing coefficient {}",
                    polynomial.id, term.coefficient
                )
            })?;
        all_point &= coefficient.lo == coefficient.hi;
        conversion_radius = next_up(conversion_radius + coefficient.width() * f64::EPSILON);
        degrees[0] = degrees[0].max(term.exponents.u);
        degrees[1] = degrees[1].max(term.exponents.v);
        if allow_q {
            degrees[2] = degrees[2].max(term.exponents.q);
        }
        let mut value = coefficient;
        for (axis, exponent) in
            domain
                .iter()
                .zip([term.exponents.u, term.exponents.v, term.exponents.q])
        {
            value = value.mul_outward(interval_power(
                F64Interval::new(axis.lo, axis.hi)?,
                exponent,
            )?)?;
        }
        interval = interval.add_outward(value)?;
        let midpoint = coefficient.lo + (coefficient.hi - coefficient.lo) * 0.5;
        if !midpoint.is_finite() {
            return Err(
                "pixels::exclusions: non-finite projective coefficient midpoint".to_string(),
            );
        }
        let radius = (midpoint - coefficient.lo)
            .abs()
            .max((coefficient.hi - midpoint).abs());
        let u_abs = domain[0].lo.abs().max(domain[0].hi.abs());
        let v_abs = domain[1].lo.abs().max(domain[1].hi.abs());
        let mut monomial_abs = next_up(
            next_up(u_abs.powi(i32::from(term.exponents.u)))
                * next_up(v_abs.powi(i32::from(term.exponents.v))),
        );
        if allow_q {
            let q_abs = domain[2].lo.abs().max(domain[2].hi.abs());
            monomial_abs = next_up(monomial_abs * next_up(q_abs.powi(i32::from(term.exponents.q))));
        }
        let contribution = next_up(radius * monomial_abs);
        coefficient_uncertainty = next_up(coefficient_uncertainty + contribution);
        numeric_terms.push(NumericTerm {
            coefficient: midpoint,
            exponents: [term.exponents.u, term.exponents.v, term.exponents.q, 0],
        });
    }
    if all_point {
        return prove_global_strict_sign(
            &NumericPolynomial {
                program: Some(polynomial.id),
                variable_count: u8::try_from(domain.len())
                    .map_err(|_| "P015: strict-sign domain arity overflow".to_string())?,
                terms: numeric_terms,
            },
            domain,
        );
    }
    let (strict_sign, minimum_margin) = if interval.lo > 0.0 {
        (ProvenSign::Positive, interval.lo)
    } else if interval.hi < 0.0 {
        (ProvenSign::Negative, -interval.hi)
    } else {
        let midpoint = NumericPolynomial {
            program: Some(polynomial.id),
            variable_count: u8::try_from(domain.len())
                .map_err(|_| "P015: strict-sign domain arity overflow".to_string())?,
            terms: numeric_terms,
        };
        let Some(mut proof) = prove_global_strict_sign(&midpoint, domain)? else {
            return Ok(None);
        };
        if proof.minimum_margin <= coefficient_uncertainty {
            return Ok(None);
        }
        proof.minimum_margin = next_down(proof.minimum_margin - coefficient_uncertainty);
        for node in &mut proof.subdivision_tree {
            if node.sign.is_some() {
                if node.margin <= coefficient_uncertainty {
                    return Ok(None);
                }
                node.margin = next_down(node.margin - coefficient_uncertainty);
            }
        }
        proof.outward_conversion_radius =
            next_up(proof.outward_conversion_radius + conversion_radius + coefficient_uncertainty);
        return Ok(Some(proof));
    };
    let coefficient_order = enumerate_indices(&degrees);
    Ok(Some(BernsteinProofPayload {
        normalized_box: domain.to_vec(),
        polynomial: Some(polynomial.id),
        coefficient_program_root: None,
        degrees,
        coefficient_order,
        outward_conversion_radius: conversion_radius.max(next_up(interval.width() * f64::EPSILON)),
        subdivision_tree: vec![BernsteinSubdivisionNode {
            path_bits: 0,
            depth: 0,
            split_variable: None,
            sign: Some(strict_sign),
            margin: minimum_margin,
        }],
        strict_sign,
        minimum_margin,
    }))
}

pub(crate) fn prove_interval_strict_sign(
    interval: F64Interval,
    domain: &[NormalizedAxis],
) -> Result<Option<BernsteinProofPayload>, String> {
    if domain.len() > MAX_BERNSTEIN_VARIABLES_V1 {
        return Ok(None);
    }
    if domain
        .iter()
        .any(|axis| !axis.lo.is_finite() || !axis.hi.is_finite() || axis.lo > axis.hi)
    {
        return Err("pixels::exclusions: invalid interval-proof domain".to_string());
    }
    let (strict_sign, minimum_margin) = if interval.lo > 0.0 {
        (ProvenSign::Positive, interval.lo)
    } else if interval.hi < 0.0 {
        (ProvenSign::Negative, -interval.hi)
    } else {
        return Ok(None);
    };
    let degrees = vec![0_u8; domain.len()];
    Ok(Some(BernsteinProofPayload {
        normalized_box: domain.to_vec(),
        polynomial: None,
        coefficient_program_root: None,
        coefficient_order: enumerate_indices(&degrees),
        degrees,
        outward_conversion_radius: next_up(interval.width() * f64::EPSILON),
        subdivision_tree: vec![BernsteinSubdivisionNode {
            path_bits: 0,
            depth: 0,
            split_variable: None,
            sign: Some(strict_sign),
            margin: minimum_margin,
        }],
        strict_sign,
        minimum_margin,
    }))
}

struct Builder {
    proofs: Vec<ProofRecord>,
    records: Vec<ExclusionRecord>,
}

impl Builder {
    fn record(
        &mut self,
        subject: ExclusionSubject,
        reason: ExclusionReason,
        margin: f64,
        rule: &'static str,
        facts: Vec<String>,
        dependencies: Vec<ProofRecordId>,
    ) -> Result<(), String> {
        if !margin.is_finite() || margin <= 0.0 {
            return Err(format!(
                "pixels::exclusions: {subject:?} has nonpositive exclusion margin {margin}"
            ));
        }
        let proof = ProofRecordId(
            u32::try_from(self.proofs.len())
                .map_err(|_| "P015: exclusion proof ID overflow".to_string())?,
        );
        self.proofs.push(ProofRecord {
            id: proof,
            payload: ProofPayload::PositiveMargin { rule, facts },
        });
        let id = ExclusionId(
            u32::try_from(self.records.len())
                .map_err(|_| "P015: exclusion record ID overflow".to_string())?,
        );
        self.records.push(ExclusionRecord {
            id,
            subject,
            domain: DomainId(0),
            reason,
            margin: F64Interval::new(next_down(margin), next_up(margin))?,
            proof,
            dependencies,
        });
        Ok(())
    }

    fn record_interval_sign(
        &mut self,
        subject: ExclusionSubject,
        polynomial: PolyProgramId,
        coefficient: CoeffId,
        enclosure: F64Interval,
        sign: super::projective::StrictSign,
    ) -> Result<(), String> {
        let (proven_sign, margin) = match sign {
            super::projective::StrictSign::Positive if enclosure.lo > 0.0 => {
                (ProvenSign::Positive, enclosure.lo)
            }
            super::projective::StrictSign::Negative if enclosure.hi < 0.0 => {
                (ProvenSign::Negative, -enclosure.hi)
            }
            _ => {
                return Err(format!(
                    "pixels::exclusions: strict-sign obligation for {subject:?} is not strict"
                ));
            }
        };
        let signed_margin = match proven_sign {
            ProvenSign::Positive => margin,
            ProvenSign::Negative => -margin,
        };
        let mut payload = prove_global_strict_sign(
            &NumericPolynomial {
                program: Some(polynomial),
                variable_count: 0,
                terms: vec![NumericTerm {
                    coefficient: signed_margin,
                    exponents: [0; MAX_BERNSTEIN_VARIABLES_V1],
                }],
            },
            &[],
        )?
        .ok_or_else(|| {
            format!(
                "pixels::exclusions: complete-box strict-sign hierarchy did not prove {subject:?}"
            )
        })?;
        payload.coefficient_program_root = Some(coefficient);
        payload.outward_conversion_radius = payload
            .outward_conversion_radius
            .max(next_up(enclosure.width() * f64::EPSILON));
        let proof = ProofRecordId(
            u32::try_from(self.proofs.len())
                .map_err(|_| "P015: exclusion proof ID overflow".to_string())?,
        );
        self.proofs.push(ProofRecord {
            id: proof,
            payload: ProofPayload::Bernstein(payload),
        });
        let id = ExclusionId(
            u32::try_from(self.records.len())
                .map_err(|_| "P015: exclusion record ID overflow".to_string())?,
        );
        self.records.push(ExclusionRecord {
            id,
            subject,
            domain: DomainId(0),
            reason: ExclusionReason::GlobalParameterBoxStrictSign,
            margin: F64Interval::new(next_down(margin), next_up(margin))?,
            proof,
            dependencies: Vec::new(),
        });
        Ok(())
    }

    fn record_bernstein(
        &mut self,
        subject: ExclusionSubject,
        reason: ExclusionReason,
        payload: BernsteinProofPayload,
    ) -> Result<(), String> {
        let margin = payload.minimum_margin;
        if !margin.is_finite() || margin <= 0.0 {
            return Err(format!(
                "pixels::exclusions: {subject:?} has invalid Bernstein margin {margin}"
            ));
        }
        let proof = ProofRecordId(
            u32::try_from(self.proofs.len())
                .map_err(|_| "P015: exclusion proof ID overflow".to_string())?,
        );
        self.proofs.push(ProofRecord {
            id: proof,
            payload: ProofPayload::Bernstein(payload),
        });
        let id = ExclusionId(
            u32::try_from(self.records.len())
                .map_err(|_| "P015: exclusion record ID overflow".to_string())?,
        );
        self.records.push(ExclusionRecord {
            id,
            subject,
            domain: DomainId(0),
            reason,
            margin: F64Interval::new(next_down(margin), next_up(margin))?,
            proof,
            dependencies: Vec::new(),
        });
        Ok(())
    }
}

pub fn compile(
    projective: &super::projective::ProjectiveEquations,
    spans: &[super::projection_bounds::ProjectedFeatureSpan],
    events: &EventPrograms,
    competitions: &CompetitionPrograms,
) -> Result<ExclusionPrograms, String> {
    let mut builder = Builder {
        proofs: Vec::new(),
        records: Vec::new(),
    };
    let mut outside_candidate_proofs = BTreeMap::new();
    for span in spans {
        if span.rule == super::projection_bounds::ProjectionRule::OutsideNearFar {
            let margin = span.outside_margin.ok_or_else(|| {
                format!(
                    "pixels::exclusions: outside-near/far span for {} has no proof margin",
                    span.feature
                )
            })?;
            let proof = ProofRecordId(
                u32::try_from(builder.proofs.len())
                    .map_err(|_| "P015: exclusion proof ID overflow".to_string())?,
            );
            builder.record(
                ExclusionSubject::Candidate(span.feature),
                ExclusionReason::OutsideNearFar,
                margin,
                "feature-projection-outside-near-far",
                vec![format!("outward clip-plane gap={margin}")],
                Vec::new(),
            )?;
            outside_candidate_proofs.insert(span.feature, proof);
        }
    }
    for entry in &events.ledger {
        match (entry.emitted, entry.omission.as_ref()) {
            (Some(_), None) => {}
            (None, Some(OmissionHint::FullDomainProjection)) => {
                let feature = entry.subject.feature.ok_or_else(|| {
                    format!(
                        "pixels::exclusions: full-domain projection omission {:?} has no feature",
                        entry.subject
                    )
                })?;
                let span = spans
                    .get(feature.index())
                    .filter(|span| span.feature == feature)
                    .ok_or_else(|| {
                        format!(
                            "pixels::exclusions: full-domain projection names missing {feature}"
                        )
                    })?;
                if span.pixels.x.start != 0
                    || span.pixels.x.end != projective.camera.width
                    || span.pixels.y.start != 0
                    || span.pixels.y.end != projective.camera.height
                {
                    return Err(format!(
                        "pixels::exclusions: full-domain projection omission for {feature} does not cover the output"
                    ));
                }
                // Projected-bound events lie on integer pixel edges. The
                // nearest in-domain sample is a pixel center, exactly half a
                // pixel from the outer edge.
                builder.record(
                    ExclusionSubject::Event(entry.subject),
                    ExclusionReason::FullDomainProjection,
                    0.5,
                    "projected-boundary-outside-complete-output-domain",
                    vec![format!(
                        "span=[{},{}]x[{},{}] output={}x{} nearest-center-gap=0.5",
                        span.pixels.x.start,
                        span.pixels.x.end,
                        span.pixels.y.start,
                        span.pixels.y.end,
                        projective.camera.width,
                        projective.camera.height,
                    )],
                    Vec::new(),
                )?
            }
            (None, Some(OmissionHint::StrictProjectiveLeadingSign)) => {
                let feature = entry.subject.feature.ok_or_else(|| {
                    format!(
                        "pixels::exclusions: strict projective omission {:?} has no feature",
                        entry.subject
                    )
                })?;
                let lowered = projective.features.get(feature.index()).ok_or_else(|| {
                    format!(
                        "pixels::exclusions: strict projective omission names missing {feature}"
                    )
                })?;
                let super::projective::SeedKind::Affine { denominator } = lowered.q_seed_kind
                else {
                    return Err(format!(
                        "pixels::exclusions: strict projective omission for {feature} is not affine"
                    ));
                };
                builder.record_interval_sign(
                    ExclusionSubject::Event(entry.subject),
                    lowered.root_equation,
                    denominator.coefficient,
                    denominator.enclosure,
                    denominator.sign,
                )?;
            }
            (None, Some(OmissionHint::OutsideNearFar)) => {
                let feature = entry.subject.feature.ok_or_else(|| {
                    format!(
                        "pixels::exclusions: outside-near/far event {:?} has no feature",
                        entry.subject
                    )
                })?;
                let proof = *outside_candidate_proofs.get(&feature).ok_or_else(|| {
                    format!(
                        "pixels::exclusions: outside-near/far event {:?} lacks a candidate proof",
                        entry.subject
                    )
                })?;
                let span = spans.get(feature.index()).ok_or_else(|| {
                    format!("pixels::exclusions: event names missing feature span {feature}")
                })?;
                let margin = span.outside_margin.ok_or_else(|| {
                    format!("pixels::exclusions: outside-near/far span for {feature} has no margin")
                })?;
                builder.record(
                    ExclusionSubject::Event(entry.subject),
                    ExclusionReason::OutsideNearFar,
                    margin,
                    "feature-projection-outside-near-far",
                    vec![format!("inherits candidate clip-plane gap={margin}")],
                    vec![proof],
                )?
            }
            (
                None,
                Some(OmissionHint::ClipQOutsideFeatureQSpan {
                    clip_q,
                    feature_q,
                    margin,
                }),
            ) => {
                let feature = entry.subject.feature.ok_or_else(|| {
                    format!(
                        "pixels::exclusions: clip-q omission {:?} has no feature",
                        entry.subject
                    )
                })?;
                let span = spans
                    .get(feature.index())
                    .filter(|span| span.feature == feature)
                    .ok_or_else(|| {
                        format!("pixels::exclusions: clip-q omission names missing {feature}")
                    })?;
                if span.q != *feature_q {
                    return Err(format!(
                        "pixels::exclusions: clip-q omission for {feature} does not name the sealed projected q span"
                    ));
                }
                if *clip_q >= feature_q.lo && *clip_q <= feature_q.hi {
                    return Err(format!(
                        "pixels::exclusions: clip-q omission for {feature} names a clip q inside the sealed span"
                    ));
                }
                builder.record(
                    ExclusionSubject::Event(entry.subject),
                    ExclusionReason::QRangesDisjoint,
                    *margin,
                    "positive-q-range-gap",
                    vec![format!(
                        "clip q={clip_q} feature q=[{},{}] separation={margin}",
                        feature_q.lo, feature_q.hi,
                    )],
                    Vec::new(),
                )?
            }
            (None, Some(OmissionHint::GlobalStrictSign { kind, proof })) => {
                let reason = match kind {
                    super::events::PredicateOmissionKind::FeatureValidity => {
                        ExclusionReason::FeatureValidityImpossible
                    }
                    super::events::PredicateOmissionKind::SupportShell => {
                        ExclusionReason::SupportShellDisjoint
                    }
                    super::events::PredicateOmissionKind::MaterialClass => {
                        ExclusionReason::MaterialClassIrrelevant
                    }
                };
                builder.record_bernstein(
                    ExclusionSubject::Event(entry.subject),
                    reason,
                    proof.clone(),
                )?
            }
            _ => {
                return Err(format!(
                    "pixels::exclusions: event {:?} is neither emitted nor explicitly omitted",
                    entry.subject
                ));
            }
        }
    }
    for decision in &competitions.ledger {
        match (decision.emitted, decision.omission) {
            (Some(_), None) => {}
            (None, Some(PairOmissionHint::ProjectedBoundsDisjoint)) => builder.record(
                ExclusionSubject::Competition(decision.subject),
                ExclusionReason::ProjectedBoundsDisjoint,
                decision.margin,
                "half-open-projected-span-gap",
                vec![format!("pixel gap={}", decision.margin)],
                Vec::new(),
            )?,
            (None, Some(PairOmissionHint::QRangesDisjoint)) => builder.record(
                ExclusionSubject::Competition(decision.subject),
                ExclusionReason::QRangesDisjoint,
                decision.margin,
                "positive-q-range-gap",
                vec![format!("q gap={}", decision.margin)],
                Vec::new(),
            )?,
            (None, Some(PairOmissionHint::CsgNonInfluential)) => builder.record(
                ExclusionSubject::Competition(decision.subject),
                ExclusionReason::CsgNonInfluential,
                decision.margin,
                "boolean-cofactor-equality",
                vec!["forced false and forced true cofactors are identical".to_string()],
                Vec::new(),
            )?,
            (
                None,
                Some(PairOmissionHint::CsgJointInfluenceImpossible {
                    variable_count,
                    states_checked,
                }),
            ) => builder.record(
                ExclusionSubject::Competition(decision.subject),
                ExclusionReason::CsgNonInfluential,
                decision.margin,
                "exhaustive-joint-boolean-influence",
                vec![
                    format!("variables={variable_count}"),
                    format!("states_checked={states_checked}"),
                    "no occupancy state makes both Boolean cofactors differ".to_string(),
                ],
                Vec::new(),
            )?,
            (None, Some(PairOmissionHint::OutsideNearFar)) => {
                let mut dependencies = [decision.subject.a, decision.subject.b]
                    .into_iter()
                    .filter_map(|feature| outside_candidate_proofs.get(&feature).copied())
                    .collect::<Vec<_>>();
                dependencies.sort();
                dependencies.dedup();
                if dependencies.is_empty() {
                    return Err(format!(
                        "pixels::exclusions: outside-near/far competition {:?} lacks a candidate proof",
                        decision.subject
                    ));
                }
                builder.record(
                    ExclusionSubject::Competition(decision.subject),
                    ExclusionReason::OutsideNearFar,
                    decision.margin,
                    "feature-projection-outside-near-far",
                    vec![format!(
                        "inherits minimum candidate clip-plane gap={}",
                        decision.margin
                    )],
                    dependencies,
                )?
            }
            (
                None,
                Some(
                    omission @ (PairOmissionHint::StaticStrictOrder
                    | PairOmissionHint::GlobalStrictOrder),
                ),
            ) => {
                let payload = decision.strict_sign_proof.clone().ok_or_else(|| {
                    format!(
                        "pixels::exclusions: strict-order competition {:?} has no proof payload",
                        decision.subject
                    )
                })?;
                let reason = match omission {
                    PairOmissionHint::StaticStrictOrder => ExclusionReason::StaticStrictOrder,
                    PairOmissionHint::GlobalStrictOrder => {
                        ExclusionReason::GlobalParameterBoxStrictSign
                    }
                    _ => unreachable!(),
                };
                builder.record_bernstein(
                    ExclusionSubject::Competition(decision.subject),
                    reason,
                    payload,
                )?;
            }
            _ => {
                return Err(format!(
                    "pixels::exclusions: competition {:?} is neither emitted nor explicitly omitted",
                    decision.subject
                ));
            }
        }
    }
    builder.records.sort_by_key(|record| record.subject);
    for (index, record) in builder.records.iter_mut().enumerate() {
        record.id = ExclusionId(
            u32::try_from(index)
                .map_err(|_| "P015: sorted exclusion record ID overflow".to_string())?,
        );
    }
    Ok(ExclusionPrograms {
        proofs: builder.proofs,
        records: builder.records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_parameter_complete_box_sign_can_be_proved_or_retained() {
        // 1 + p^2 is strictly positive even though naive dependency/rate
        // checks cannot mark the moving parameter static.
        let positive = NumericPolynomial {
            program: Some(PolyProgramId(7)),
            variable_count: 1,
            terms: vec![
                NumericTerm {
                    coefficient: 1.0,
                    exponents: [0; MAX_BERNSTEIN_VARIABLES_V1],
                },
                NumericTerm {
                    coefficient: 1.0,
                    exponents: [2, 0, 0, 0],
                },
            ],
        };
        let domain = [NormalizedAxis { lo: -2.0, hi: 2.0 }];
        let proof = prove_global_strict_sign(&positive, &domain)
            .unwrap()
            .expect("complete-box proof");
        assert_eq!(proof.strict_sign, ProvenSign::Positive);
        assert!(proof.minimum_margin > 0.0);

        // Perturbing the constant to -1 makes the sign genuinely
        // inconclusive. The static proof must disappear so a runtime
        // predicate is retained.
        let inconclusive = NumericPolynomial {
            terms: vec![
                NumericTerm {
                    coefficient: -1.0,
                    exponents: [0; MAX_BERNSTEIN_VARIABLES_V1],
                },
                NumericTerm {
                    coefficient: 1.0,
                    exponents: [2, 0, 0, 0],
                },
            ],
            ..positive
        };
        assert!(
            prove_global_strict_sign(&inconclusive, &domain)
                .unwrap()
                .is_none()
        );

        let cancelling_constant = NumericPolynomial {
            program: None,
            variable_count: 0,
            terms: vec![
                NumericTerm {
                    coefficient: 1.0e16,
                    exponents: [0; MAX_BERNSTEIN_VARIABLES_V1],
                },
                NumericTerm {
                    coefficient: 1.0,
                    exponents: [0; MAX_BERNSTEIN_VARIABLES_V1],
                },
                NumericTerm {
                    coefficient: -1.0e16,
                    exponents: [0; MAX_BERNSTEIN_VARIABLES_V1],
                },
                NumericTerm {
                    coefficient: -1.0,
                    exponents: [0; MAX_BERNSTEIN_VARIABLES_V1],
                },
            ],
        };
        assert!(
            prove_global_strict_sign(&cancelling_constant, &[])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn actual_projective_predicate_is_proved_or_retained_after_parameter_perturbation() {
        use super::super::ids::ScalarId;
        use super::super::polynomial::{Exponents, PolyBuilder, ProgramLimit};
        use super::super::program::CoeffBuilder;

        let mut coefficients = CoeffBuilder::new();
        let moving_constant = coefficients.scalar(ScalarId(0)).unwrap();
        let one = coefficients.constant(1.0).unwrap();
        let constant = PolyBuilder::monomial(
            moving_constant,
            Exponents::default(),
            coefficients.program(),
        );
        let u_squared = PolyBuilder::monomial(
            one,
            Exponents {
                u: 2,
                ..Exponents::default()
            },
            coefficients.program(),
        );
        let polynomial = constant
            .add(u_squared, &mut coefficients)
            .unwrap()
            .finish(PolyProgramId(19), ProgramLimit::EventBivariate)
            .unwrap();
        let domain = [
            // The broad u box makes ordinary interval evaluation of u²
            // sign-indefinite, forcing the production Bernstein stage.
            NormalizedAxis { lo: -2.0, hi: 2.0 },
            NormalizedAxis { lo: -1.0, hi: 1.0 },
        ];
        let positive_intervals = [
            F64Interval::new(1.0, 1.25).unwrap(),
            F64Interval::point(1.0).unwrap(),
        ];
        let proof = prove_projective_program_strict_sign(&polynomial, &positive_intervals, domain)
            .unwrap()
            .expect("full moving-parameter box is positive");
        assert_eq!(proof.polynomial, Some(PolyProgramId(19)));
        assert_eq!(proof.normalized_box, domain);
        assert_eq!(proof.strict_sign, ProvenSign::Positive);

        let crossing_intervals = [
            F64Interval::new(-1.25, 1.25).unwrap(),
            F64Interval::point(1.0).unwrap(),
        ];
        assert!(
            prove_projective_program_strict_sign(&polynomial, &crossing_intervals, domain)
                .unwrap()
                .is_none(),
            "an inconclusive complete box must retain the runtime predicate"
        );
    }
}
