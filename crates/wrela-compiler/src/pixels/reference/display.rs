//! Output-referred interval transforms and exact byte singleton checks.

use super::interval::{F64Interval, next_down_f32, next_up_f32};
use super::iv32::{FixedDomain, Iv32, NumericError};

pub use wrela_machine::pixels::LINEAR_TONE_LUT_V1 as LINEAR_TONE_LUT;

// A repository-owned golden fixture selects this through a compiler-reserved
// enum variant. Keeping the malformed bytes here exercises the same validator
// as canonical tables without making custom source tone tables part of v1.
const NON_MONOTONE_FIXTURE_TONE_LUT: [u16; 2] = [2, 1];

fn canonical_table(kind: super::super::tables::TableKind) -> &'static [u16] {
    use std::sync::OnceLock;
    static FILMIC: OnceLock<Vec<u16>> = OnceLock::new();
    static SRGB: OnceLock<Vec<u16>> = OnceLock::new();
    let slot = match kind {
        super::super::tables::TableKind::FilmicV1 => &FILMIC,
        super::super::tables::TableKind::SrgbV1 => &SRGB,
    };
    slot.get_or_init(|| {
        super::super::tables::values(kind)
            .expect("canonical Pixels transfer tables are verified before renderer compilation")
    })
}

pub fn filmic_v1_tone_lut() -> &'static [u16] {
    canonical_table(super::super::tables::TableKind::FilmicV1)
}

pub fn srgb_transfer_lut() -> &'static [u16] {
    canonical_table(super::super::tables::TableKind::SrgbV1)
}

pub fn sealed_tone_lut(name: &str) -> Option<&'static [u16]> {
    match name {
        "Linear" => Some(&LINEAR_TONE_LUT),
        "FilmicV1" => Some(filmic_v1_tone_lut()),
        "__wrela_NonMonotoneFixture" => Some(&NON_MONOTONE_FIXTURE_TONE_LUT),
        _ => None,
    }
}

pub fn exposure(
    color: [Iv32; 3],
    multiplier: Iv32,
    domain: FixedDomain,
) -> Result<[Iv32; 3], NumericError> {
    Ok([
        color[0].multiply(multiplier, domain)?,
        color[1].multiply(multiplier, domain)?,
        color[2].multiply(multiplier, domain)?,
    ])
}

pub fn color_matrix(
    color: [Iv32; 3],
    matrix: [[Iv32; 3]; 3],
    domain: FixedDomain,
) -> Result<[Iv32; 3], NumericError> {
    let mut output = [Iv32::point(0); 3];
    for row in 0..3 {
        for (column, value) in color.iter().enumerate() {
            output[row] = output[row].add(matrix[row][column].multiply(*value, domain)?, domain)?;
        }
    }
    Ok(output)
}

pub fn validate_monotone_lut(table: &[u16]) -> Result<(), NumericError> {
    if table.len() < 2 || table.windows(2).any(|pair| pair[0] > pair[1]) {
        Err(NumericError::UnsupportedShape)
    } else {
        Ok(())
    }
}

/// LUT input has `index_fraction_bits` fractional bits. Interpolation is
/// outward in the LUT's integer output domain.
pub fn monotone_lut(
    input: Iv32,
    table: &[u16],
    index_fraction_bits: u8,
) -> Result<Iv32, NumericError> {
    validate_monotone_lut(table)?;
    let last_index = table.len() - 1;
    let maximum_raw = i64::try_from(last_index)
        .map_err(|_| NumericError::Overflow)?
        .checked_shl(u32::from(index_fraction_bits))
        .ok_or(NumericError::Overflow)?;
    if input.lo < 0 || i64::from(input.hi) > maximum_raw {
        return Err(NumericError::DomainMismatch);
    }
    let low = interpolate_endpoint(input.lo, table, index_fraction_bits, false)?;
    let high = interpolate_endpoint(input.hi, table, index_fraction_bits, true)?;
    Iv32::new(low, high)
}

pub fn quantize_u8_ties_even(value: i32, fraction_bits: u8) -> Result<u8, NumericError> {
    if value < 0 || fraction_bits >= 31 {
        return Err(NumericError::DomainMismatch);
    }
    if fraction_bits == 0 {
        return u8::try_from(value).map_err(|_| NumericError::Overflow);
    }
    let scale = 1_i64 << fraction_bits;
    let quotient = i64::from(value) / scale;
    let remainder = i64::from(value) % scale;
    let half = scale / 2;
    let rounded = if remainder > half || (remainder == half && quotient & 1 != 0) {
        quotient + 1
    } else {
        quotient
    };
    u8::try_from(rounded).map_err(|_| NumericError::Overflow)
}

pub fn endpoint_singleton(interval: Iv32, fraction_bits: u8) -> Result<Option<u8>, NumericError> {
    let lo = quantize_u8_ties_even(interval.lo, fraction_bits)?;
    let hi = quantize_u8_ties_even(interval.hi, fraction_bits)?;
    Ok((lo == hi).then_some(lo))
}

