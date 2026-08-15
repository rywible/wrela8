//! Certified scalar integer quadratic recurrence for inverse depth.

use super::iv32::{FixedDomain, Iv32, NumericError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QRunScalar {
    pub q: i32,
    pub dq: i32,
    pub ddq: i32,
    pub domain: FixedDomain,
    pub error_radius: i32,
    pub microtile_width: u8,
    step: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectedDomain {
    pub domain: FixedDomain,
    pub maximum_raw: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SealedSetup {
    pub run: QRunScalar,
    pub maximum_raw: i32,
}

/// Selects the finest v1 dyadic exponent that encloses a symmetric real
/// magnitude in signed i32 storage. This is a cold compiler operation.
pub fn select_domain(maximum_abs: f64) -> Result<SelectedDomain, NumericError> {
    if !maximum_abs.is_finite() || maximum_abs < 0.0 {
        return Err(NumericError::NonFinite);
    }
    for exponent in super::iv32::MIN_EXPONENT..=super::iv32::MAX_EXPONENT {
        let scale = 2.0_f64.powi(i32::from(exponent));
        let raw = (maximum_abs / scale).ceil();
        if raw <= f64::from(i32::MAX) {
            return Ok(SelectedDomain {
                domain: FixedDomain::full(exponent),
                maximum_raw: raw as i32,
            });
        }
    }
    Err(NumericError::Overflow)
}

pub fn select_integer_domain(maximum_abs: i64) -> Result<SelectedDomain, NumericError> {
    if maximum_abs < 0 {
        return Err(NumericError::InvalidDomain);
    }
    let selected = select_domain(maximum_abs as f64)?;
    if selected.domain.exponent >= -30 {
        return Ok(selected);
    }
    let maximum_raw = maximum_abs
        .checked_mul(1_i64 << 30)
        .ok_or(NumericError::Overflow)?;
    Ok(SelectedDomain {
        domain: FixedDomain::full(-30),
        maximum_raw: i32::try_from(maximum_raw).map_err(|_| NumericError::Overflow)?,
    })
}

pub fn setup(
    q: Iv32,
    dq: Iv32,
    ddq: Iv32,
    domain: FixedDomain,
    requested_width: u8,
    model_error_radius: i32,
) -> Result<QRunScalar, NumericError> {
    domain.validate()?;
    if model_error_radius < 0 || requested_width == 0 {
        return Err(NumericError::InvalidDomain);
    }
    q.intersect(q, domain)?;
    dq.intersect(dq, domain)?;
    ddq.intersect(ddq, domain)?;
    let maximum = requested_width.min(64);
    let mut width = 32_u8.min(maximum).next_power_of_two();
    if width > maximum {
        width /= 2;
    }
    loop {
        if width == 0 {
            return Err(NumericError::Overflow);
        }
        if recurrence_fits(q, dq, ddq, width, domain) {
            let q_center = midpoint(q)?;
            let dq_center = midpoint(dq)?;
            let ddq_center = midpoint(ddq)?;
            let q_radius = half_width_ceil(q)?;
            let dq_radius = half_width_ceil(dq)?;
            let ddq_radius = half_width_ceil(ddq)?;
            let linear_radius = i64::from(dq_radius) * i64::from(width);
            let quadratic_radius =
                i64::from(ddq_radius) * i64::from(width) * i64::from(width.saturating_sub(1)) / 2;
            let coefficient_radius = i32::try_from(
                i64::from(q_radius)
                    .checked_add(linear_radius)
                    .and_then(|value| value.checked_add(quadratic_radius))
                    .and_then(|value| value.checked_add(i64::from(model_error_radius)))
                    .ok_or(NumericError::Overflow)?,
            )
            .map_err(|_| NumericError::Overflow)?;
            return Ok(QRunScalar {
                q: q_center,
                dq: dq_center,
                ddq: ddq_center,
                domain,
                error_radius: coefficient_radius,
                microtile_width: width,
                step: 0,
            });
        }
        width /= 2;
    }
}

/// Cold setup for integer-valued q coefficients. The recurrence envelope is
/// used to select a shared dyadic domain before coefficients are converted
/// outward and the ordinary checked setup is applied.
pub fn setup_from_integer_coefficients(
    q: i32,
    dq: i32,
    ddq: i32,
    requested_width: u8,
    model_error_radius: i32,
) -> Result<QRunScalar, NumericError> {
    if requested_width == 0 || model_error_radius < 0 {
        return Err(NumericError::InvalidDomain);
    }
    let width = i128::from(requested_width.min(64));
    let triangular = width.checked_mul(width - 1).ok_or(NumericError::Overflow)? / 2;
    let envelope = i128::from(q).abs()
        + width
            .checked_mul(i128::from(dq).abs())
            .ok_or(NumericError::Overflow)?
        + triangular
            .checked_mul(i128::from(ddq).abs())
            .ok_or(NumericError::Overflow)?;
    let selected =
        select_integer_domain(i64::try_from(envelope).map_err(|_| NumericError::Overflow)?)?;
    let source = FixedDomain::full(0);
    setup(
        Iv32::point(q).convert(source, selected.domain)?,
        Iv32::point(dq).convert(source, selected.domain)?,
        Iv32::point(ddq).convert(source, selected.domain)?,
        selected.domain,
        requested_width,
        model_error_radius,
    )
}

/// Select a shared domain for the complete compiler-certified q/dq/ddq
/// envelope. The returned radius conservatively treats every coefficient in
/// the envelope as possible, and also includes outward conversion and the
/// supplied real-valued model/Taylor/reset error.
pub fn setup_from_real_envelopes(
    q_lo: f64,
    q_hi: f64,
    dq_abs: f64,
    ddq_abs: f64,
    requested_width: u8,
    model_error_abs: f64,
) -> Result<SealedSetup, NumericError> {
    if !q_lo.is_finite()
        || !q_hi.is_finite()
        || !dq_abs.is_finite()
        || !ddq_abs.is_finite()
        || !model_error_abs.is_finite()
        || q_lo > q_hi
        || dq_abs < 0.0
        || ddq_abs < 0.0
        || model_error_abs < 0.0
        || requested_width == 0
    {
        return Err(NumericError::InvalidDomain);
    }
    let policy = wrela_machine::pixels::derive_fixed_q_envelope_v1(
        q_lo,
        q_hi,
        dq_abs,
        ddq_abs,
        requested_width,
        model_error_abs,
    )
    .ok_or(NumericError::Overflow)?;
    let domain = FixedDomain::full(policy.exponent);
    let q = quantize_real_interval(q_lo, q_hi, policy.exponent).ok_or(NumericError::Overflow)?;
    let dq =
        quantize_real_interval(-dq_abs, dq_abs, policy.exponent).ok_or(NumericError::Overflow)?;
    let ddq =
        quantize_real_interval(-ddq_abs, ddq_abs, policy.exponent).ok_or(NumericError::Overflow)?;
    let scale = 2.0_f64.powi(i32::from(policy.exponent));
    let model_error_radius = i32::try_from((model_error_abs / scale).ceil() as i64)
        .map_err(|_| NumericError::Overflow)?;
    let run = setup(q, dq, ddq, domain, requested_width, model_error_radius)?;
    let maximum_raw = recurrence_maximum_abs(q, dq, ddq, run.microtile_width)?;
    if run.microtile_width != policy.reset_width
        || run.error_radius != policy.error_radius
        || maximum_raw != policy.maximum_raw
    {
        return Err(NumericError::DomainMismatch);
    }
    Ok(SealedSetup { run, maximum_raw })
}

impl QRunScalar {
    pub fn advance(&mut self) -> Result<i32, NumericError> {
        if self.step >= self.microtile_width {
            return Err(NumericError::CapacityExceeded);
        }
        let q = self.q.checked_add(self.dq).ok_or(NumericError::Overflow)?;
        let dq = self
            .dq
            .checked_add(self.ddq)
            .ok_or(NumericError::Overflow)?;
        if q < self.domain.min_raw
            || q > self.domain.max_raw
            || dq < self.domain.min_raw
            || dq > self.domain.max_raw
        {
            return Err(NumericError::Overflow);
        }
        self.q = q;
        self.dq = dq;
        self.step += 1;
        Ok(q)
    }

    /// Re-anchor the sealed recurrence at an absolute step without carrying
    /// the hot-loop step counter across a microtile reset.
    pub fn reset_at(self, steps: u32) -> Result<Self, NumericError> {
        let n = i64::from(steps);
        let triangular = n
            .checked_mul(n.saturating_sub(1))
            .ok_or(NumericError::Overflow)?
            / 2;
        let q = i64::from(self.q)
            .checked_add(
                n.checked_mul(i64::from(self.dq))
                    .ok_or(NumericError::Overflow)?,
            )
            .and_then(|value| {
                triangular
                    .checked_mul(i64::from(self.ddq))
                    .and_then(|term| value.checked_add(term))
            })
            .ok_or(NumericError::Overflow)?;
        let dq = i64::from(self.dq)
            .checked_add(
                n.checked_mul(i64::from(self.ddq))
                    .ok_or(NumericError::Overflow)?,
            )
            .ok_or(NumericError::Overflow)?;
        let q = i32::try_from(q).map_err(|_| NumericError::Overflow)?;
        let dq = i32::try_from(dq).map_err(|_| NumericError::Overflow)?;
        if q < self.domain.min_raw
            || q > self.domain.max_raw
            || dq < self.domain.min_raw
            || dq > self.domain.max_raw
        {
            return Err(NumericError::Overflow);
        }
        Ok(Self {
            q,
            dq,
            step: 0,
            ..self
        })
    }
}

fn quantize_real_interval(lo: f64, hi: f64, exponent: i16) -> Option<Iv32> {
    let scale = 2.0_f64.powi(i32::from(exponent));
    let lo = (lo / scale).floor();
    let hi = (hi / scale).ceil();
    if lo < f64::from(i32::MIN) || hi > f64::from(i32::MAX) {
        return None;
    }
    Iv32::new(lo as i32, hi as i32).ok()
}

fn recurrence_maximum_abs(q: Iv32, dq: Iv32, ddq: Iv32, width: u8) -> Result<i32, NumericError> {
    let mut maximum = 0_i64;
    for (q0, d1, d2) in [
        (q.lo, dq.lo, ddq.lo),
        (q.lo, dq.lo, ddq.hi),
        (q.lo, dq.hi, ddq.lo),
        (q.lo, dq.hi, ddq.hi),
        (q.hi, dq.lo, ddq.lo),
        (q.hi, dq.lo, ddq.hi),
        (q.hi, dq.hi, ddq.lo),
        (q.hi, dq.hi, ddq.hi),
    ] {
        let mut q_value = i64::from(q0);
        let mut dq_value = i64::from(d1);
        maximum = maximum
            .max(q_value.unsigned_abs() as i64)
            .max(dq_value.unsigned_abs() as i64)
            .max(i64::from(d2).unsigned_abs() as i64);
        for _ in 0..width {
            q_value = q_value
                .checked_add(dq_value)
                .ok_or(NumericError::Overflow)?;
            dq_value = dq_value
                .checked_add(i64::from(d2))
                .ok_or(NumericError::Overflow)?;
            maximum = maximum
                .max(q_value.unsigned_abs() as i64)
                .max(dq_value.unsigned_abs() as i64);
        }
    }
    i32::try_from(maximum).map_err(|_| NumericError::Overflow)
}

pub fn quantized_nearer(a: &QRunScalar, b: &QRunScalar) -> bool {
    quantized_nearer_raw(a.q, a.error_radius, b.q, b.error_radius)
}

pub fn quantized_nearer_raw(a_q: i32, a_error: i32, b_q: i32, b_error: i32) -> bool {
    a_error >= 0
        && b_error >= 0
        && i64::from(a_q) - i64::from(a_error) > i64::from(b_q) + i64::from(b_error)
}

fn recurrence_fits(q: Iv32, dq: Iv32, ddq: Iv32, width: u8, domain: FixedDomain) -> bool {
    for (q0, d1, d2) in [
        (q.lo, dq.lo, ddq.lo),
        (q.lo, dq.lo, ddq.hi),
        (q.lo, dq.hi, ddq.lo),
        (q.lo, dq.hi, ddq.hi),
        (q.hi, dq.lo, ddq.lo),
        (q.hi, dq.lo, ddq.hi),
        (q.hi, dq.hi, ddq.lo),
        (q.hi, dq.hi, ddq.hi),
    ] {
        let mut q = i64::from(q0);
        let mut dq = i64::from(d1);
        for _ in 0..width {
            q += dq;
            dq += i64::from(d2);
            if q < i64::from(domain.min_raw)
                || q > i64::from(domain.max_raw)
                || dq < i64::from(domain.min_raw)
                || dq > i64::from(domain.max_raw)
            {
                return false;
            }
        }
    }
    true
}

fn midpoint(interval: Iv32) -> Result<i32, NumericError> {
    i32::try_from(
        i64::from(interval.lo) + (i64::from(interval.hi) - i64::from(interval.lo)).div_euclid(2),
    )
    .map_err(|_| NumericError::Overflow)
}

fn half_width_ceil(interval: Iv32) -> Result<i32, NumericError> {
    let width = i64::from(interval.hi) - i64::from(interval.lo);
    i32::try_from((width + 1) / 2).map_err(|_| NumericError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recurrence_matches_closed_form_bit_for_bit() {
        let domain = FixedDomain::full(-8);
        let mut run = setup(
            Iv32::point(100),
            Iv32::point(7),
            Iv32::point(-1),
            domain,
            32,
            0,
        )
        .unwrap();
        for n in 1_i32..=32 {
            let expected = 100 + n * 7 + n * (n - 1) / 2 * -1;
            assert_eq!(run.advance().unwrap(), expected);
        }
    }

    #[test]
    fn reset_anchor_matches_scalar_steps() {
        let domain = FixedDomain::full(-8);
        let run = setup(
            Iv32::point(100),
            Iv32::point(7),
            Iv32::point(-1),
            domain,
            32,
            0,
        )
        .unwrap();
        let mut scalar = run;
        for step in 0..=16 {
            assert_eq!(run.reset_at(step).unwrap().q, scalar.q);
            if step != 16 {
                scalar.advance().unwrap();
            }
        }
    }

    #[test]
    fn near_overflow_shrinks_or_fails_explicitly() {
        let domain = FixedDomain::full(0);
        let run = setup(
            Iv32::point(i32::MAX - 20),
            Iv32::point(10),
            Iv32::point(0),
            domain,
            32,
            0,
        );
        assert!(
            matches!(
                run,
                Ok(QRunScalar {
                    microtile_width: 1 | 2,
                    ..
                })
            ) || run == Err(NumericError::Overflow)
        );
    }

    #[test]
    fn setup_checks_the_first_evolved_delta() {
        assert_eq!(
            setup_from_integer_coefficients(0, i32::MAX, i32::MAX, 1, 0),
            Err(NumericError::Overflow)
        );
    }

    #[test]
    fn q_order_accounts_for_both_radii() {
        let domain = FixedDomain::full(0);
        let near = setup(
            Iv32::new(99, 101).unwrap(),
            Iv32::point(0),
            Iv32::point(0),
            domain,
            1,
            0,
        )
        .unwrap();
        let far = setup(
            Iv32::new(97, 99).unwrap(),
            Iv32::point(0),
            Iv32::point(0),
            domain,
            1,
            0,
        )
        .unwrap();
        assert!(!quantized_nearer(&near, &far));
    }

    #[test]
    fn domain_selection_uses_exponents_before_rejecting() {
        assert_eq!(select_domain(2.0_f64.powi(31)).unwrap().domain.exponent, 1);
        assert_eq!(select_domain(2.0_f64.powi(95)), Err(NumericError::Overflow));
        let selected = select_domain(1.0).unwrap();
        assert!(selected.domain.exponent < 0);
        assert!(selected.maximum_raw > 0);
    }

    #[test]
    fn integer_setup_selects_and_seals_shared_domain_and_width() {
        let run = setup_from_integer_coefficients(8, -3, 1, 32, 2).unwrap();
        assert!((-96..=63).contains(&run.domain.exponent));
        assert!(run.microtile_width.is_power_of_two());
        assert!(run.microtile_width <= 32);
        assert!(run.error_radius >= 2);
    }

    #[test]
    fn real_envelope_setup_seals_recurrence_and_error_radius() {
        assert!(
            wrela_machine::pixels::derive_fixed_q_envelope_v1(
                1.0 / 128.0,
                10.0,
                10.0,
                20.0,
                64,
                0.0,
            )
            .is_some()
        );
        let sealed = setup_from_real_envelopes(1.0 / 128.0, 10.0, 10.0, 20.0, 64, 0.0).unwrap();
        assert_eq!(sealed.run.microtile_width, 32);
        assert!(sealed.maximum_raw > 0);
        assert!(sealed.run.error_radius > 0);
        assert!((-96..=63).contains(&sealed.run.domain.exponent));
    }

    #[test]
    fn real_envelope_setup_exhausts_the_v1_exponent_range() {
        assert_eq!(
            setup_from_real_envelopes(0.0, 2.0_f64.powi(95), 0.0, 0.0, 32, 0.0),
            Err(NumericError::Overflow)
        );
    }

    #[test]
    fn real_quadratic_truth_stays_inside_the_independent_error_envelope() {
        struct TruthCase {
            q_interval: (f64, f64),
            q: f64,
            dq: f64,
            ddq: f64,
            requested_width: u8,
            model_error: f64,
        }
        let tiny = 2.0_f64.powi(-96);
        let cases = [
            TruthCase {
                q_interval: (0.09375, 0.125),
                q: 0.1,
                dq: -0.003_125,
                ddq: 0.000_21,
                requested_width: 64,
                model_error: 0.000_37,
            },
            TruthCase {
                q_interval: (32.0 * tiny, 96.0 * tiny),
                q: 65.0 * tiny,
                dq: -2.0 * tiny,
                ddq: tiny,
                requested_width: 8,
                model_error: 3.0 * tiny,
            },
            TruthCase {
                q_interval: (2.0_f64.powi(60), 2.0_f64.powi(60) + 2.0_f64.powi(52)),
                q: 2.0_f64.powi(60) + 2.0_f64.powi(51),
                dq: -2.0_f64.powi(54),
                ddq: 2.0_f64.powi(48),
                requested_width: 16,
                model_error: 2.0_f64.powi(44),
            },
        ];
        let mut observed_widths = std::collections::BTreeSet::new();
        for case in cases {
            let dq_abs = case.dq.abs() + case.model_error;
            let ddq_abs = case.ddq.abs() + case.model_error;
            let sealed = setup_from_real_envelopes(
                case.q_interval.0,
                case.q_interval.1,
                dq_abs,
                ddq_abs,
                case.requested_width,
                case.model_error,
            )
            .unwrap();
            observed_widths.insert(sealed.run.microtile_width);
            let scale = 2.0_f64.powi(i32::from(sealed.run.domain.exponent));
            let real_radius = f64::from(sealed.run.error_radius) * scale;
            let mut run = sealed.run;
            for step in 0..=usize::from(run.microtile_width) {
                let n = step as f64;
                let alternating_model_error = if step & 1 == 0 {
                    case.model_error
                } else {
                    -case.model_error
                };
                let truth = case.q
                    + n * case.dq
                    + (n * (n - 1.0) / 2.0) * case.ddq
                    + alternating_model_error;
                let code = f64::from(run.q) * scale;
                assert!(
                    (truth - code).abs() <= real_radius + scale * 1.0e-9,
                    "step {step}, exponent {}, width {}: truth={truth:e}, code={code:e}, radius={real_radius:e}",
                    run.domain.exponent,
                    run.microtile_width
                );
                if step < usize::from(run.microtile_width) {
                    run.advance().unwrap();
                }
            }
        }
        assert!(
            observed_widths.len() >= 2,
            "truth oracle must cover more than one reset width"
        );
    }
}
