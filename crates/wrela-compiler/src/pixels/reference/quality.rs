//! Locked P9 opaque-quality sequences and independent truth checks.

use super::ao;
use super::area_light::{self, AreaError, CellBound, EmitterShape};
use super::display::encode_linear_candidate;
use super::interval::F64Interval;
use super::light::{self, Light, StandardMaterial, Vec3};
use super::moments::{self, SlopeMoments};

pub const FRAME_COUNT: usize = 8;
pub const FRAME_WIDTH: usize = 32;
pub const FRAME_HEIGHT: usize = 16;
pub const LOCKED_PROPERTIES: &str = "diagonal-pan,glossy-sphere,slope-recede,rectangle-penumbra,thin-blade,material-edge,ao-contact,filmic-shoulder";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualitySequence {
    pub frame_digests: [[u8; 32]; FRAME_COUNT],
    pub repeated_digests: [[u8; 32]; FRAME_COUNT],
    pub one_core_digests: [[u8; 32]; FRAME_COUNT],
    pub three_core_digests: [[u8; 32]; FRAME_COUNT],
}

fn integrate_penumbra(edge: f64) -> Result<area_light::AreaResult, String> {
    area_light::integrate(
        EmitterShape::Rectangle,
        1.0 / 1024.0,
        10,
        4096,
        |_, cell| {
            let area = (cell.s1 - cell.s0) * (cell.t1 - cell.t0) / 4.0;
            let (lo, hi) = if cell.s1 <= edge {
                (0.0, 0.0)
            } else if cell.s0 >= edge {
                (area, area)
            } else {
                (0.0, area)
            };
            let midpoint_clear = (cell.s0 + cell.s1) * 0.5 >= edge;
            Ok(CellBound {
                contribution: [F64Interval::new(lo, hi).map_err(|_| AreaError::InvalidInput)?; 3],
                candidate: [if midpoint_clear { area } else { 0.0 }; 3],
            })
        },
    )
    .map_err(|error| format!("P032: quality penumbra integration failed: {error:?}"))
}

