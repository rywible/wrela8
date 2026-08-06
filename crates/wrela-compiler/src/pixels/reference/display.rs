//! Output-referred interval transforms and exact byte singleton checks.

use super::iv32::{FixedDomain, Iv32, NumericError};

pub use wrela_machine::pixels::{
    FILMIC_TONE_LUT_V1 as FILMIC_V1_TONE_LUT, LINEAR_TONE_LUT_V1 as LINEAR_TONE_LUT,
    SRGB_TRANSFER_LUT_V1 as SRGB_TRANSFER_LUT,
};

pub fn sealed_tone_lut(name: &str) -> Option<&'static [u16]> {
    match name {
        "Linear" => Some(&LINEAR_TONE_LUT),
        "FilmicV1" => Some(&FILMIC_V1_TONE_LUT),
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
        assert_eq!(validate_monotone_lut(&[0, 1, 1, 4]), Ok(()));
        validate_monotone_lut(&LINEAR_TONE_LUT).unwrap();
        validate_monotone_lut(&FILMIC_V1_TONE_LUT).unwrap();
        validate_monotone_lut(&SRGB_TRANSFER_LUT).unwrap();
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
}