pub fn rgb_singleton(color: [Iv32; 3], fraction_bits: u8) -> Result<Option<[u8; 3]>, NumericError> {
    let mut output = [0_u8; 3];
    for (channel, interval) in color.into_iter().enumerate() {
        let Some(code) = endpoint_singleton(interval, fraction_bits)? else {
            return Ok(None);
        };
        output[channel] = code;
    }
    Ok(Some(output))
}

fn round_ratio(numerator: u64, denominator: u64) -> Result<u64, NumericError> {
    if denominator == 0 {
        return Err(NumericError::DomainMismatch);
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let twice = remainder.checked_mul(2).ok_or(NumericError::Overflow)?;
    Ok(
        if twice > denominator || (twice == denominator && quotient & 1 != 0) {
            quotient + 1
        } else {
            quotient
        },
    )
}

fn interpolate_u16_fixed(
    table: &[u16],
    coordinate: u64,
    fraction_bits: u8,
) -> Result<u16, NumericError> {
    validate_monotone_lut(table)?;
    let last = u64::try_from(table.len() - 1).map_err(|_| NumericError::Overflow)?;
    let index = coordinate >> fraction_bits;
    if index > last {
        return Err(NumericError::DomainMismatch);
    }
    let index = usize::try_from(index).map_err(|_| NumericError::Overflow)?;
    if index == table.len() - 1 {
        return Ok(table[index]);
    }
    let scale = 1_u64 << fraction_bits;
    let fraction = coordinate & (scale - 1);
    let low = u64::from(table[index]);
    let delta = u64::from(table[index + 1] - table[index]);
    let numerator = low
        .checked_mul(scale)
        .and_then(|value| value.checked_add(delta.checked_mul(fraction)?))
        .ok_or(NumericError::Overflow)?;
    u16::try_from(round_ratio(numerator, scale)?).map_err(|_| NumericError::Overflow)
}

/// Exact f32-bit coordinate used by the generated guest. The result has eight
/// fractional LUT-index bits: one log2 unit is exactly 128 table entries.
fn filmic_coordinate_q8(value: f32) -> Result<Option<u64>, NumericError> {
    let bits = value.to_bits();
    if bits & 0x7f80_0000 == 0x7f80_0000 {
        return Err(NumericError::DomainMismatch);
    }
    if bits & 0x8000_0000 != 0 || bits & 0x7fff_ffff == 0 {
        return Ok(None);
    }
    let exponent_bits = (bits >> 23) & 0xff;
    let mut mantissa = u64::from(bits & 0x007f_ffff);
    let mut exponent = i64::from(exponent_bits) - 127;
    if exponent_bits == 0 {
        exponent = -126;
        while mantissa < 0x0080_0000 {
            mantissa <<= 1;
            exponent -= 1;
        }
    } else {
        mantissa |= 0x0080_0000;
    }
    let mut fraction = 0_u64;
    for step in 0..15_u32 {
        let squared = mantissa
            .checked_mul(mantissa)
            .ok_or(NumericError::Overflow)?
            >> 23;
        if squared >= 0x0100_0000 {
            mantissa = squared >> 1;
            fraction |= 1_u64 << (14 - step);
        } else {
            mantissa = squared;
        }
    }
    let log_q15 = exponent * 32_768 + i64::try_from(fraction).unwrap();
    Ok(Some(if log_q15 >= 524_288 {
        1_048_576
    } else if log_q15 > -524_288 {
        u64::try_from(log_q15 + 524_288).unwrap()
    } else {
        0
    }))
}

fn encode_linear_f32(input: f32, filmic: bool) -> Result<u8, NumericError> {
    if !input.is_finite() {
        return Err(NumericError::DomainMismatch);
    }
    let tone = if filmic {
        filmic_coordinate_q8(input)?
            .map(|coordinate| interpolate_u16_fixed(filmic_v1_tone_lut(), coordinate, 8))
            .transpose()?
            .unwrap_or(0)
    } else {
        (input.clamp(0.0, 1.0) * 65_535.0 + 0.5) as u16
    };
    let srgb_coordinate_q16 = round_ratio(u64::from(tone) * 4096 * 65_536, 65_535)?;
    let srgb = interpolate_u16_fixed(srgb_transfer_lut(), srgb_coordinate_q16, 16)?;
    u8::try_from(round_ratio(u64::from(srgb) * 255, 65_535)?).map_err(|_| NumericError::Overflow)
}

pub fn encode_linear_candidate(linear: f64, filmic: bool) -> Result<u8, NumericError> {
    if !linear.is_finite() {
        return Err(NumericError::DomainMismatch);
    }
    let input = linear as f32;
    if !input.is_finite() {
        return Err(NumericError::DomainMismatch);
    }
    encode_linear_f32(input, filmic)
}

pub fn encode_linear_endpoint(linear: f64, filmic: bool, upper: bool) -> Result<u8, NumericError> {
    if !linear.is_finite() || linear < -(f32::MAX as f64) || linear > f32::MAX as f64 {
        return Err(NumericError::DomainMismatch);
    }
    let mut input = linear as f32;
    if upper && f64::from(input) < linear {
        input = next_up_f32(input);
    } else if !upper && f64::from(input) > linear {
        input = next_down_f32(input);
    }
    encode_linear_f32(input, filmic)
}

pub fn encoded_singleton(linear: F64Interval, filmic: bool) -> Result<Option<u8>, NumericError> {
    let lo = encode_linear_endpoint(linear.lo, filmic, false)?;
    let hi = encode_linear_endpoint(linear.hi, filmic, true)?;
    Ok((lo == hi).then_some(lo))
}

fn interpolate_endpoint(
    raw: i32,
    table: &[u16],
    fraction_bits: u8,
    upper: bool,
) -> Result<i32, NumericError> {
    let scale = 1_i64 << fraction_bits;
    let index = usize::try_from(i64::from(raw) / scale).map_err(|_| NumericError::Overflow)?;
    let fraction = i64::from(raw) % scale;
    if index + 1 >= table.len() {
        return Ok(i32::from(table[index]));
    }
    let base = i64::from(table[index]);
    let delta = i64::from(table[index + 1]) - base;
    let numerator = delta.checked_mul(fraction).ok_or(NumericError::Overflow)?;
    let adjustment = if upper {
        (numerator + scale - 1) / scale
    } else {
        numerator / scale
    };
    i32::try_from(base + adjustment).map_err(|_| NumericError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_validation_fails_closed() {
        assert_eq!(
            validate_monotone_lut(&[0, 2, 1]),
            Err(NumericError::UnsupportedShape)
        );
        assert_eq!(
            validate_monotone_lut(sealed_tone_lut("__wrela_NonMonotoneFixture").unwrap()),
            Err(NumericError::UnsupportedShape)
        );
        assert_eq!(validate_monotone_lut(&[0, 1, 1, 4]), Ok(()));
        validate_monotone_lut(&LINEAR_TONE_LUT).unwrap();
        validate_monotone_lut(filmic_v1_tone_lut()).unwrap();
        validate_monotone_lut(srgb_transfer_lut()).unwrap();
    }

    #[test]
    fn ties_are_even_and_crossing_boundaries_are_not_singletons() {
        assert_eq!(quantize_u8_ties_even(3, 0), Ok(3));
        assert_eq!(quantize_u8_ties_even(2, 2), Ok(0));
        assert_eq!(quantize_u8_ties_even(6, 2), Ok(2));
        assert_eq!(endpoint_singleton(Iv32::new(5, 6).unwrap(), 2), Ok(None));
        assert_eq!(endpoint_singleton(Iv32::new(6, 9).unwrap(), 2), Ok(Some(2)));
    }

    #[test]
    fn monotone_interpolation_is_outward() {
        assert_eq!(
            monotone_lut(Iv32::new(1, 3).unwrap(), &[0, 3], 2),
            Ok(Iv32 { lo: 0, hi: 3 })
        );
    }

    #[test]
    fn hdr_encoding_handles_zero_without_log_and_requires_endpoint_agreement() {
        assert_eq!(encode_linear_endpoint(0.0, true, false), Ok(0));
        assert_eq!(
            encoded_singleton(F64Interval::point(0.0).unwrap(), true),
            Ok(Some(0))
        );
        assert_eq!(
            encoded_singleton(F64Interval::new(0.0, 1.0).unwrap(), true),
            Ok(None)
        );
    }

    #[test]
    fn filmic_coordinate_matches_guest_bits_at_byte_transition_and_extremes() {
        let transition = f32::from_bits(0x39c6_0245);
        assert_eq!(
            encode_linear_endpoint(f64::from(transition), true, false),
            Ok(1)
        );
        assert_eq!(
            encode_linear_endpoint(f64::from(transition), true, true),
            Ok(1)
        );
        assert_eq!(
            encode_linear_endpoint(f64::from(f32::from_bits(1)), true, false),
            Ok(0)
        );
        assert_eq!(
            encode_linear_endpoint(f64::from(f32::MAX), true, false),
            Ok(255)
        );
    }

    #[test]
    fn f64_interval_endpoints_enclose_adjacent_source_floats() {
        let mut lower_bits = 0_u32;
        let mut upper_bits = 1.0_f32.to_bits();
        while lower_bits + 1 < upper_bits {
            let middle = lower_bits + (upper_bits - lower_bits) / 2;
            if encode_linear_candidate(f64::from(f32::from_bits(middle)), true).unwrap() == 0 {
                lower_bits = middle;
            } else {
                upper_bits = middle;
            }
        }
        let prior = f32::from_bits(lower_bits);
        let transition = f32::from_bits(upper_bits);
        assert_eq!(encode_linear_candidate(f64::from(prior), true), Ok(0));
        assert_eq!(encode_linear_candidate(f64::from(transition), true), Ok(1));
        let between = f64::from(prior) + (f64::from(transition) - f64::from(prior)) * 0.75;
        assert_eq!(encode_linear_candidate(between, true), Ok(1));
        assert_eq!(
            encoded_singleton(F64Interval::point(between).unwrap(), true),
            Ok(None),
        );
    }
}
