//! Deterministic slope-moment normal filtering.

use super::light::Vec3;

/// Universal unit-radiance response radius for the sealed v1 BRDF.
///
/// The GGX distribution is at most `1 / (pi * 1e-12)`, the specular
/// denominator is at least `1e-12`, and Fresnel, Smith visibility, and the
/// outgoing cosine are in `[0, 1]`. The diffuse response is below two. This
/// deliberately broad box prevents a moment proposal from certifying a byte;
/// non-flat detail proceeds to the deterministic terminal tap set.
pub const BRDF_UNIT_RADIANCE_RADIUS_V1: f64 = 4.0e23;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlopeMoments {
    pub sx: f64,
    pub sy: f64,
    pub sx2: f64,
    pub sx_sy: f64,
    pub sy2: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilteredNormal {
    pub mean: Vec3,
    pub variance: f64,
    pub roughness: f64,
    pub curvature_error: f64,
}

pub fn filter(moments: SlopeMoments, base_roughness: f64) -> Result<FilteredNormal, String> {
    if ![
        moments.sx,
        moments.sy,
        moments.sx2,
        moments.sx_sy,
        moments.sy2,
        base_roughness,
    ]
    .into_iter()
    .all(f64::is_finite)
        || !(0.02..=1.0).contains(&base_roughness)
    {
        return Err("P025: invalid normal-detail moment input".to_string());
    }
    let var_x = moments.sx2 - moments.sx * moments.sx;
    let var_y = moments.sy2 - moments.sy * moments.sy;
    let covariance = moments.sx_sy - moments.sx * moments.sy;
    let determinant = var_x * var_y - covariance * covariance;
    let tolerance = 64.0 * f64::EPSILON * (1.0 + moments.sx2.abs() + moments.sy2.abs());
    if var_x < -tolerance || var_y < -tolerance || determinant < -tolerance {
        return Err(
            "P025: slope moments do not define a positive-semidefinite covariance".to_string(),
        );
    }
    let variance = (var_x.max(0.0) + var_y.max(0.0)).max(0.0);
    let mean = Vec3 {
        x: -moments.sx,
        y: -moments.sy,
        z: 1.0,
    }
    .normalize()
    .map_err(|_| "P025: mean perturbed normal is degenerate".to_string())?;
    let roughness = (base_roughness * base_roughness + variance)
        .sqrt()
        .clamp(0.02, 1.0);
    Ok(FilteredNormal {
        mean,
        variance,
        roughness,
        curvature_error: if variance == 0.0 {
            0.0
        } else {
            BRDF_UNIT_RADIANCE_RADIUS_V1
        },
    })
}

/// Candidate and conservative channel bounds for the deterministic four-tap
/// terminal. Moment mip filtering produces a convex mixture of these base-tap
/// responses; its weights need not be uniform, so the equal-weight candidate
/// is accompanied by the complete componentwise convex hull.
pub fn terminal_tap_envelope(
    taps: [[f64; 3]; 4],
) -> Result<([f64; 3], [f64; 3], [f64; 3]), String> {
    if !taps.into_iter().flatten().all(f64::is_finite) {
        return Err("P025: non-finite normal terminal response".to_string());
    }
    let mut candidate = [0.0; 3];
    let mut lower = taps[0];
    let mut upper = taps[0];
    for tap in taps {
        for channel in 0..3 {
            candidate[channel] += tap[channel] * 0.25;
            lower[channel] = lower[channel].min(tap[channel]);
            upper[channel] = upper[channel].max(tap[channel]);
        }
    }
    Ok((candidate, lower, upper))
}

#[cfg(test)]
mod tests {
    use super::super::light::{StandardMaterial, brdf};
    use super::*;

    #[test]
    fn flat_detail_is_an_exact_identity() {
        let filtered = filter(
            SlopeMoments {
                sx: 0.0,
                sy: 0.0,
                sx2: 0.0,
                sx_sy: 0.0,
                sy2: 0.0,
            },
            0.4,
        )
        .unwrap();
        assert_eq!(
            filtered.mean,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0
            }
        );
        assert_eq!(
            (
                filtered.variance,
                filtered.roughness,
                filtered.curvature_error
            ),
            (0.0, 0.4, 0.0)
        );
    }

    #[test]
    fn impossible_covariance_fails_closed() {
        assert!(
            filter(
                SlopeMoments {
                    sx: 0.0,
                    sy: 0.0,
                    sx2: 0.1,
                    sx_sy: 1.0,
                    sy2: 0.1
                },
                0.4
            )
            .is_err()
        );
    }

    #[test]
    fn curvature_interval_contains_dense_high_frequency_brdf_integration() {
        const SAMPLES: usize = 16_384;
        const AMPLITUDE: f64 = 0.0625;
        let mut moments = SlopeMoments {
            sx: 0.0,
            sy: 0.0,
            sx2: 0.0,
            sx_sy: 0.0,
            sy2: 0.0,
        };
        let slopes = (0..SAMPLES)
            .map(|sample| {
                let phase = (sample as f64 + 0.5) / SAMPLES as f64;
                (
                    AMPLITUDE * (std::f64::consts::TAU * 17.0 * phase).sin(),
                    AMPLITUDE * (std::f64::consts::TAU * 23.0 * phase).cos(),
                )
            })
            .collect::<Vec<_>>();
        for (sx, sy) in &slopes {
            moments.sx += sx;
            moments.sy += sy;
            moments.sx2 += sx * sx;
            moments.sx_sy += sx * sy;
            moments.sy2 += sy * sy;
        }
        let inverse_count = 1.0 / SAMPLES as f64;
        moments.sx *= inverse_count;
        moments.sy *= inverse_count;
        moments.sx2 *= inverse_count;
        moments.sx_sy *= inverse_count;
        moments.sy2 *= inverse_count;

        let base = StandardMaterial {
            base_color: [0.5, 0.375, 0.25],
            metallic: 0.0,
            roughness: 0.6,
            specular: 0.5,
            emissive: [0.0; 3],
            opacity: 1.0,
        };
        let filtered = filter(moments, base.roughness).unwrap();
        let view = Vec3 {
            x: 0.1,
            y: -0.05,
            z: 1.0,
        };
        let light = Vec3 {
            x: -0.2,
            y: 0.15,
            z: 1.0,
        };
        let candidate = brdf(
            StandardMaterial {
                roughness: filtered.roughness,
                ..base
            },
            filtered.mean,
            view,
            light,
        )
        .unwrap();
        let mut dense = [0.0; 3];
        for (sx, sy) in slopes {
            let normal = Vec3 {
                x: -sx,
                y: -sy,
                z: 1.0,
            }
            .normalize()
            .unwrap();
            let sample = brdf(base, normal, view, light).unwrap();
            for channel in 0..3 {
                dense[channel] += sample[channel] * inverse_count;
            }
        }
        for channel in 0..3 {
            let lower = (candidate[channel] - filtered.curvature_error).max(0.0);
            let upper = candidate[channel] + filtered.curvature_error;
            assert!(
                lower <= dense[channel] && dense[channel] <= upper,
                "channel={channel} interval=[{lower},{upper}] dense={} candidate={} error={}",
                dense[channel],
                candidate[channel],
                filtered.curvature_error,
            );
        }
    }

    #[test]
    fn minimum_roughness_moment_proposal_cannot_understate_ggx_curvature() {
        let amplitude = 0.001;
        let filtered = filter(
            SlopeMoments {
                sx: 0.0,
                sy: 0.0,
                sx2: amplitude * amplitude,
                sx_sy: 0.0,
                sy2: 0.0,
            },
            0.02,
        )
        .unwrap();
        let material = StandardMaterial {
            base_color: [0.5; 3],
            metallic: 0.0,
            roughness: 0.02,
            specular: 0.5,
            emissive: [0.0; 3],
            opacity: 1.0,
        };
        let direction = Vec3 {
            z: 1.0,
            ..Vec3::default()
        };
        let proposal = brdf(
            StandardMaterial {
                roughness: filtered.roughness,
                ..material
            },
            filtered.mean,
            direction,
            direction,
        )
        .unwrap()[0];
        let dense = [-amplitude, amplitude]
            .into_iter()
            .map(|slope| {
                brdf(
                    material,
                    Vec3 {
                        x: -slope,
                        y: 0.0,
                        z: 1.0,
                    },
                    direction,
                    direction,
                )
                .unwrap()[0]
            })
            .sum::<f64>()
            * 0.5;
        let old_variance_heuristic = amplitude * amplitude;
        assert!((proposal - dense).abs() > old_variance_heuristic * 1.0e6);
        assert!((proposal - dense).abs() <= filtered.curvature_error);
        assert_eq!(filtered.curvature_error, BRDF_UNIT_RADIANCE_RADIUS_V1);
    }

    #[test]
    fn terminal_tap_hull_contains_nonuniform_filtered_mixtures() {
        let taps = [
            [0.0, 1.0, 8.0],
            [4.0, 2.0, 6.0],
            [8.0, 3.0, 4.0],
            [12.0, 4.0, 2.0],
        ];
        let (candidate, lower, upper) = terminal_tap_envelope(taps).unwrap();
        let weights = [0.7, 0.2, 0.075, 0.025];
        let mut filtered = [0.0; 3];
        for (tap, weight) in taps.into_iter().zip(weights) {
            for channel in 0..3 {
                filtered[channel] += tap[channel] * weight;
            }
        }
        assert_ne!(
            candidate, filtered,
            "the terminal weights are not assumed uniform"
        );
        for channel in 0..3 {
            assert!(lower[channel] <= filtered[channel]);
            assert!(filtered[channel] <= upper[channel]);
        }
        assert!(terminal_tap_envelope([[f64::NAN; 3]; 4]).is_err());
    }
}