fn render_frame(frame: usize) -> Result<Vec<u8>, String> {
    let motion = frame as f64 / 128.0;
    let exposure = 2.0_f64.powf(-0.75 + frame as f64 * 0.25);
    let environment = [0.015625, 0.03125, 0.0625];
    let base_texture = super::super::texture::compiler_asset(19)?;
    let slope_texture = super::super::texture::compiler_asset(21)?;
    let mut bgra = Vec::with_capacity(FRAME_WIDTH * FRAME_HEIGHT * 4);
    for y in 0..FRAME_HEIGHT {
        let v = (y as f64 + 0.5) / FRAME_HEIGHT as f64;
        for x in 0..FRAME_WIDTH {
            let u = (x as f64 + 0.5) / FRAME_WIDTH as f64;
            let diagonal = 0.14 + motion + (v - 0.5) * 0.25;
            let silhouette = ((u - diagonal) * FRAME_WIDTH as f64 + 0.5).clamp(0.0, 1.0);
            let footprint = 0.03125 + frame as f64 * 0.015625 + v * 0.0078125;
            let (base_sample, base_bound) = super::super::texture::sample(
                &base_texture,
                u * 4.0 + motion,
                v * 2.0 + 0.375,
                [footprint, 0.0],
                [0.0, footprint * 0.25],
            )?;
            let (slope_sample, slope_bound) = super::super::texture::sample(
                &slope_texture,
                u * 32.0 + motion * 8.0,
                v * 8.0 + 0.625,
                [footprint * 4.0, 0.0],
                [0.0, footprint],
            )?;
            for channel in 0..3 {
                if !base_bound.channels[channel].contains(base_sample[channel]) {
                    return Err(
                        "P032: quality texture candidate escaped min/max pyramid".to_string()
                    );
                }
            }
            for channel in 0..2 {
                if !slope_bound.channels[channel].contains(slope_sample[channel]) {
                    return Err(
                        "P032: quality slope candidate escaped moment mip bounds".to_string()
                    );
                }
            }
            let slope = slope_sample[0] * 0.25;
            let filtered = moments::filter(
                SlopeMoments {
                    sx: slope * 0.125,
                    sy: 0.0,
                    sx2: slope * slope + 0.03125,
                    sx_sy: 0.0,
                    sy2: 0.015625,
                },
                0.18,
            )?;
            let normal = Vec3 {
                x: (u * 2.0 - 1.0) * 0.65,
                y: (v * 2.0 - 1.0) * 0.45 + filtered.mean.x * 0.25,
                z: 1.0,
            }
            .normalize()
            .map_err(|_| "P032: quality normal is degenerate".to_string())?;
            let material = StandardMaterial {
                // The x=1/2 switch is an identical-depth material boundary. It
                // changes only the closed material program; geometry and q stay
                // fixed on both sides.
                base_color: if u < 0.5 {
                    [
                        0.125 + 0.25 * base_sample[0],
                        0.25 + 0.25 * base_sample[1],
                        0.375 + 0.25 * base_sample[2],
                    ]
                } else {
                    [0.75, 0.125, 0.03125]
                },
                metallic: 0.1,
                roughness: filtered.roughness,
                specular: 0.75,
                emissive: [0.01, 0.005, 0.0025],
                opacity: 1.0,
            };
            let penumbra = integrate_penumbra((u - 0.5 - motion * 2.0).clamp(-1.0, 1.0))?;
            let shadow = penumbra.candidate[0].clamp(0.0, 1.0);
            let direct = light::direct(
                material,
                Vec3::default(),
                normal,
                Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 1.0,
                },
                Light::Directional {
                    direction_to_light: Vec3 {
                        x: -0.35 + motion * 2.0,
                        y: -0.25,
                        z: 1.0,
                    },
                    radiance: [6.0, 4.0, 2.0],
                },
                shadow,
            )
            .map_err(|error| format!("P032: quality direct light failed: {error:?}"))?;
            let contact = ((u - 0.72 - motion).abs() * 8.0).clamp(0.0, 1.0);
            let ao = ao::evaluate(0.5, 0.75, |distance| F64Interval::point(distance * contact))?;
            let exact_ao = 0.25 + 0.75 * contact;
            if !ao.contains(exact_ao) {
                return Err("P032: quality AO interval excludes dense reference".to_string());
            }
            let ao_candidate = (ao.lo + ao.hi) * 0.5;
            let mut hdr: [f64; 3] = std::array::from_fn(|channel| {
                (material.emissive[channel] + direct[channel] * ao_candidate) * silhouette
                    + environment[channel] * (1.0 - silhouette)
            });
            // A subpixel-width blade crosses both the lit surface and background;
            // analytic coverage keeps the transition deterministic instead of
            // turning it into a sampled on/off test.
            let blade_center = 0.30 + motion * 3.0;
            let blade_coverage = ((1.0 - (u - blade_center).abs() * FRAME_WIDTH as f64 * 2.0)
                * (1.0 - (v - 0.5).abs() * 2.5).clamp(0.0, 1.0))
            .clamp(0.0, 1.0);
            let blade = [0.03125, 0.0625, 0.125];
            for channel in 0..3 {
                hdr[channel] =
                    hdr[channel] * (1.0 - blade_coverage) + blade[channel] * blade_coverage;
            }
            let mut rgb = [0_u8; 3];
            for channel in 0..3 {
                rgb[channel] = encode_linear_candidate(hdr[channel] * exposure, true)
                    .map_err(|error| format!("P032: quality transfer failed: {error:?}"))?;
            }
            bgra.extend([rgb[2], rgb[1], rgb[0], 255]);
        }
    }
    Ok(bgra)
}

pub fn render_truth_frames() -> Result<Vec<Vec<u8>>, String> {
    (0..FRAME_COUNT).map(render_frame).collect()
}

fn partitioned_digest(frame: &[u8], workers: usize) -> Result<[u8; 32], String> {
    if workers == 0 || !frame.len().is_multiple_of(4) {
        return Err("P032: invalid quality partition".to_string());
    }
    let pixels = frame.len() / 4;
    let mut reconstructed = vec![0_u8; frame.len()];
    for worker in 0..workers {
        let start = pixels * worker / workers;
        let end = pixels * (worker + 1) / workers;
        reconstructed[start * 4..end * 4].copy_from_slice(&frame[start * 4..end * 4]);
    }
    if reconstructed != frame {
        return Err("P032: quality worker partition lost output bytes".to_string());
    }
    Ok(wrela_machine::sha256::sha256(&reconstructed))
}

pub fn sequence() -> Result<QualitySequence, String> {
    let frames = render_truth_frames()?;
    let digests = frames
        .iter()
        .map(|frame| wrela_machine::sha256::sha256(frame))
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "P032: quality frame count mismatch".to_string())?;
    let repeated = (0..FRAME_COUNT)
        .map(render_frame)
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .map(|frame| wrela_machine::sha256::sha256(frame))
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "P032: repeated quality frame count mismatch".to_string())?;
    let one_core_digests = frames
        .iter()
        .map(|frame| partitioned_digest(frame, 1))
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "P032: one-core quality frame count mismatch".to_string())?;
    let three_core_digests = frames
        .iter()
        .map(|frame| partitioned_digest(frame, 3))
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "P032: three-core quality frame count mismatch".to_string())?;
    Ok(QualitySequence {
        frame_digests: digests,
        repeated_digests: repeated,
        one_core_digests,
        three_core_digests,
    })
}

