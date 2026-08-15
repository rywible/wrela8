//! From-scratch structural row sweep built on the sealed P6 predicates.

use super::csg::{self, CsgInstruction, Orientation};
use super::interval::F64Interval;
use super::iv32::Iv32;
use super::telemetry::{
    CertificateTelemetry, CompositionShape, ExpiryCause, MarginOwner, RootMethod,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeatureId(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectId(pub u8);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdentitySetId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SweepError {
    CapacityExceeded,
    ProgramIndex,
    NumericFailure,
    CertificateExhausted,
    InternalInvariant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExclusionResult {
    Retain,
    Static { margin: i32 },
    RuntimeRow { margin: i32 },
}

impl Default for ExclusionResult {
    fn default() -> Self {
        Self::Retain
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexedFeature {
    pub id: FeatureId,
    pub row_start: u16,
    pub row_end: u16,
    pub exclusion: ExclusionResult,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CandidateCompleteness {
    pub indexed: u16,
    pub retained: u16,
    pub row_span_excluded: u16,
    pub static_excluded: u16,
    pub runtime_excluded: u16,
    /// Retained candidates for which complete isolation produced every root
    /// or a certified root-free result.
    pub roots_accounted: u16,
    pub minimum_exclusion_margin: i32,
}

pub fn enumerate_row_candidates(
    indexed: &[IndexedFeature],
    row: u16,
    output: &mut [FeatureId],
) -> Result<(usize, CandidateCompleteness), SweepError> {
    let mut count = 0_usize;
    let mut proof = CandidateCompleteness {
        indexed: u16::try_from(indexed.len()).map_err(|_| SweepError::CapacityExceeded)?,
        minimum_exclusion_margin: i32::MAX,
        ..CandidateCompleteness::default()
    };
    for feature in indexed {
        if row < feature.row_start || row >= feature.row_end {
            proof.row_span_excluded = proof
                .row_span_excluded
                .checked_add(1)
                .ok_or(SweepError::CapacityExceeded)?;
            continue;
        }
        match feature.exclusion {
            ExclusionResult::Retain => {
                let Some(slot) = output.get_mut(count) else {
                    return Err(SweepError::CapacityExceeded);
                };
                *slot = feature.id;
                count += 1;
            }
            ExclusionResult::Static { margin } | ExclusionResult::RuntimeRow { margin }
                if margin <= 0 =>
            {
                return Err(SweepError::CertificateExhausted);
            }
            ExclusionResult::Static { margin } => {
                proof.static_excluded = proof
                    .static_excluded
                    .checked_add(1)
                    .ok_or(SweepError::CapacityExceeded)?;
                proof.minimum_exclusion_margin = proof.minimum_exclusion_margin.min(margin);
            }
            ExclusionResult::RuntimeRow { margin } => {
                proof.runtime_excluded = proof
                    .runtime_excluded
                    .checked_add(1)
                    .ok_or(SweepError::CapacityExceeded)?;
                proof.minimum_exclusion_margin = proof.minimum_exclusion_margin.min(margin);
            }
        }
    }
    for index in 1..count {
        let value = output[index];
        let mut destination = index;
        while destination != 0 && value < output[destination - 1] {
            output[destination] = output[destination - 1];
            destination -= 1;
        }
        if destination != 0 && output[destination - 1] == value {
            return Err(SweepError::InternalInvariant);
        }
        output[destination] = value;
    }
    proof.retained = u16::try_from(count).map_err(|_| SweepError::CapacityExceeded)?;
    if proof.minimum_exclusion_margin == i32::MAX {
        proof.minimum_exclusion_margin = 0;
    }
    Ok((count, proof))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RootRecord {
    pub feature: FeatureId,
    pub object: ObjectId,
    pub identity_set: IdentitySetId,
    pub q: Iv32,
    pub orientation: i8,
    pub validity_margin: i32,
    pub root_slack: i32,
    pub dedup_owner: u32,
    pub support_sublevel_proof: bool,
}

impl RootRecord {
    pub fn crossing(self) -> Result<Orientation, SweepError> {
        match self.orientation {
            1 => Ok(Orientation::Enter),
            -1 => Ok(Orientation::Exit),
            _ => Err(SweepError::CertificateExhausted),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootOrder {
    Strict {
        count: usize,
    },
    Corridor {
        count: usize,
        first: usize,
        last: usize,
    },
}

pub fn collect_and_order_roots(
    candidates: &[FeatureId],
    roots_by_feature: &[&[RootRecord]],
    output: &mut [RootRecord],
) -> Result<RootOrder, SweepError> {
    if candidates.len() != roots_by_feature.len() {
        return Err(SweepError::InternalInvariant);
    }
    let mut count = 0_usize;
    for (feature, roots) in candidates.iter().copied().zip(roots_by_feature) {
        for root in *roots {
            if root.feature != feature
                || !root.support_sublevel_proof
                || root.validity_margin <= 0
                || root.root_slack <= 0
            {
                return Err(SweepError::CertificateExhausted);
            }
            if root.orientation == 0 {
                let Some(slot) = output.get_mut(count) else {
                    return Err(SweepError::CapacityExceeded);
                };
                *slot = *root;
                count += 1;
                continue;
            }
            let duplicate = output[..count].iter().any(|existing| {
                root.dedup_owner != 0
                    && existing.dedup_owner == root.dedup_owner
                    && existing.q == root.q
                    && existing.orientation == root.orientation
            });
            if duplicate {
                continue;
            }
            let Some(slot) = output.get_mut(count) else {
                return Err(SweepError::CapacityExceeded);
            };
            *slot = *root;
            count += 1;
        }
    }
    for index in 1..count {
        let value = output[index];
        let mut destination = index;
        while destination != 0
            && (value.q.lo, value.q.hi, std::cmp::Reverse(value.feature))
                > (
                    output[destination - 1].q.lo,
                    output[destination - 1].q.hi,
                    std::cmp::Reverse(output[destination - 1].feature),
                )
        {
            output[destination] = output[destination - 1];
            destination -= 1;
        }
        output[destination] = value;
    }
    for index in 1..count {
        if output[index - 1].q.lo <= output[index].q.hi {
            let mut last = index + 1;
            let mut corridor_lo = output[index].q.lo;
            while last < count && output[last].q.hi >= corridor_lo {
                corridor_lo = corridor_lo.min(output[last].q.lo);
                last += 1;
            }
            return Ok(RootOrder::Corridor {
                count,
                first: index - 1,
                last,
            });
        }
    }
    if let Some(index) = output[..count]
        .iter()
        .position(|root| root.orientation == 0)
    {
        return Ok(RootOrder::Corridor {
            count,
            first: index,
            last: index + 1,
        });
    }
    Ok(RootOrder::Strict { count })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QModel {
    pub q0: Iv32,
    pub qx: Iv32,
    pub qxx: Iv32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NormalModel {
    pub nx: Iv32,
    pub ny: Iv32,
    pub nz: Iv32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImplicitDerivativeBundle {
    pub g_x: f64,
    pub g_q: F64Interval,
    pub g_xx: f64,
    pub g_xq: f64,
    pub g_qq: f64,
    pub verifier_remainder: F64Interval,
    pub active_leaf_count: u16,
    pub active_cluster_proven: bool,
    pub nonsmooth: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImplicitJetCandidate {
    pub q0: f64,
    pub q_x: f64,
    pub q_xx: f64,
    pub initial_error: F64Interval,
    pub active_leaf_count: u16,
}

/// Build an untrusted quadratic proposal from a certified derivative bundle.
/// The complete root stays live when this fails; callers must enter a
/// corridor/rebuild tier instead of treating proposal failure as a miss.
pub fn construct_implicit_jet(
    q0: f64,
    derivatives: ImplicitDerivativeBundle,
) -> Result<ImplicitJetCandidate, SweepError> {
    if !q0.is_finite()
        || !derivatives.g_x.is_finite()
        || !derivatives.g_xx.is_finite()
        || !derivatives.g_xq.is_finite()
        || !derivatives.g_qq.is_finite()
        || derivatives.active_leaf_count == 0
        || !derivatives.active_cluster_proven
        || derivatives.nonsmooth
        || derivatives.g_q.contains(0.0)
    {
        return Err(SweepError::CertificateExhausted);
    }
    let g_q_center = derivatives.g_q.lo + (derivatives.g_q.hi - derivatives.g_q.lo) * 0.5;
    if !g_q_center.is_finite() || g_q_center == 0.0 {
        return Err(SweepError::NumericFailure);
    }
    let q_x = -derivatives.g_x / g_q_center;
    let q_xx = -(derivatives.g_xx + 2.0 * derivatives.g_xq * q_x + derivatives.g_qq * q_x * q_x)
        / g_q_center;
    if !q_x.is_finite() || !q_xx.is_finite() {
        return Err(SweepError::NumericFailure);
    }
    Ok(ImplicitJetCandidate {
        q0,
        q_x,
        q_xx,
        initial_error: derivatives.verifier_remainder,
        active_leaf_count: derivatives.active_leaf_count,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RootSheet {
    pub root: RootRecord,
    pub q_model: QModel,
    pub q_domain: Iv32,
    pub q_error: Iv32,
    /// Certified projective inverse-depth derivative with respect to screen u.
    pub q_u: Iv32,
    /// Certified projective inverse-depth derivative with respect to screen v.
    pub q_v: Iv32,
    pub normal_model: NormalModel,
    pub q_order_slack: i32,
    pub root_slack: i32,
    pub feature_slack: i32,
    pub branch_slack: i32,
    pub fixed_q_slack: i32,
    pub expires_at: u16,
    pub method: u8,
    pub composition_shape: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ProofMarginKind {
    #[default]
    Root = 0,
    Feature = 1,
    Order = 2,
    Csg = 3,
    Branch = 4,
    Numeric = 5,
    FixedQ = 6,
    Event = 7,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CertifiedRun {
    pub x0: u16,
    pub x1: u16,
    pub visible: Option<u16>,
    pub sheet_range_start: u16,
    pub sheet_count: u16,
    pub q_model: QModel,
    pub q_error: Iv32,
    pub q_u: Iv32,
    pub q_v: Iv32,
    pub q_order_slack: Iv32,
    pub root_slack: Iv32,
    pub identity: IdentitySetId,
    pub normal_model: NormalModel,
    pub event_left: Option<u16>,
    pub event_right: Option<u16>,
    pub proof_owner: ProofMarginKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CertifiedRunEvidencePoint {
    pub row_y: u16,
    pub point_witness: bool,
    pub normal: Option<[i16; 3]>,
}

/// Decode the point sample carried beside a stable certified-run record.
/// Word 3 always contains row y. Bit 3 of word 14 says the guest rechecked the
/// run centre as a same-identity point witness with overlapping q. The remaining
/// word-3 components are normal claims only when words 10..13 carry an exact
/// point model; a wider cone makes them non-authoritative.
pub fn decode_certified_run_evidence_point(
    words: [u64; 16],
) -> Result<CertifiedRunEvidencePoint, SweepError> {
    let packed_method = words[14] as u32;
    if (words[14] >> 32) as u32 != packed_method {
        return Err(SweepError::InternalInvariant);
    }
    let component = |shift| (words[3] >> shift) as u16 as i16;
    let exact_normal_model = words[10..13]
        .iter()
        .all(|word| *word as u32 == (*word >> 32) as u32);
    Ok(CertifiedRunEvidencePoint {
        row_y: words[3] as u16,
        point_witness: packed_method & 8 != 0,
        normal: exact_normal_model.then(|| [component(16), component(32), component(48)]),
    })
}

/// Decode and recheck the stable 128-byte guest run record.
///
/// Words 0..4 are the compact coverage header and words 4..16 are the
/// concrete `CertifiedRun` evidence projection written by the guest. Header
/// word 3 always stores row y plus three optional sampled-normal components;
/// word-14 bit 3 tags a centre witness and an exact point model in words 10..13
/// says the normal components are a claim. This is
/// deliberately independent of the guest writer: conformance/debug tooling
/// must reject malformed proof records rather than trusting packed fields.
pub fn decode_certified_run_record(words: [u64; 16]) -> Result<CertifiedRun, SweepError> {
    fn pair(word: u64) -> Result<Iv32, SweepError> {
        Iv32::new(word as u32 as i32, (word >> 32) as u32 as i32)
            .map_err(|_| SweepError::InternalInvariant)
    }

    let _ = decode_certified_run_evidence_point(words)?;
    let x0 = (words[0] & 0xffff) as u16;
    let x1 = ((words[0] >> 16) & 0xffff) as u16;
    if x0 >= x1 {
        return Err(SweepError::InternalInvariant);
    }
    let coverage_q = pair(words[2])?;
    let q_model = QModel {
        q0: pair(words[4])?,
        qx: pair(words[5])?,
        qxx: pair(words[6])?,
    };
    let q_error = pair(words[7])?;
    let q_order_slack = pair(words[8])?;
    let root_slack = pair(words[9])?;
    if q_order_slack.lo <= 0 || root_slack.lo <= 0 {
        return Err(SweepError::InternalInvariant);
    }
    let model_lo = i64::from(q_model.q0.lo) + i64::from(q_error.lo);
    let model_hi = i64::from(q_model.q0.hi) + i64::from(q_error.hi);
    if model_lo > i64::from(coverage_q.lo) || model_hi < i64::from(coverage_q.hi) {
        return Err(SweepError::InternalInvariant);
    }
    let packed_method = words[14] as u32 as i32 as u32;
    let proof_owner = match packed_method & 7 {
        0 => ProofMarginKind::Root,
        1 => ProofMarginKind::Feature,
        2 => ProofMarginKind::Order,
        3 => ProofMarginKind::Csg,
        4 => ProofMarginKind::Branch,
        5 => ProofMarginKind::Numeric,
        6 => ProofMarginKind::FixedQ,
        7 => ProofMarginKind::Event,
        _ => unreachable!(),
    };
    if proof_owner == ProofMarginKind::Event {
        return Err(SweepError::InternalInvariant);
    }
    let packed_sheet = words[15] as u32;
    let sheet_count = (packed_sheet >> 16) as u16;
    let event = |value: u16| (value != u16::MAX).then_some(value);
    let event_left = event(words[13] as u32 as u16);
    let event_right = event((words[13] >> 32) as u32 as u16);
    if event_left.is_some() || event_right.is_some() {
        return Err(SweepError::InternalInvariant);
    }
    Ok(CertifiedRun {
        x0,
        x1,
        visible: (sheet_count != 0).then_some(packed_sheet as u16),
        sheet_range_start: 0,
        sheet_count,
        q_model,
        q_error,
        q_u: Iv32::default(),
        q_v: Iv32::default(),
        q_order_slack,
        root_slack,
        identity: IdentitySetId((words[1] >> 32) as u32),
        normal_model: NormalModel {
            nx: pair(words[10])?,
            ny: pair(words[11])?,
            nz: pair(words[12])?,
        },
        event_left,
        event_right,
        proof_owner,
    })
}

pub fn certify_regular_run(
    x0: u16,
    x1: u16,
    sheets: &[RootSheet],
    csg_program: &[CsgInstruction],
    initial_inside_bits: u64,
    completeness: CandidateCompleteness,
    event_left: Option<u16>,
    event_right: Option<u16>,
    telemetry: Option<&mut CertificateTelemetry>,
) -> Result<CertifiedRun, SweepError> {
    if x0 >= x1
        || usize::from(completeness.retained)
            + usize::from(completeness.row_span_excluded)
            + usize::from(completeness.static_excluded)
            + usize::from(completeness.runtime_excluded)
            != usize::from(completeness.indexed)
        || completeness.roots_accounted != completeness.retained
    {
        return Err(SweepError::InternalInvariant);
    }
    if sheets.is_empty() {
        let run = CertifiedRun {
            x0,
            x1,
            visible: None,
            event_left,
            event_right,
            ..CertifiedRun::default()
        };
        if let Some(telemetry) = telemetry {
            telemetry.charge_run(
                x1 - x0,
                RootMethod::MonotoneTube,
                CompositionShape::General,
                ExpiryCause::DomainEnd,
                MarginOwner::Root,
            );
        }
        return Ok(run);
    }
    let mut endpoint = x1;
    let mut owner = MarginOwner::Root;
    let mut minimum = i32::MAX;
    for sheet in sheets {
        for (margin, candidate_owner) in [
            (sheet.root_slack, MarginOwner::Root),
            (sheet.feature_slack, MarginOwner::Feature),
            (sheet.q_order_slack, MarginOwner::Order),
            (sheet.branch_slack, MarginOwner::Branch),
            (sheet.fixed_q_slack, MarginOwner::FixedQ),
        ] {
            if margin <= 0 {
                return Err(SweepError::CertificateExhausted);
            }
            if margin < minimum {
                minimum = margin;
                owner = candidate_owner;
            }
        }
        endpoint = endpoint.min(sheet.expires_at);
    }
    if endpoint <= x0 {
        return Err(SweepError::CertificateExhausted);
    }
    for pair in sheets.windows(2) {
        if pair[0].q_domain.lo <= pair[1].q_domain.hi {
            return Err(SweepError::CertificateExhausted);
        }
    }
    let mut crossings = [(0_u8, Orientation::Enter); 64];
    if sheets.len() > crossings.len() {
        return Err(SweepError::CapacityExceeded);
    }
    for (slot, sheet) in crossings.iter_mut().zip(sheets) {
        *slot = (sheet.root.object.0, sheet.root.crossing()?);
    }
    let visible =
        csg::first_transition(csg_program, initial_inside_bits, &crossings[..sheets.len()])
            .map_err(|_| SweepError::CertificateExhausted)?;
    let visible_sheet = visible
        .map(|index| u16::try_from(index).map_err(|_| SweepError::CapacityExceeded))
        .transpose()?;
    let (
        identity,
        q_model,
        q_error,
        q_u,
        q_v,
        normal_model,
        root_slack,
        q_order_slack,
        method,
        shape,
    ) = if let Some(index) = visible {
        let sheet = sheets[index];
        (
            sheet.root.identity_set,
            sheet.q_model,
            sheet.q_error,
            sheet.q_u,
            sheet.q_v,
            sheet.normal_model,
            Iv32::point(sheet.root_slack),
            Iv32::point(sheet.q_order_slack),
            decode_method(sheet.method)?,
            decode_shape(sheet.composition_shape)?,
        )
    } else {
        (
            IdentitySetId(0),
            QModel::default(),
            Iv32::default(),
            Iv32::default(),
            Iv32::default(),
            NormalModel::default(),
            Iv32::point(minimum),
            Iv32::point(minimum),
            RootMethod::MonotoneTube,
            CompositionShape::General,
        )
    };
    let run = CertifiedRun {
        x0,
        x1: endpoint,
        visible: visible_sheet,
        sheet_range_start: 0,
        sheet_count: u16::try_from(sheets.len()).map_err(|_| SweepError::CapacityExceeded)?,
        q_model,
        q_error,
        q_u,
        q_v,
        q_order_slack,
        root_slack,
        identity,
        normal_model,
        event_left,
        event_right,
        proof_owner: margin_owner_kind(owner),
    };
    if let Some(telemetry) = telemetry {
        telemetry.charge_run(
            endpoint - x0,
            method,
            shape,
            if endpoint == x1 {
                ExpiryCause::DomainEnd
            } else {
                ExpiryCause::Residual
            },
            owner,
        );
        telemetry.charge_density(
            usize::from(completeness.retained),
            sheets.len(),
            usize::from(event_left.is_some()) + usize::from(event_right.is_some()),
            usize::from(completeness.indexed),
        );
    }
    Ok(run)
}

fn margin_owner_kind(owner: MarginOwner) -> ProofMarginKind {
    match owner {
        MarginOwner::Root => ProofMarginKind::Root,
        MarginOwner::Feature => ProofMarginKind::Feature,
        MarginOwner::Order => ProofMarginKind::Order,
        MarginOwner::Branch => ProofMarginKind::Branch,
        MarginOwner::FixedQ => ProofMarginKind::FixedQ,
        MarginOwner::Csg => ProofMarginKind::Csg,
        MarginOwner::Numeric => ProofMarginKind::Numeric,
        MarginOwner::Event => ProofMarginKind::Event,
    }
}

fn decode_method(value: u8) -> Result<RootMethod, SweepError> {
    match value {
        0 => Ok(RootMethod::BernsteinFaces),
        1 => Ok(RootMethod::MonotoneTube),
        2 => Ok(RootMethod::Krawczyk),
        _ => Err(SweepError::InternalInvariant),
    }
}

fn decode_shape(value: u8) -> Result<CompositionShape, SweepError> {
    match value {
        0 => Ok(CompositionShape::General),
        1 => Ok(CompositionShape::Plane),
        2 => Ok(CompositionShape::Sphere),
        3 => Ok(CompositionShape::Torus),
        _ => Err(SweepError::InternalInvariant),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowProposal {
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProposalCounts {
    pub proposed: u16,
    pub revalidated: u16,
    pub new: u16,
}

pub fn seed_next_row(
    mode: RowProposal,
    complete_candidates: &[FeatureId],
    previous_features: &[FeatureId],
    output: &mut [FeatureId],
    mut revalidate: impl FnMut(FeatureId) -> Result<bool, SweepError>,
) -> Result<(usize, ProposalCounts), SweepError> {
    let mut count = 0_usize;
    let mut counts = ProposalCounts::default();
    if mode == RowProposal::Enabled {
        for feature in previous_features {
            if complete_candidates.binary_search(feature).is_ok() {
                counts.proposed = counts
                    .proposed
                    .checked_add(1)
                    .ok_or(SweepError::CapacityExceeded)?;
                if revalidate(*feature)? {
                    let Some(slot) = output.get_mut(count) else {
                        return Err(SweepError::CapacityExceeded);
                    };
                    *slot = *feature;
                    count += 1;
                    counts.revalidated = counts
                        .revalidated
                        .checked_add(1)
                        .ok_or(SweepError::CapacityExceeded)?;
                }
            }
        }
    }
    for feature in complete_candidates {
        if output[..count].contains(feature) {
            continue;
        }
        let Some(slot) = output.get_mut(count) else {
            return Err(SweepError::CapacityExceeded);
        };
        *slot = *feature;
        count += 1;
        counts.new = counts
            .new
            .checked_add(1)
            .ok_or(SweepError::CapacityExceeded)?;
    }
    for index in 1..count {
        let value = output[index];
        let mut destination = index;
        while destination != 0 && value < output[destination - 1] {
            output[destination] = output[destination - 1];
            destination -= 1;
        }
        output[destination] = value;
    }
    Ok((count, counts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn derivative_bundle(
        g_x: f64,
        g_q: F64Interval,
        g_xx: f64,
        g_xq: f64,
        g_qq: f64,
    ) -> ImplicitDerivativeBundle {
        ImplicitDerivativeBundle {
            g_x,
            g_q,
            g_xx,
            g_xq,
            g_qq,
            verifier_remainder: F64Interval::new(-1.0e-12, 1.0e-12).unwrap(),
            active_leaf_count: 1,
            active_cluster_proven: true,
            nonsmooth: false,
        }
    }

    #[test]
    fn plane_implicit_jet_is_exactly_affine() {
        let jet = construct_implicit_jet(
            3.0,
            derivative_bundle(1.0, F64Interval::point(2.0).unwrap(), 0.0, 0.0, 0.0),
        )
        .unwrap();
        assert_eq!(jet.q_x, -0.5);
        assert_eq!(jet.q_xx, 0.0);
        assert_eq!(jet.active_leaf_count, 1);
    }

    #[test]
    fn sphere_implicit_jet_matches_analytic_derivatives() {
        let x = 0.25_f64;
        let q = (1.0 - x * x).sqrt();
        let jet = construct_implicit_jet(
            q,
            derivative_bundle(2.0 * x, F64Interval::point(2.0 * q).unwrap(), 2.0, 0.0, 2.0),
        )
        .unwrap();
        let expected_qx = -x / q;
        let expected_qxx = -1.0 / (q * q * q);
        assert!((jet.q_x - expected_qx).abs() < 1.0e-14);
        assert!((jet.q_xx - expected_qxx).abs() < 1.0e-14);
    }

    #[test]
    fn smooth_jet_requires_a_proven_active_cluster_and_retains_the_root_on_failure() {
        let retained = root(7, 0, 100, 101, 1);
        let mut derivatives =
            derivative_bundle(1.0, F64Interval::new(-1.0, 1.0).unwrap(), 0.0, 0.0, 0.0);
        derivatives.active_leaf_count = 2;
        derivatives.active_cluster_proven = false;
        assert_eq!(
            construct_implicit_jet(1.0, derivatives),
            Err(SweepError::CertificateExhausted)
        );
        assert_eq!(retained, root(7, 0, 100, 101, 1));

        derivatives.g_q = F64Interval::new(2.0, 2.25).unwrap();
        derivatives.active_cluster_proven = true;
        let jet = construct_implicit_jet(1.0, derivatives).unwrap();
        assert_eq!(jet.active_leaf_count, 2);
    }

    fn root(feature: u32, object: u8, q_lo: i32, q_hi: i32, orientation: i8) -> RootRecord {
        RootRecord {
            feature: FeatureId(feature),
            object: ObjectId(object),
            identity_set: IdentitySetId(feature + 1),
            q: Iv32::new(q_lo, q_hi).unwrap(),
            orientation,
            validity_margin: 4,
            root_slack: 3,
            dedup_owner: 0,
            support_sublevel_proof: true,
        }
    }

    #[test]
    fn candidates_are_structural_sorted_and_every_exclusion_has_margin() {
        let indexed = [
            IndexedFeature {
                id: FeatureId(9),
                row_start: 0,
                row_end: 8,
                exclusion: ExclusionResult::Retain,
            },
            IndexedFeature {
                id: FeatureId(2),
                row_start: 0,
                row_end: 8,
                exclusion: ExclusionResult::Retain,
            },
            IndexedFeature {
                id: FeatureId(7),
                row_start: 0,
                row_end: 8,
                exclusion: ExclusionResult::Static { margin: 12 },
            },
        ];
        let mut output = [FeatureId(0); 3];
        let (count, proof) = enumerate_row_candidates(&indexed, 3, &mut output).unwrap();
        assert_eq!(&output[..count], &[FeatureId(2), FeatureId(9)]);
        assert_eq!(proof.static_excluded, 1);
        assert_eq!(proof.minimum_exclusion_margin, 12);
    }

    #[test]
    fn support_sublevel_roots_include_multiple_crossings_and_sort_front_to_back() {
        let candidates = [FeatureId(0), FeatureId(1)];
        let roots0 = [root(0, 0, 100, 101, 1), root(0, 0, 50, 51, -1)];
        let roots1 = [root(1, 1, 75, 76, 1)];
        let mut output = [RootRecord::default(); 4];
        let result =
            collect_and_order_roots(&candidates, &[&roots0, &roots1], &mut output).unwrap();
        assert_eq!(result, RootOrder::Strict { count: 3 });
        assert_eq!(
            output[..3].iter().map(|root| root.q.lo).collect::<Vec<_>>(),
            [100, 75, 50]
        );
    }

    #[test]
    fn overlapping_roots_create_a_corridor_instead_of_using_ids() {
        let candidates = [FeatureId(0), FeatureId(1)];
        let first = [root(0, 0, 99, 101, 1)];
        let second = [root(1, 1, 100, 102, 1)];
        let mut output = [RootRecord::default(); 2];
        assert!(matches!(
            collect_and_order_roots(&candidates, &[&first, &second], &mut output).unwrap(),
            RootOrder::Corridor { .. }
        ));
    }

    #[test]
    fn tangency_corridor_retains_every_other_complete_root() {
        let candidates = [FeatureId(0), FeatureId(1)];
        let tangent = [root(0, 0, 50, 51, 0)];
        let crossings = [root(1, 1, 100, 101, 1), root(1, 1, 25, 26, -1)];
        let mut output = [RootRecord::default(); 3];
        assert_eq!(
            collect_and_order_roots(&candidates, &[&tangent, &crossings], &mut output).unwrap(),
            RootOrder::Corridor {
                count: 3,
                first: 1,
                last: 2,
            }
        );
        assert_eq!(
            output.iter().map(|root| root.q.lo).collect::<Vec<_>>(),
            [100, 50, 25]
        );
    }

    #[test]
    fn hard_csg_camera_inside_selects_the_first_exit() {
        let root = root(0, 0, 100, 101, -1);
        let sheet = RootSheet {
            root,
            q_model: QModel {
                q0: root.q,
                qx: Iv32::point(0),
                qxx: Iv32::point(0),
            },
            q_domain: root.q,
            q_error: Iv32::point(1),
            q_u: Iv32::point(0),
            q_v: Iv32::point(0),
            normal_model: NormalModel::default(),
            q_order_slack: 8,
            root_slack: 8,
            feature_slack: 8,
            branch_slack: 8,
            fixed_q_slack: 8,
            expires_at: 32,
            method: 0,
            composition_shape: 2,
        };
        let proof = CandidateCompleteness {
            indexed: 1,
            retained: 1,
            roots_accounted: 1,
            ..CandidateCompleteness::default()
        };
        let run = certify_regular_run(
            0,
            32,
            &[sheet],
            &[CsgInstruction::Object(0)],
            1,
            proof,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(run.visible, Some(0));
        assert_eq!(run.identity, IdentitySetId(1));
    }

    #[test]
    fn certificate_failure_never_becomes_background() {
        let root = root(0, 0, 100, 101, 1);
        let sheet = RootSheet {
            root,
            q_domain: root.q,
            root_slack: 0,
            feature_slack: 1,
            q_order_slack: 1,
            branch_slack: 1,
            fixed_q_slack: 1,
            expires_at: 2,
            ..RootSheet::default()
        };
        let proof = CandidateCompleteness {
            indexed: 1,
            retained: 1,
            roots_accounted: 1,
            ..CandidateCompleteness::default()
        };
        assert_eq!(
            certify_regular_run(
                0,
                2,
                &[sheet],
                &[CsgInstruction::Object(0)],
                0,
                proof,
                None,
                None,
                None,
            ),
            Err(SweepError::CertificateExhausted)
        );
    }

    #[test]
    fn unaccounted_retained_candidate_cannot_become_background() {
        let proof = CandidateCompleteness {
            indexed: 1,
            retained: 1,
            roots_accounted: 0,
            ..CandidateCompleteness::default()
        };
        assert_eq!(
            certify_regular_run(
                0,
                8,
                &[],
                &[CsgInstruction::Object(0)],
                0,
                proof,
                None,
                None,
                None,
            ),
            Err(SweepError::InternalInvariant)
        );
    }

    #[test]
    fn row_proposals_cannot_suppress_new_structural_features() {
        let complete = [FeatureId(1), FeatureId(2), FeatureId(9)];
        let previous = [FeatureId(1), FeatureId(2)];
        let mut enabled = [FeatureId(0); 3];
        let mut disabled = [FeatureId(0); 3];
        let (enabled_count, counts) = seed_next_row(
            RowProposal::Enabled,
            &complete,
            &previous,
            &mut enabled,
            |_| Ok(true),
        )
        .unwrap();
        let (disabled_count, _) = seed_next_row(
            RowProposal::Disabled,
            &complete,
            &previous,
            &mut disabled,
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(&enabled[..enabled_count], &disabled[..disabled_count]);
        assert_eq!(&enabled[..enabled_count], &complete);
        assert_eq!(counts.new, 1);
    }

    #[test]
    fn guest_run_record_decodes_to_the_lean_shaped_certificate() {
        fn packed_pair(lo: i32, hi: i32) -> u64 {
            u64::from(lo as u32) | (u64::from(hi as u32) << 32)
        }

        let mut words = [0_u64; 16];
        words[0] = 3 | (11 << 16);
        words[1] = 5 | (91 << 32);
        words[2] = packed_pair(990, 1010);
        words[3] = 1 | (0x0004_0100 << 32);
        words[4] = packed_pair(1000, 1000);
        words[5] = packed_pair(2, 2);
        words[6] = packed_pair(0, 0);
        words[7] = packed_pair(-10, 10);
        words[8] = packed_pair(7, 9);
        words[9] = packed_pair(5, 6);
        words[10] = packed_pair(-32_767, 32_767);
        words[11] = packed_pair(-32_767, 32_767);
        words[12] = packed_pair(-32_767, 32_767);
        words[13] = packed_pair(65_535, 65_535);
        words[14] = packed_pair(0x00ff_0200, 0x00ff_0200);
        words[15] = packed_pair(4 | (1 << 16), 4 | (1 << 16));

        let run = decode_certified_run_record(words).unwrap();
        assert_eq!((run.x0, run.x1), (3, 11));
        assert_eq!(run.visible, Some(4));
        assert_eq!(run.sheet_count, 1);
        assert_eq!(run.identity, IdentitySetId(91));
        assert_eq!(run.q_model.q0, Iv32::point(1000));
        assert_eq!(run.q_error, Iv32::new(-10, 10).unwrap());
        assert_eq!(run.proof_owner, ProofMarginKind::Root);
        assert_eq!(
            decode_certified_run_evidence_point(words).unwrap(),
            CertifiedRunEvidencePoint {
                row_y: 1,
                point_witness: false,
                normal: None,
            }
        );

        words[14] = packed_pair(0x00ff_0208, 0x00ff_0208);
        words[10] = packed_pair(0, 0);
        words[11] = packed_pair(256, 256);
        words[12] = packed_pair(4, 4);
        assert_eq!(
            decode_certified_run_evidence_point(words).unwrap(),
            CertifiedRunEvidencePoint {
                row_y: 1,
                point_witness: true,
                normal: Some([0, 256, 4]),
            }
        );
    }

    #[test]
    fn guest_run_record_rechecker_rejects_nonpositive_slack() {
        fn packed_pair(lo: i32, hi: i32) -> u64 {
            u64::from(lo as u32) | (u64::from(hi as u32) << 32)
        }

        let mut words = [0_u64; 16];
        words[0] = 1 | (2 << 16);
        words[2] = packed_pair(0, 0);
        words[4] = packed_pair(0, 0);
        words[5] = packed_pair(0, 0);
        words[6] = packed_pair(0, 0);
        words[7] = packed_pair(0, 0);
        words[8] = packed_pair(0, 1);
        words[9] = packed_pair(1, 1);
        words[10] = packed_pair(0, 0);
        words[11] = packed_pair(0, 0);
        words[12] = packed_pair(0, 0);
        words[13] = packed_pair(65_535, 65_535);
        words[14] = packed_pair(0, 0);
        words[15] = packed_pair(0, 0);
        assert_eq!(
            decode_certified_run_record(words),
            Err(SweepError::InternalInvariant)
        );
    }
}
