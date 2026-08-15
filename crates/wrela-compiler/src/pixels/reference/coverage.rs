//! Analytic unit-box coverage with exact rational line clipping.

use super::iv32::{FixedDomain, Iv32, NumericError};
use super::poly::{Quadratic2, exact_quadratic_range};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HalfPlane {
    /// Foreground is `a*x + b*y + c >= 0`.
    pub a: i64,
    pub b: i64,
    pub c: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryOwner {
    LowerOrLeft,
    UpperOrRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuadraticGraph {
    /// `y(x) = c + b*x + a*x²`, with y coefficients in coverage raw units.
    pub c: i32,
    pub b: i32,
    pub a: i32,
}

/// Returns the outward raw x coordinate of the sole interior derivative zero.
pub fn quadratic_monotone_split(
    graph: QuadraticGraph,
    coverage_domain: FixedDomain,
) -> Result<Option<Iv32>, NumericError> {
    let scale = i128::from(coverage_scale(coverage_domain)?);
    if graph.a == 0 {
        return Ok(None);
    }
    let numerator = -i128::from(graph.b) * scale;
    let denominator = 2_i128 * i128::from(graph.a);
    let rational = Rational::new(numerator, denominator)?;
    if !rational.in_unit_scaled(scale)? {
        return Ok(None);
    }
    Iv32::new(
        i32::try_from(rational.floor()?).map_err(|_| NumericError::Overflow)?,
        i32::try_from(rational.ceil()?).map_err(|_| NumericError::Overflow)?,
    )
    .map(Some)
}

/// Analytic box-filter area for a quadratic graph that remains within the
/// unit pixel vertically. Crossing/clipped graphs fail closed to full
/// coverage; callers may subdivide at the returned monotone split and retry.
pub fn quadratic_graph_area(
    graph: QuadraticGraph,
    strip_area_raw: u32,
    coverage_domain: FixedDomain,
    foreground_below: bool,
    owner: BoundaryOwner,
) -> Result<Iv32, NumericError> {
    let scale = coverage_scale(coverage_domain)?;
    let range = exact_quadratic_range(
        Quadratic2 {
            c: graph.c,
            bu: graph.b,
            bv: 0,
            buu: graph.a,
            buv: 0,
            bvv: 0,
        },
        coverage_domain,
    )?;
    if range.lo < 0 || i64::from(range.hi) > scale {
        return clipped_quadratic_graph_area(graph, strip_area_raw, scale, foreground_below, owner);
    }
    let integral = Rational::integer(graph.c)
        .checked_add(Rational::new(i128::from(graph.b), 2)?)?
        .checked_add(Rational::new(i128::from(graph.a), 3)?)?;
    let (mut lo, mut hi) = if foreground_below {
        (integral.floor()?, integral.ceil()?)
    } else {
        (
            i128::from(scale) - integral.ceil()?,
            i128::from(scale) - integral.floor()?,
        )
    };
    let strip = i128::from(strip_area_raw);
    lo = (lo - strip).max(0);
    hi = (hi + strip).min(i128::from(scale));
    Iv32::new(
        i32::try_from(lo).map_err(|_| NumericError::Overflow)?,
        i32::try_from(hi).map_err(|_| NumericError::Overflow)?,
    )
}

/// Clip a crossing graph analytically. Each fixed dyadic x piece uses the
/// exact endpoint values and includes the sole derivative zero when it lies
/// in that piece. The resulting vertical interval is clipped to the unit
/// pixel before integration. Boundary ownership assigns a stationary point
/// on a shared piece edge to exactly one neighbor; endpoint inclusion keeps
/// the numeric area enclosure unchanged.
fn clipped_quadratic_graph_area(
    graph: QuadraticGraph,
    strip_area_raw: u32,
    scale: i64,
    foreground_below: bool,
    owner: BoundaryOwner,
) -> Result<Iv32, NumericError> {
    const PIECES: i128 = 256;
    // The scalar Wrela counterpart has i64 widened arithmetic. Larger local
    // coefficients retain a conservative full interval instead of allowing
    // the two implementations to disagree or wrap.
    if [graph.c, graph.b, graph.a]
        .into_iter()
        .any(|value| !(-1_000_000..=1_000_000).contains(&value))
    {
        return Iv32::new(0, i32::try_from(scale).map_err(|_| NumericError::Overflow)?);
    }
    let denominator = PIECES.checked_mul(PIECES).ok_or(NumericError::Overflow)?;
    let stationary_numerator = i128::from(graph.b)
        .checked_neg()
        .ok_or(NumericError::Overflow)?;
    let stationary_denominator = 2_i128
        .checked_mul(i128::from(graph.a))
        .ok_or(NumericError::Overflow)?;
    let (stationary_numerator, stationary_denominator) = if stationary_denominator < 0 {
        (
            stationary_numerator
                .checked_neg()
                .ok_or(NumericError::Overflow)?,
            stationary_denominator
                .checked_neg()
                .ok_or(NumericError::Overflow)?,
        )
    } else {
        (stationary_numerator, stationary_denominator)
    };
    let stationary_value = if graph.a == 0 {
        None
    } else {
        Some(Rational::new(
            4_i128
                .checked_mul(i128::from(graph.a))
                .and_then(|value| value.checked_mul(i128::from(graph.c)))
                .and_then(|value| {
                    i128::from(graph.b)
                        .checked_mul(i128::from(graph.b))
                        .and_then(|square| value.checked_sub(square))
                })
                .ok_or(NumericError::Overflow)?,
            4_i128
                .checked_mul(i128::from(graph.a))
                .ok_or(NumericError::Overflow)?,
        )?)
    };
    let mut lower_sum = 0_i128;
    let mut upper_sum = 0_i128;
    for piece in 0..PIECES {
        let endpoint_numerator = |x: i128| -> Result<i128, NumericError> {
            i128::from(graph.c)
                .checked_mul(denominator)
                .and_then(|value| {
                    i128::from(graph.b)
                        .checked_mul(x)
                        .and_then(|term| term.checked_mul(PIECES))
                        .and_then(|term| value.checked_add(term))
                })
                .and_then(|value| {
                    i128::from(graph.a)
                        .checked_mul(x)
                        .and_then(|term| term.checked_mul(x))
                        .and_then(|term| value.checked_add(term))
                })
                .ok_or(NumericError::Overflow)
        };
        let left = Rational::new(endpoint_numerator(piece)?, denominator)?;
        let right = Rational::new(endpoint_numerator(piece + 1)?, denominator)?;
        let mut lo = left.floor()?.min(right.floor()?);
        let mut hi = left.ceil()?.max(right.ceil()?);
        if let Some(stationary) = stationary_value {
            let scaled_stationary = stationary_numerator
                .checked_mul(PIECES)
                .ok_or(NumericError::Overflow)?;
            let lower = piece
                .checked_mul(stationary_denominator)
                .ok_or(NumericError::Overflow)?;
            let upper = (piece + 1)
                .checked_mul(stationary_denominator)
                .ok_or(NumericError::Overflow)?;
            let assigned = match owner {
                BoundaryOwner::LowerOrLeft => {
                    lower <= scaled_stationary && scaled_stationary < upper
                }
                BoundaryOwner::UpperOrRight => {
                    lower < scaled_stationary && scaled_stationary <= upper
                }
            };
            if assigned {
                lo = lo.min(stationary.floor()?);
                hi = hi.max(stationary.ceil()?);
            }
        }
        lower_sum = lower_sum
            .checked_add(lo.clamp(0, i128::from(scale)))
            .ok_or(NumericError::Overflow)?;
        upper_sum = upper_sum
            .checked_add(hi.clamp(0, i128::from(scale)))
            .ok_or(NumericError::Overflow)?;
    }
    let mut lo = lower_sum.div_euclid(PIECES);
    let mut hi = upper_sum
        .checked_neg()
        .ok_or(NumericError::Overflow)?
        .div_euclid(PIECES)
        .checked_neg()
        .ok_or(NumericError::Overflow)?;
    if !foreground_below {
        (lo, hi) = (i128::from(scale) - hi, i128::from(scale) - lo);
    }
    let strip = i128::from(strip_area_raw);
    lo = (lo - strip).max(0);
    hi = (hi + strip).min(i128::from(scale));
    Iv32::new(
        i32::try_from(lo).map_err(|_| NumericError::Overflow)?,
        i32::try_from(hi).map_err(|_| NumericError::Overflow)?,
    )
}

pub fn half_plane_area(
    edge: HalfPlane,
    coverage_domain: FixedDomain,
    owner: BoundaryOwner,
) -> Result<Iv32, NumericError> {
    coverage_domain.validate()?;
    if coverage_domain.exponent > 0 {
        return Err(NumericError::InvalidDomain);
    }
    rational_coverage(half_plane_rational_area(edge, owner)?, coverage_domain)
}

/// Exact round-half-up byte coverage for a linear event over the unit pixel.
/// This retains the clipped polygon's rational area until the final byte
/// conversion, rather than quantizing through the ordinary fixed coverage
/// domain first.
pub fn half_plane_byte(edge: HalfPlane, owner: BoundaryOwner) -> Result<u8, NumericError> {
    let area = half_plane_rational_area(edge, owner)?.checked_mul_integer(255)?;
    let doubled_numerator = area
        .numerator
        .checked_mul(2)
        .and_then(|value| value.checked_add(area.denominator))
        .ok_or(NumericError::Overflow)?;
    let doubled_denominator = area
        .denominator
        .checked_mul(2)
        .ok_or(NumericError::Overflow)?;
    u8::try_from(doubled_numerator.div_euclid(doubled_denominator))
        .map_err(|_| NumericError::Overflow)
}

fn half_plane_rational_area(
    edge: HalfPlane,
    owner: BoundaryOwner,
) -> Result<Rational, NumericError> {
    if [edge.a, edge.b, edge.c]
        .into_iter()
        .any(|value| !(-1_000_000..=1_000_000).contains(&value))
    {
        return Err(NumericError::Overflow);
    }
    let mut vertices = [Point::zero(); 8];
    vertices[0] = Point::integer(0, 0);
    vertices[1] = Point::integer(1, 0);
    vertices[2] = Point::integer(1, 1);
    vertices[3] = Point::integer(0, 1);
    let mut count = 4_usize;
    let mut output = [Point::zero(); 8];
    let mut output_count = 0_usize;
    for index in 0..count {
        let start = vertices[index];
        let end = vertices[(index + 1) % count];
        let start_value = evaluate(edge, start)?;
        let end_value = evaluate(edge, end)?;
        let start_inside = inside(start_value, owner);
        let end_inside = inside(end_value, owner);
        if start_inside != end_inside {
            let denominator = start_value
                .checked_sub(end_value)
                .ok_or(NumericError::Overflow)?;
            let t = Rational::new(start_value, denominator)?;
            output[output_count] = start.interpolate(end, t)?;
            output_count += 1;
        }
        if end_inside {
            output[output_count] = end;
            output_count += 1;
        }
    }
    vertices = output;
    count = output_count;
    if count < 3 {
        return Ok(Rational::zero());
    }
    let mut double_area = Rational::zero();
    for index in 0..count {
        let a = vertices[index];
        let b = vertices[(index + 1) % count];
        double_area =
            double_area.checked_add(a.x.checked_mul(b.y)?.checked_sub(a.y.checked_mul(b.x)?)?)?;
    }
    double_area.abs()?.checked_div_integer(2)
}

/// Conservative area around a quadratic segment. The caller supplies a chord
/// half-plane plus a proved strip-area bound for all monotone pieces.
pub fn quadratic_strip_area(
    chord: HalfPlane,
    strip_area_raw: u32,
    coverage_domain: FixedDomain,
    owner: BoundaryOwner,
) -> Result<Iv32, NumericError> {
    let line = half_plane_area(chord, coverage_domain, owner)?;
    let strip = i64::from(strip_area_raw);
    let full_scale = coverage_scale(coverage_domain)?;
    let lo = (i64::from(line.lo) - strip).max(0);
    let hi = (i64::from(line.hi) + strip).min(full_scale);
    Iv32::new(
        i32::try_from(lo).map_err(|_| NumericError::Overflow)?,
        i32::try_from(hi).map_err(|_| NumericError::Overflow)?,
    )
}

pub fn color_error_bound(
    coverage: Iv32,
    contrast: Iv32,
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    let width = i32::try_from(coverage.width_raw()?).map_err(|_| NumericError::Overflow)?;
    Iv32::new(-width, width)?
        .multiply(contrast.abs(domain)?, domain)?
        .abs(domain)
}

fn inside(value: i128, owner: BoundaryOwner) -> bool {
    match owner {
        BoundaryOwner::LowerOrLeft => value >= 0,
        BoundaryOwner::UpperOrRight => value > 0,
    }
}

fn evaluate(edge: HalfPlane, point: Point) -> Result<i128, NumericError> {
    let value = point
        .x
        .checked_mul_integer(i128::from(edge.a))?
        .checked_add(point.y.checked_mul_integer(i128::from(edge.b))?)?
        .checked_add(Rational::integer(edge.c))?;
    value.as_integer()
}

fn rational_coverage(area: Rational, coverage_domain: FixedDomain) -> Result<Iv32, NumericError> {
    let scale = i128::from(coverage_scale(coverage_domain)?);
    let scaled = area.checked_mul_integer(scale)?;
    let lo = scaled.floor()?;
    let hi = scaled.ceil()?;
    Iv32::new(
        i32::try_from(lo).map_err(|_| NumericError::Overflow)?,
        i32::try_from(hi).map_err(|_| NumericError::Overflow)?,
    )
}

fn coverage_scale(domain: FixedDomain) -> Result<i64, NumericError> {
    let bits = u32::try_from(-domain.exponent).map_err(|_| NumericError::InvalidDomain)?;
    1_i64.checked_shl(bits).ok_or(NumericError::Overflow)
}

#[derive(Clone, Copy, Debug)]
struct Point {
    x: Rational,
    y: Rational,
}

impl Point {
    const fn zero() -> Self {
        Self {
            x: Rational::zero(),
            y: Rational::zero(),
        }
    }

    fn integer(x: i64, y: i64) -> Self {
        Self {
            x: Rational::integer(x),
            y: Rational::integer(y),
        }
    }

    fn interpolate(self, other: Self, t: Rational) -> Result<Self, NumericError> {
        Ok(Self {
            x: self
                .x
                .checked_add(other.x.checked_sub(self.x)?.checked_mul(t)?)?,
            y: self
                .y
                .checked_add(other.y.checked_sub(self.y)?.checked_mul(t)?)?,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Rational {
    numerator: i128,
    denominator: i128,
}

impl Rational {
    const fn zero() -> Self {
        Self {
            numerator: 0,
            denominator: 1,
        }
    }

    fn integer(value: impl Into<i128>) -> Self {
        Self {
            numerator: value.into(),
            denominator: 1,
        }
    }

    fn new(numerator: i128, denominator: i128) -> Result<Self, NumericError> {
        if denominator == 0 {
            return Err(NumericError::DivisionByZero);
        }
        if denominator < 0 {
            Ok(Self {
                numerator: numerator.checked_neg().ok_or(NumericError::Overflow)?,
                denominator: denominator.checked_neg().ok_or(NumericError::Overflow)?,
            })
        } else {
            Ok(Self {
                numerator,
                denominator,
            })
        }
    }

    fn checked_add(self, other: Self) -> Result<Self, NumericError> {
        Self::new(
            self.numerator
                .checked_mul(other.denominator)
                .and_then(|left| {
                    other
                        .numerator
                        .checked_mul(self.denominator)
                        .and_then(|right| left.checked_add(right))
                })
                .ok_or(NumericError::Overflow)?,
            self.denominator
                .checked_mul(other.denominator)
                .ok_or(NumericError::Overflow)?,
        )
    }

    fn checked_sub(self, other: Self) -> Result<Self, NumericError> {
        Self::new(
            self.numerator
                .checked_mul(other.denominator)
                .and_then(|left| {
                    other
                        .numerator
                        .checked_mul(self.denominator)
                        .and_then(|right| left.checked_sub(right))
                })
                .ok_or(NumericError::Overflow)?,
            self.denominator
                .checked_mul(other.denominator)
                .ok_or(NumericError::Overflow)?,
        )
    }

    fn checked_mul(self, other: Self) -> Result<Self, NumericError> {
        Self::new(
            self.numerator
                .checked_mul(other.numerator)
                .ok_or(NumericError::Overflow)?,
            self.denominator
                .checked_mul(other.denominator)
                .ok_or(NumericError::Overflow)?,
        )
    }

    fn checked_mul_integer(self, value: i128) -> Result<Self, NumericError> {
        Self::new(
            self.numerator
                .checked_mul(value)
                .ok_or(NumericError::Overflow)?,
            self.denominator,
        )
    }

    fn checked_div_integer(self, value: i128) -> Result<Self, NumericError> {
        Self::new(
            self.numerator,
            self.denominator
                .checked_mul(value)
                .ok_or(NumericError::Overflow)?,
        )
    }

    fn in_unit_scaled(self, scale: i128) -> Result<bool, NumericError> {
        let upper = scale
            .checked_mul(self.denominator)
            .ok_or(NumericError::Overflow)?;
        Ok(self.numerator > 0 && self.numerator < upper)
    }

    fn abs(self) -> Result<Self, NumericError> {
        Self::new(
            self.numerator.checked_abs().ok_or(NumericError::Overflow)?,
            self.denominator,
        )
    }

    fn as_integer(self) -> Result<i128, NumericError> {
        if self.denominator != 1 {
            return Err(NumericError::UnsupportedShape);
        }
        Ok(self.numerator)
    }

    fn floor(self) -> Result<i128, NumericError> {
        Ok(self.numerator.div_euclid(self.denominator))
    }

    fn ceil(self) -> Result<i128, NumericError> {
        Ok(-self
            .numerator
            .checked_neg()
            .ok_or(NumericError::Overflow)?
            .div_euclid(self.denominator))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COVERAGE: FixedDomain = FixedDomain::full(-8);

    #[test]
    fn axis_diagonal_and_corner_coverage_are_exact() {
        assert_eq!(
            half_plane_area(
                HalfPlane { a: 1, b: 0, c: 0 },
                COVERAGE,
                BoundaryOwner::LowerOrLeft
            ),
            Ok(Iv32::point(256))
        );
        assert_eq!(
            half_plane_area(
                HalfPlane { a: 1, b: 1, c: -1 },
                COVERAGE,
                BoundaryOwner::LowerOrLeft
            ),
            Ok(Iv32::point(128))
        );
        assert_eq!(
            half_plane_area(
                HalfPlane { a: 1, b: 1, c: -2 },
                COVERAGE,
                BoundaryOwner::LowerOrLeft
            ),
            Ok(Iv32::point(0))
        );
    }

    #[test]
    fn diagonal_byte_rounding_keeps_the_exact_rational_until_the_end() {
        assert_eq!(
            half_plane_byte(
                HalfPlane {
                    a: -32,
                    b: 32,
                    c: 11,
                },
                BoundaryOwner::UpperOrRight,
            ),
            Ok(200),
        );
        assert_eq!(
            half_plane_byte(
                HalfPlane {
                    a: 32,
                    b: -32,
                    c: 21,
                },
                BoundaryOwner::LowerOrLeft,
            ),
            Ok(240),
        );
    }

    #[test]
    fn quadratic_strip_contains_chord_area() {
        let line = HalfPlane { a: 1, b: 1, c: -1 };
        assert_eq!(
            quadratic_strip_area(line, 3, COVERAGE, BoundaryOwner::LowerOrLeft),
            Ok(Iv32 { lo: 125, hi: 131 })
        );
    }

    #[test]
    fn high_curvature_quadratic_uses_monotone_split_and_exact_integral() {
        let graph = QuadraticGraph {
            c: 192,
            b: -256,
            a: 256,
        };
        assert_eq!(
            quadratic_monotone_split(graph, COVERAGE),
            Ok(Some(Iv32::point(128)))
        );
        // Integral is 192 - 128 + 256/3 = 149 1/3 raw units.
        assert_eq!(
            quadratic_graph_area(graph, 2, COVERAGE, true, BoundaryOwner::LowerOrLeft),
            Ok(Iv32::new(147, 152).unwrap())
        );
    }

    #[test]
    fn half_open_boundary_ownership_does_not_change_area() {
        let edge = HalfPlane { a: 1, b: 0, c: 0 };
        let left = half_plane_area(edge, COVERAGE, BoundaryOwner::LowerOrLeft).unwrap();
        let right = half_plane_area(edge, COVERAGE, BoundaryOwner::UpperOrRight).unwrap();
        assert_eq!(left, right);
    }

    #[test]
    fn line_intervals_contain_dense_deterministic_oracle() {
        for edge in [
            HalfPlane { a: -3, b: 5, c: -1 },
            HalfPlane { a: 7, b: -2, c: -3 },
            HalfPlane { a: 1, b: 0, c: -1 },
            HalfPlane { a: 9, b: 11, c: -7 },
        ] {
            let interval = half_plane_area(edge, COVERAGE, BoundaryOwner::LowerOrLeft).unwrap();
            // Independent one-dimensional integration: split at each x where
            // the line meets y=0 or y=1, then integrate the linear vertical
            // foreground fraction exactly by the trapezoid rule.
            let mut breaks = vec![0.0_f64, 1.0];
            if edge.a != 0 {
                for numerator in [-edge.c, -edge.b - edge.c] {
                    let x = numerator as f64 / edge.a as f64;
                    if 0.0 < x && x < 1.0 {
                        breaks.push(x);
                    }
                }
            }
            breaks.sort_by(f64::total_cmp);
            breaks.dedup();
            let vertical = |x: f64| {
                let at_zero = edge.a as f64 * x + edge.c as f64;
                if edge.b > 0 {
                    (1.0 + at_zero / edge.b as f64).clamp(0.0, 1.0)
                } else if edge.b < 0 {
                    (-at_zero / edge.b as f64).clamp(0.0, 1.0)
                } else if at_zero >= 0.0 {
                    1.0
                } else {
                    0.0
                }
            };
            let mut area = 0.0;
            for pair in breaks.windows(2) {
                let left = pair[0];
                let right = pair[1];
                if edge.b == 0 {
                    area += (right - left) * vertical((left + right) * 0.5);
                } else {
                    area += (right - left) * (vertical(left) + vertical(right)) * 0.5;
                }
            }
            let oracle = area * 256.0;
            assert!(
                f64::from(interval.lo) <= oracle + 1.0e-9
                    && oracle - 1.0e-9 <= f64::from(interval.hi),
                "{edge:?}: {interval:?} does not contain {oracle}"
            );
        }
    }

    #[test]
    fn quadratic_interval_contains_independent_dense_integration() {
        for (graph, strip) in [
            (
                QuadraticGraph {
                    c: 32,
                    b: 96,
                    a: 64,
                },
                1,
            ),
            (
                QuadraticGraph {
                    c: 192,
                    b: -256,
                    a: 256,
                },
                2,
            ),
            (
                QuadraticGraph {
                    c: 220,
                    b: -160,
                    a: 80,
                },
                1,
            ),
            (
                QuadraticGraph {
                    c: -64,
                    b: 512,
                    a: -256,
                },
                1,
            ),
        ] {
            let interval =
                quadratic_graph_area(graph, strip, COVERAGE, true, BoundaryOwner::LowerOrLeft)
                    .unwrap();
            let samples = 65_536_i64;
            let mut sum = 0_f64;
            for index in 0..samples {
                let x = (index as f64 + 0.5) / samples as f64;
                sum += (f64::from(graph.c) + f64::from(graph.b) * x + f64::from(graph.a) * x * x)
                    .clamp(0.0, 256.0);
            }
            let oracle = sum / samples as f64;
            assert!(
                f64::from(interval.lo) <= oracle && oracle <= f64::from(interval.hi),
                "{graph:?}: {interval:?} does not contain {oracle}"
            );
            if graph.c < 0 {
                assert!(
                    interval.width_raw().unwrap() < 32,
                    "clipped thin curve must not collapse to full coverage: {interval:?}"
                );
            }
        }
    }

    #[test]
    fn clipped_quadratic_carries_half_open_piece_ownership() {
        let graph = QuadraticGraph {
            c: -64,
            b: 512,
            a: -256,
        };
        let lower =
            quadratic_graph_area(graph, 0, COVERAGE, true, BoundaryOwner::LowerOrLeft).unwrap();
        let upper =
            quadratic_graph_area(graph, 0, COVERAGE, true, BoundaryOwner::UpperOrRight).unwrap();
        assert_eq!(lower, upper);
        assert_ne!(lower, Iv32::new(0, 256).unwrap());
    }

    #[test]
    fn half_open_boundary_has_exactly_one_discrete_owner() {
        for signed_value in [-1_i128, 0, 1] {
            let lower_or_left = inside(signed_value, BoundaryOwner::LowerOrLeft);
            let upper_or_right_complement = inside(-signed_value, BoundaryOwner::UpperOrRight);
            assert_ne!(
                lower_or_left, upper_or_right_complement,
                "the exact boundary must be assigned once, not dropped or doubled"
            );
        }
        assert!(inside(0, BoundaryOwner::LowerOrLeft));
        assert!(!inside(0, BoundaryOwner::UpperOrRight));
    }

    #[test]
    fn complementary_shared_boundary_has_full_total_area() {
        let edge = HalfPlane { a: 3, b: -2, c: -1 };
        let complement = HalfPlane {
            a: -edge.a,
            b: -edge.b,
            c: -edge.c,
        };
        let a = half_plane_area(edge, COVERAGE, BoundaryOwner::LowerOrLeft).unwrap();
        let b = half_plane_area(complement, COVERAGE, BoundaryOwner::UpperOrRight).unwrap();
        assert!(a.lo + b.lo <= 256);
        assert!(a.hi + b.hi >= 256);
    }
}