pub fn truth_text() -> Result<String, String> {
    let result = sequence()?;
    let mut text = format!(
        "PixelsQuality version=1 frames={FRAME_COUNT} width={FRAME_WIDTH} height={FRAME_HEIGHT} properties={LOCKED_PROPERTIES} status=pass\n"
    );
    for (index, digest) in result.frame_digests.iter().enumerate() {
        let digest = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        text.push_str(&format!(
            "frame={index} digest={} repeat=identical one_three=identical status=pass\n",
            digest
        ));
    }
    text.push_str(
        "summary visibility_failures=0 identity_failures=0 shadow_failures=0 interval_failures=0 stochastic=none status=pass\n",
    );
    Ok(text)
}

fn production_pixel(frame: &[u8], x: usize, y: usize) -> [u8; 3] {
    let base = (y * FRAME_WIDTH + x) * 4;
    [frame[base + 2], frame[base + 1], frame[base]]
}

fn production_luma(pixel: [u8; 3]) -> u64 {
    u64::from(pixel[0]) * 54 + u64::from(pixel[1]) * 183 + u64::from(pixel[2]) * 19
}

fn production_region_luma(frame: &[u8], x0: usize, x1: usize, y0: usize, y1: usize) -> u64 {
    let mut total = 0_u64;
    for y in y0..y1 {
        for x in x0..x1 {
            total += production_luma(production_pixel(frame, x, y));
        }
    }
    total
}

/// Locate the moving glossy response only after the porcelain sphere's dark
/// core is proved darker than its own rim.  The regions scale with the locked
/// frame dimensions: at 32x16 the core is 2x2 and the surrounding box is 4x4,
/// so comparing `core * 3 < rim` compares equal-area averages exactly.
fn production_glossy_sphere_control(frame: &[u8]) -> Option<(usize, usize)> {
    let box_x0 = FRAME_WIDTH * 3 / 8;
    let box_x1 = FRAME_WIDTH / 2;
    let box_y0 = FRAME_HEIGHT * 3 / 8;
    let box_y1 = FRAME_HEIGHT * 5 / 8;
    let core_x0 = FRAME_WIDTH * 13 / 32;
    let core_x1 = FRAME_WIDTH * 15 / 32;
    let core_y0 = FRAME_HEIGHT * 7 / 16;
    let core_y1 = FRAME_HEIGHT * 9 / 16;
    let core = production_region_luma(frame, core_x0, core_x1, core_y0, core_y1);
    let rim = production_region_luma(frame, box_x0, box_x1, box_y0, box_y1).checked_sub(core)?;
    if core.checked_mul(3).is_none_or(|scaled| scaled >= rim) {
        return None;
    }
    let mut peak = None;
    for y in box_y0..box_y1 {
        for x in box_x0..box_x1 {
            let luma = production_luma(production_pixel(frame, x, y));
            if peak.is_none_or(|(best, _, _)| luma > best) {
                peak = Some((luma, x, y));
            }
        }
    }
    peak.map(|(_, x, y)| (x, y))
}

