//! Premultiplied transfer summaries and conservative suffix cutoff.

use super::iv32::{FixedDomain, Iv32, NumericError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub rgb: [Iv32; 3],
    pub transmittance: Iv32,
}

impl Transfer {
    pub fn identity(one: Iv32) -> Self {
        Self {
            rgb: [Iv32::point(0); 3],
            transmittance: one,
        }
    }
}

pub fn compose(
    front: Transfer,
    back: Transfer,
    domain: FixedDomain,
    rounding_radius: i32,
) -> Result<Transfer, NumericError> {
    if rounding_radius < 0 {
        return Err(NumericError::InvalidDomain);
    }
    let rounding = Iv32::new(-rounding_radius, rounding_radius)?;
    let mut rgb = [Iv32::point(0); 3];
    for (channel, output) in rgb.iter_mut().enumerate() {
        *output = front.rgb[channel]
            .add(
                front.transmittance.multiply(back.rgb[channel], domain)?,
                domain,
            )?
            .add(rounding, domain)?;
    }
    Ok(Transfer {
        rgb,
        transmittance: front
            .transmittance
            .multiply(back.transmittance, domain)?
            .add(rounding, domain)?,
    })
}

pub fn balanced_summary(
    layers: &[Transfer],
    domain: FixedDomain,
    one: Iv32,
    rounding_radius: i32,
) -> Result<Transfer, NumericError> {
    fn summarize(
        layers: &[Transfer],
        domain: FixedDomain,
        one: Iv32,
        rounding_radius: i32,
    ) -> Result<Transfer, NumericError> {
        match layers {
            [] => Ok(Transfer::identity(one)),
            [layer] => Ok(*layer),
            _ => {
                let midpoint = layers.len() / 2;
                compose(
                    summarize(&layers[..midpoint], domain, one, rounding_radius)?,
                    summarize(&layers[midpoint..], domain, one, rounding_radius)?,
                    domain,
                    rounding_radius,
                )
            }
        }
    }
    summarize(layers, domain, one, rounding_radius)
}

pub fn replace_and_summarize(
    layers: &mut [Transfer],
    index: usize,
    replacement: Transfer,
    domain: FixedDomain,
    one: Iv32,
    rounding_radius: i32,
) -> Result<Transfer, NumericError> {
    let target = layers
        .get_mut(index)
        .ok_or(NumericError::CapacityExceeded)?;
    *target = replacement;
    balanced_summary(layers, domain, one, rounding_radius)
}

pub fn tail_can_stop(
    prefix_transmittance: Iv32,
    maximum_remaining_radiance: Iv32,
    assigned_error_raw: i32,
    domain: FixedDomain,
) -> Result<bool, NumericError> {
    if assigned_error_raw < 0 || prefix_transmittance.lo < 0 || maximum_remaining_radiance.lo < 0 {
        return Err(NumericError::InvalidDomain);
    }
    let possible_error = prefix_transmittance.multiply(maximum_remaining_radiance, domain)?;
    Ok(possible_error.hi <= assigned_error_raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bright_low_alpha_tail_is_not_cut_early() {
        let domain = FixedDomain::full(-8);
        assert_eq!(
            tail_can_stop(Iv32::point(8), Iv32::point(4096), 1, domain),
            Ok(false)
        );
    }

    #[test]
    fn local_replacement_rebuilds_summary() {
        let domain = FixedDomain::full(-8);
        let one = Iv32::point(256);
        let clear = Transfer::identity(one);
        let mut layers = [clear, clear];
        assert_eq!(
            replace_and_summarize(&mut layers, 1, clear, domain, one, 0),
            Ok(clear)
        );
    }
}