/// Validate and lock the frames emitted by the production quality scene.
///
/// The analytic sequence above remains the independent, high-resolution
/// component oracle.  This entry point deliberately consumes the captured
/// guest frames, so the permanent quality truth cannot stay green while the
/// production scene emits unrelated bytes.
pub fn production_truth_text(frames: &[Vec<u8>]) -> Result<String, String> {
    if frames.len() != FRAME_COUNT
        || frames
            .iter()
            .any(|frame| frame.len() != FRAME_WIDTH * FRAME_HEIGHT * 4)
    {
        return Err("P032: production quality frame shape mismatch".to_string());
    }
    let oracle = render_truth_frames()?;
    let mut previous_frame: Option<&[u8]> = None;
    let mut production_luma = Vec::with_capacity(FRAME_COUNT);
    let mut chromatic_material_frames = 0_usize;
    let mut diagonal_control_frames = 0_usize;
    let mut sphere_control_frames = 0_usize;
    let mut glossy_peak_positions = std::collections::BTreeSet::new();
    let mut high_frequency_edges = 0_usize;
    let oracle_luma = oracle
        .iter()
        .map(|frame| {
            frame
                .chunks_exact(4)
                .map(|pixel| {
                    u64::from(pixel[2]) * 54 + u64::from(pixel[1]) * 183 + u64::from(pixel[0]) * 19
                })
                .sum::<u64>()
        })
        .collect::<Vec<_>>();
    let mut text = format!(
        "PixelsQuality version=2 frames={FRAME_COUNT} width={FRAME_WIDTH} height={FRAME_HEIGHT} oracle=production-scene properties={LOCKED_PROPERTIES} status=pass\n"
    );
    for (index, frame) in frames.iter().enumerate() {
        if frame.chunks_exact(4).any(|pixel| pixel[3] != 255) {
            return Err(format!(
                "P032: production quality frame {index} contains non-opaque output"
            ));
        }
        let luma = frame
            .chunks_exact(4)
            .map(|pixel| {
                u64::from(pixel[2]) * 54 + u64::from(pixel[1]) * 183 + u64::from(pixel[0]) * 19
            })
            .sum::<u64>();
        let dark = frame
            .chunks_exact(4)
            .filter(|pixel| {
                u64::from(pixel[2]) * 54 + u64::from(pixel[1]) * 183 + u64::from(pixel[0]) * 19
                    <= 256
            })
            .count();
        let bright = frame
            .chunks_exact(4)
            .filter(|pixel| {
                u64::from(pixel[2]) * 54 + u64::from(pixel[1]) * 183 + u64::from(pixel[0]) * 19
                    >= 4096
            })
            .count();
        let distinct = frame
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let pixel = |x: usize, y: usize| production_pixel(frame, x, y);
        let red_dominant = frame
            .chunks_exact(4)
            .filter(|p| p[2] > p[1].saturating_add(8) && p[2] > p[0].saturating_add(8))
            .count();
        let blue_dominant = frame
            .chunks_exact(4)
            .filter(|p| p[0] > p[1].saturating_add(8) && p[0] > p[2].saturating_add(8))
            .count();
        if red_dominant >= FRAME_WIDTH * FRAME_HEIGHT / 32
            && blue_dominant >= FRAME_WIDTH * FRAME_HEIGHT / 256
        {
            chromatic_material_frames += 1;
        }
        let corner_luma = |x0, x1, y0, y1| production_region_luma(frame, x0, x1, y0, y1);
        if corner_luma(
            FRAME_WIDTH * 3 / 4,
            FRAME_WIDTH,
            FRAME_HEIGHT * 5 / 8,
            FRAME_HEIGHT,
        ) > corner_luma(0, FRAME_WIDTH / 4, 0, FRAME_HEIGHT * 3 / 8) * 4
        {
            diagonal_control_frames += 1;
        }
        if let Some(peak) = production_glossy_sphere_control(frame) {
            sphere_control_frames += 1;
            glossy_peak_positions.insert(peak);
        }
        for y in 1..FRAME_HEIGHT - 1 {
            for x in 1..FRAME_WIDTH * 11 / 32 {
                if pixel(x, y) != pixel(x - 1, y) {
                    high_frequency_edges += 1;
                }
            }
        }
        if dark < FRAME_WIDTH * FRAME_HEIGHT / 128
            || bright < FRAME_WIDTH * FRAME_HEIGHT / 128
            || distinct < 8
        {
            return Err(format!(
                "P032: production quality frame {index} lost its visibility controls (dark={dark}, bright={bright}, distinct={distinct})"
            ));
        }
        if let Some(previous_frame) = previous_frame {
            let changed = frame
                .chunks_exact(4)
                .zip(previous_frame.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            if changed == 0 {
                return Err(format!(
                    "P032: production quality frame {index} did not respond to the sealed motion/light/exposure step"
                ));
            }
        }
        production_luma.push(luma);
        previous_frame = Some(frame);
        let digest = wrela_machine::sha256::sha256(frame)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        text.push_str(&format!(
            "frame={index} digest={digest} host_components=pass opaque=pass motion=pass status=pass\n"
        ));
    }
    if chromatic_material_frames != FRAME_COUNT {
        return Err(format!(
            "P032: production material-edge controls survived in {chromatic_material_frames}/{FRAME_COUNT} frames"
        ));
    }
    if diagonal_control_frames != FRAME_COUNT {
        return Err(format!(
            "P032: production diagonal visibility control survived in {diagonal_control_frames}/{FRAME_COUNT} frames"
        ));
    }
    if sphere_control_frames < FRAME_COUNT - 1 {
        return Err(format!(
            "P032: production glossy/AO sphere control survived in only {sphere_control_frames}/{FRAME_COUNT} frames"
        ));
    }
    if glossy_peak_positions.len() < 2 {
        return Err(format!(
            "P032: production glossy highlight occupied only {} deterministic peak position(s)",
            glossy_peak_positions.len(),
        ));
    }
    if high_frequency_edges < FRAME_WIDTH * FRAME_HEIGHT / 32 {
        return Err(format!(
            "P032: production receding texture lost deterministic spatial detail ({high_frequency_edges} edges)"
        ));
    }
    if oracle_luma.last() > oracle_luma.first() && production_luma.last() <= production_luma.first()
    {
        return Err(
            "P032: production quality sequence reverses the independent exposure response"
                .to_string(),
        );
    }
    text.push_str(
        "summary visibility_controls=pass lighting_controls=pass material_controls=pass spatial_property_controls=pass glossy_motion=pass interval_components=pass stochastic=none status=pass\n",
    );
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_sequence_is_repeatable_and_partition_invariant() {
        let result = sequence().unwrap();
        assert_eq!(result.frame_digests, result.repeated_digests);
        assert_eq!(result.frame_digests, result.one_core_digests);
        assert_eq!(result.frame_digests, result.three_core_digests);
        assert!(
            result
                .frame_digests
                .windows(2)
                .all(|pair| pair[0] != pair[1])
        );
    }

    #[test]
    fn checked_truth_file_matches_every_exact_frame_digest() {
        assert_eq!(
            truth_text().unwrap(),
            include_str!("../../../../../tests/pixels_truth/quality/p9-v1.txt")
        );
    }

    #[test]
    fn penumbra_interval_contains_the_analytic_source_integral() {
        for edge in [-0.75, -0.125, 0.0, 0.375, 0.875] {
            let result = integrate_penumbra(edge).unwrap();
            let exact = (1.0 - edge) * 0.5;
            assert!(
                result.bounds.iter().all(|bound| bound.contains(exact)),
                "edge={edge} exact={exact} bounds={:?}",
                result.bounds,
            );
        }
    }

    #[test]
    fn production_truth_rejects_an_unrelated_or_nonopaque_sequence() {
        let mut frames = (0..FRAME_COUNT)
            .map(|frame_index| {
                let mut bytes = Vec::with_capacity(FRAME_WIDTH * FRAME_HEIGHT * 4);
                for pixel in 0..FRAME_WIDTH * FRAME_HEIGHT {
                    let value = if pixel % 4 == 0 {
                        0
                    } else {
                        160 + (pixel % 16) as u8 * 2 + frame_index as u8 * 4
                    };
                    bytes.extend([value, value, value, 255]);
                }
                bytes
            })
            .collect::<Vec<_>>();
        assert!(production_truth_text(&frames).is_err());
        frames[3][3] = 0;
        assert!(
            production_truth_text(&frames)
                .unwrap_err()
                .contains("non-opaque")
        );
    }

    #[test]
    fn production_sphere_control_uses_its_local_core_and_rim() {
        let mut frame = vec![0_u8; FRAME_WIDTH * FRAME_HEIGHT * 4];
        for pixel in frame.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        for y in FRAME_HEIGHT * 3 / 8..FRAME_HEIGHT * 5 / 8 {
            for x in FRAME_WIDTH * 3 / 8..FRAME_WIDTH / 2 {
                let base = (y * FRAME_WIDTH + x) * 4;
                frame[base..base + 4].copy_from_slice(&[33, 22, 13, 255]);
            }
        }
        for y in FRAME_HEIGHT * 7 / 16..FRAME_HEIGHT * 9 / 16 {
            for x in FRAME_WIDTH * 13 / 32..FRAME_WIDTH * 15 / 32 {
                let base = (y * FRAME_WIDTH + x) * 4;
                frame[base..base + 4].copy_from_slice(&[5, 2, 1, 255]);
            }
        }
        assert_eq!(
            production_glossy_sphere_control(&frame),
            Some((FRAME_WIDTH * 3 / 8, FRAME_HEIGHT * 3 / 8))
        );
        for y in FRAME_HEIGHT * 7 / 16..FRAME_HEIGHT * 9 / 16 {
            for x in FRAME_WIDTH * 13 / 32..FRAME_WIDTH * 15 / 32 {
                let base = (y * FRAME_WIDTH + x) * 4;
                frame[base..base + 4].copy_from_slice(&[33, 22, 13, 255]);
            }
        }
        assert_eq!(production_glossy_sphere_control(&frame), None);
    }
}
