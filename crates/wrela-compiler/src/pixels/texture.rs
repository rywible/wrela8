//! Deterministic compiler-owned texture assets and certified sampling.

use super::reference::interval::F64Interval;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    Rgb8Srgb,
    Rgb8Linear,
    Rg8Snorm,
    R8Linear,
}

impl TextureFormat {
    pub const fn channels(self) -> usize {
        match self {
            Self::Rgb8Srgb | Self::Rgb8Linear => 3,
            Self::Rg8Snorm => 2,
            Self::R8Linear => 1,
        }
    }

    pub const fn tag(self) -> u64 {
        match self {
            Self::Rgb8Srgb => 1,
            Self::Rgb8Linear => 2,
            Self::Rg8Snorm => 3,
            Self::R8Linear => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapMode {
    Clamp,
    Repeat,
}

/// Closed v1 mapping set. No callback or arbitrary topology UV program is
/// representable in a sealed texture record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvSource {
    Plane,
    Sphere,
    Cylinder,
    Torus,
    BoxFeature,
    RoundBoxFeature,
    ObjectTriplanar,
    WorldTriplanar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvEvent {
    Continuous,
    RepeatSeamU,
    RepeatSeamV,
    FeatureBoundary,
}

pub fn classify_uv_event(
    wrap_u: WrapMode,
    wrap_v: WrapMode,
    u_interval: [f64; 2],
    v_interval: [f64; 2],
) -> Result<UvEvent, String> {
    if !u_interval.into_iter().chain(v_interval).all(f64::is_finite)
        || u_interval[0] > u_interval[1]
        || v_interval[0] > v_interval[1]
    {
        return Err("P023: invalid UV event interval".to_string());
    }
    let crosses_integer = |range: [f64; 2]| range[0].floor() != range[1].floor();
    if wrap_u == WrapMode::Repeat && crosses_integer(u_interval) {
        Ok(UvEvent::RepeatSeamU)
    } else if wrap_v == WrapMode::Repeat && crosses_integer(v_interval) {
        Ok(UvEvent::RepeatSeamV)
    } else {
        Ok(UvEvent::Continuous)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MipLevel {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
    pub channel_min: Vec<u8>,
    pub channel_max: Vec<u8>,
    /// Per-texel E[sx], E[sy], E[sx²], E[sx sy], E[sy²] in signed
    /// Q16.16. A mip texel stores the exact deterministic aggregate of its
    /// four children, so filtered moment footprints remain local instead of
    /// accidentally using a whole-level average.
    pub slope_moments: Option<Vec<[i64; 5]>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureAsset {
    pub stable_id: u32,
    pub name: &'static str,
    pub format: TextureFormat,
    pub wrap_u: WrapMode,
    pub wrap_v: WrapMode,
    pub width: u32,
    pub height: u32,
    pub digest: String,
    pub mips: Vec<MipLevel>,
}

impl TextureAsset {
    pub fn identity_bytes(&self) -> Vec<u8> {
        let mut identity = Vec::new();
        identity.extend_from_slice(b"wrela-texture-v1\0");
        identity.extend_from_slice(&self.stable_id.to_le_bytes());
        identity.extend_from_slice(&self.format.tag().to_le_bytes());
        identity.push(match self.wrap_u {
            WrapMode::Clamp => 0,
            WrapMode::Repeat => 1,
        });
        identity.push(match self.wrap_v {
            WrapMode::Clamp => 0,
            WrapMode::Repeat => 1,
        });
        for mip in &self.mips {
            identity.extend_from_slice(&mip.width.to_le_bytes());
            identity.extend_from_slice(&mip.height.to_le_bytes());
            identity.extend_from_slice(&mip.bytes);
            identity.extend_from_slice(&mip.channel_min);
            identity.extend_from_slice(&mip.channel_max);
            if let Some(moments) = &mip.slope_moments {
                identity.extend_from_slice(&(moments.len() as u64).to_le_bytes());
                for texel in moments {
                    for moment in texel {
                        identity.extend_from_slice(&moment.to_le_bytes());
                    }
                }
            }
        }
        identity
    }

    pub fn digest_bytes(&self) -> [u8; 32] {
        wrela_machine::sha256::sha256(&self.identity_bytes())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleInterval {
    pub channels: [F64Interval; 3],
    pub channel_count: u8,
}

const CHECKER_2X2_V1_RGB8_SRGB: &[u8] = &[0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0];
const CHECKER_2X2_V1_RGB8_LINEAR: &[u8] = &[0, 64, 255, 255, 128, 0, 255, 128, 0, 0, 64, 255];
const SLOPE_2X2_V1_RG8_SNORM: &[u8] = &[(-64_i8) as u8, 0, 64, 0, 0, (-64_i8) as u8, 0, 64];
// Four signed codes are the smallest symmetric slope whose first and second
// moments remain positive-semidefinite after the sealed Q16.16 encoding at
// every texel and mip. Smaller nonzero codes can round E[sx^2] below E[sx]^2.
const FINE_SLOPE_2X2_V1_RG8_SNORM: &[u8] = &[(-4_i8) as u8, 0, 4, 0, 0, (-4_i8) as u8, 0, 4];
const MASK_2X2_V1_R8_LINEAR: &[u8] = &[0, 85, 170, 255];

fn linear_to_srgb_u8(linear: u32) -> u8 {
    // `linear` is Q0.16. Compiler-only f64 evaluation chooses bytes once;
    // the resulting mip bytes and asset digest are serialized and verified.
    let x = f64::from(linear) / 65_535.0;
    let encoded = if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (encoded.mul_add(255.0, 0.5).floor() as i32).clamp(0, 255) as u8
}

fn srgb_to_linear_q16(encoded: u8) -> u32 {
    let x = f64::from(encoded) / 255.0;
    let linear = if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    };
    (linear.mul_add(65_535.0, 0.5).floor() as i64).clamp(0, 65_535) as u32
}

fn downsample_channel(format: TextureFormat, samples: &[u8]) -> u8 {
    if format == TextureFormat::Rgb8Srgb {
        let sum = samples
            .iter()
            .map(|value| u64::from(srgb_to_linear_q16(*value)))
            .sum::<u64>();
        linear_to_srgb_u8(((sum + samples.len() as u64 / 2) / samples.len() as u64) as u32)
    } else if format == TextureFormat::Rg8Snorm {
        let sum = samples
            .iter()
            .map(|value| i32::from(*value as i8))
            .sum::<i32>();
        let magnitude = sum.unsigned_abs();
        let rounded = ((magnitude + samples.len() as u32 / 2) / samples.len() as u32) as i32;
        let signed = if sum < 0 { -rounded } else { rounded }.clamp(-128, 127) as i8;
        signed as u8
    } else {
        ((samples.iter().map(|value| u32::from(*value)).sum::<u32>() + samples.len() as u32 / 2)
            / samples.len() as u32) as u8
    }
}

fn summarize(format: TextureFormat, bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let channels = format.channels();
    let mut minimum = vec![u8::MAX; channels];
    let mut maximum = vec![u8::MIN; channels];
    for texel in bytes.chunks_exact(channels) {
        for channel in 0..channels {
            if format == TextureFormat::Rg8Snorm {
                if (texel[channel] as i8) < (minimum[channel] as i8) {
                    minimum[channel] = texel[channel];
                }
                if (texel[channel] as i8) > (maximum[channel] as i8) {
                    maximum[channel] = texel[channel];
                }
            } else {
                minimum[channel] = minimum[channel].min(texel[channel]);
                maximum[channel] = maximum[channel].max(texel[channel]);
            }
        }
    }
    (minimum, maximum)
}

fn div_round_nearest(value: i64, denominator: i64) -> i64 {
    let magnitude = value.unsigned_abs();
    let denominator = denominator as u64;
    let rounded = (magnitude + denominator / 2) / denominator;
    if value < 0 {
        -(rounded as i64)
    } else {
        rounded as i64
    }
}

fn slope_q16(byte: u8) -> i64 {
    let value = i64::from(byte as i8);
    if value <= -127 {
        -65_536
    } else {
        div_round_nearest(value * 65_536, 127)
    }
}

fn slope_base_moments(bytes: &[u8]) -> Vec<[i64; 5]> {
    bytes
        .chunks_exact(2)
        .map(|texel| {
            let sx = slope_q16(texel[0]);
            let sy = slope_q16(texel[1]);
            [
                sx,
                sy,
                div_round_nearest(sx * sx, 65_536),
                div_round_nearest(sx * sy, 65_536),
                div_round_nearest(sy * sy, 65_536),
            ]
        })
        .collect()
}

pub fn build_mips(
    format: TextureFormat,
    width: u32,
    height: u32,
    bytes: &[u8],
) -> Result<Vec<MipLevel>, String> {
    if width == 0 || height == 0 {
        return Err("P023: texture dimensions must be positive".to_string());
    }
    let channels = format.channels();
    let expected = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "P023: texture byte length overflow".to_string())?;
    if bytes.len() != expected {
        return Err(format!(
            "P023: texture has {} bytes, expected {expected}",
            bytes.len()
        ));
    }
    let mut levels = Vec::new();
    let (mut w, mut h, mut current) = (width, height, bytes.to_vec());
    let mut current_moments =
        (format == TextureFormat::Rg8Snorm).then(|| slope_base_moments(bytes));
    loop {
        let (channel_min, channel_max) = summarize(format, &current);
        levels.push(MipLevel {
            width: w,
            height: h,
            bytes: current.clone(),
            channel_min,
            channel_max,
            slope_moments: current_moments.clone(),
        });
        if w == 1 && h == 1 {
            break;
        }
        let next_w = w.div_ceil(2);
        let next_h = h.div_ceil(2);
        let mut next = vec![0_u8; next_w as usize * next_h as usize * channels];
        let mut next_moments = current_moments
            .as_ref()
            .map(|_| vec![[0_i64; 5]; next_w as usize * next_h as usize]);
        for y in 0..next_h {
            for x in 0..next_w {
                for channel in 0..channels {
                    let mut samples = [0_u8; 4];
                    let mut count = 0_usize;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = x * 2 + dx;
                            let sy = y * 2 + dy;
                            if sx >= w || sy >= h {
                                continue;
                            }
                            samples[count] = current
                                [(sy as usize * w as usize + sx as usize) * channels + channel];
                            count += 1;
                        }
                    }
                    next[(y as usize * next_w as usize + x as usize) * channels + channel] =
                        downsample_channel(format, &samples[..count]);
                }
                if let (Some(current), Some(next)) = (&current_moments, &mut next_moments) {
                    let mut sums = [0_i64; 5];
                    let mut count = 0_i64;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = x * 2 + dx;
                            let sy = y * 2 + dy;
                            if sx >= w || sy >= h {
                                continue;
                            }
                            let source = current[sy as usize * w as usize + sx as usize];
                            for moment in 0..5 {
                                sums[moment] += source[moment];
                            }
                            count += 1;
                        }
                    }
                    next[y as usize * next_w as usize + x as usize] =
                        sums.map(|sum| div_round_nearest(sum, count));
                }
            }
        }
        (w, h, current, current_moments) = (next_w, next_h, next, next_moments);
    }
    Ok(levels)
}

pub fn compiler_asset(stable_id: u32) -> Result<TextureAsset, String> {
    let (name, format, width, height, bytes, wrap_u, wrap_v) = match stable_id {
        19 => (
            "Checker2x2V1",
            TextureFormat::Rgb8Srgb,
            2,
            2,
            CHECKER_2X2_V1_RGB8_SRGB,
            WrapMode::Repeat,
            WrapMode::Repeat,
        ),
        20 => (
            "LinearData2x2V1",
            TextureFormat::Rgb8Linear,
            2,
            2,
            CHECKER_2X2_V1_RGB8_LINEAR,
            WrapMode::Clamp,
            WrapMode::Clamp,
        ),
        21 => (
            "ObjectSlope2x2V1",
            TextureFormat::Rg8Snorm,
            2,
            2,
            SLOPE_2X2_V1_RG8_SNORM,
            WrapMode::Repeat,
            WrapMode::Repeat,
        ),
        22 => (
            "Mask2x2V1",
            TextureFormat::R8Linear,
            2,
            2,
            MASK_2X2_V1_R8_LINEAR,
            WrapMode::Clamp,
            WrapMode::Clamp,
        ),
        23 => (
            "FineSlope2x2V1",
            TextureFormat::Rg8Snorm,
            2,
            2,
            FINE_SLOPE_2X2_V1_RG8_SNORM,
            WrapMode::Repeat,
            WrapMode::Repeat,
        ),
        other => {
            return Err(format!(
                "P023: unknown compiler-owned texture asset id `{other}`"
            ));
        }
    };
    let mips = build_mips(format, width, height, bytes)?;
    let mut asset = TextureAsset {
        stable_id,
        name,
        format,
        wrap_u,
        wrap_v,
        width,
        height,
        digest: String::new(),
        mips,
    };
    asset.digest = wrela_machine::sha256::sha256_hex(&asset.identity_bytes());
    Ok(asset)
}

fn wrap(value: f64, mode: WrapMode) -> f64 {
    match mode {
        WrapMode::Clamp => value.clamp(0.0, 1.0),
        WrapMode::Repeat => value.rem_euclid(1.0),
    }
}

fn texel(asset: &TextureAsset, level: usize, x: i64, y: i64, channel: usize) -> f64 {
    let mip = &asset.mips[level];
    let coordinate = |value: i64, size: u32, mode| match mode {
        WrapMode::Clamp => value.clamp(0, i64::from(size) - 1) as usize,
        WrapMode::Repeat => value.rem_euclid(i64::from(size)) as usize,
    };
    let x = coordinate(x, mip.width, asset.wrap_u);
    let y = coordinate(y, mip.height, asset.wrap_v);
    let byte = mip.bytes[(y * mip.width as usize + x) * asset.format.channels() + channel];
    match asset.format {
        TextureFormat::Rgb8Srgb => f64::from(srgb_to_linear_q16(byte)) / 65_535.0,
        TextureFormat::Rg8Snorm => (f64::from(byte as i8) / 127.0).max(-1.0),
        TextureFormat::Rgb8Linear | TextureFormat::R8Linear => f64::from(byte) / 255.0,
    }
}

fn bilinear(asset: &TextureAsset, level: usize, u: f64, v: f64) -> [f64; 3] {
    let mip = &asset.mips[level];
    let x = wrap(u, asset.wrap_u) * f64::from(mip.width) - 0.5;
    let y = wrap(v, asset.wrap_v) * f64::from(mip.height) - 0.5;
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let mut result = [0.0; 3];
    for (channel, output) in result.iter_mut().enumerate().take(asset.format.channels()) {
        let a = texel(asset, level, x0, y0, channel);
        let b = texel(asset, level, x0 + 1, y0, channel);
        let c = texel(asset, level, x0, y0 + 1, channel);
        let d = texel(asset, level, x0 + 1, y0 + 1, channel);
        *output = (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy;
    }
    result
}

/// Deterministic trilinear/4-tap anisotropic candidate plus a conservative
/// min/max enclosure from the two selected mip levels.
pub fn sample(
    asset: &TextureAsset,
    u: f64,
    v: f64,
    du: [f64; 2],
    dv: [f64; 2],
) -> Result<([f64; 3], SampleInterval), String> {
    if ![u, v, du[0], du[1], dv[0], dv[1]]
        .into_iter()
        .all(f64::is_finite)
    {
        return Err("P023: non-finite certified texture footprint".to_string());
    }
    let axis_u = (du[0] * f64::from(asset.width)).hypot(du[1] * f64::from(asset.height));
    let axis_v = (dv[0] * f64::from(asset.width)).hypot(dv[1] * f64::from(asset.height));
    let major = axis_u.max(axis_v).max(1.0);
    let minor = axis_u.min(axis_v).max(1.0);
    let anisotropic = major / minor > 2.0;
    let mut lod_axis = if anisotropic { minor } else { major };
    let mut lo = 0_usize;
    while lod_axis >= 2.0 && lo + 1 < asset.mips.len() {
        lod_axis *= 0.5;
        lo += 1;
    }
    let hi = (lo + 1).min(asset.mips.len() - 1);
    // The v1 mip coordinate is a deterministic linear octave coordinate.
    // Its adjacent-level min/max enclosure, not this proposal, certifies the
    // filtered result.
    let blend = if hi == lo {
        0.0
    } else {
        (lod_axis - 1.0).clamp(0.0, 1.0)
    };
    let mut candidate = [0.0; 3];
    let positions: &[f64] = if anisotropic {
        &[-0.375, -0.125, 0.125, 0.375]
    } else {
        &[0.0]
    };
    let mut major_uv = if axis_u >= axis_v { du } else { dv };
    let anisotropy_scale = (4.0 * minor / major).min(1.0);
    major_uv[0] *= anisotropy_scale;
    major_uv[1] *= anisotropy_scale;
    for position in positions {
        let low = bilinear(
            asset,
            lo,
            u + major_uv[0] * position,
            v + major_uv[1] * position,
        );
        let high = bilinear(
            asset,
            hi,
            u + major_uv[0] * position,
            v + major_uv[1] * position,
        );
        for channel in 0..asset.format.channels() {
            candidate[channel] +=
                (low[channel] * (1.0 - blend) + high[channel] * blend) / positions.len() as f64;
        }
    }
    // The derivative arithmetic selecting a mip is rounded outward by also
    // enclosing the immediate coarser/finer neighbors. This is intentionally
    // wider than the candidate's two-level filter and remains valid at exact
    // power-of-two and repeat-seam boundaries.
    let bound_lo = lo.saturating_sub(1);
    let bound_hi = (hi + 1).min(asset.mips.len() - 1);
    let mut channels = [F64Interval::point(0.0)?; 3];
    for channel in 0..asset.format.channels() {
        let decode = |value: u8| match asset.format {
            TextureFormat::Rgb8Srgb => f64::from(srgb_to_linear_q16(value)) / 65_535.0,
            TextureFormat::Rg8Snorm => (f64::from(value as i8) / 127.0).max(-1.0),
            TextureFormat::Rgb8Linear | TextureFormat::R8Linear => f64::from(value) / 255.0,
        };
        let min = (bound_lo..=bound_hi)
            .map(|level| decode(asset.mips[level].channel_min[channel]))
            .fold(f64::INFINITY, f64::min);
        let max = (bound_lo..=bound_hi)
            .map(|level| decode(asset.mips[level].channel_max[channel]))
            .fold(f64::NEG_INFINITY, f64::max);
        channels[channel] = F64Interval::new(min.min(max), min.max(max))?;
    }
    Ok((
        candidate,
        SampleInterval {
            channels,
            channel_count: asset.format.channels() as u8,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checker_mips_are_deterministic_and_reach_one_texel() {
        let first = compiler_asset(19).unwrap();
        let second = compiler_asset(19).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first
                .mips
                .iter()
                .map(|mip| (mip.width, mip.height))
                .collect::<Vec<_>>(),
            vec![(2, 2), (1, 1)]
        );
        assert_eq!(first.mips[1].bytes, vec![188, 188, 188]);
    }

    #[test]
    fn every_v1_format_has_a_sealed_independently_decodable_asset() {
        for (stable_id, format, channels) in [
            (19, TextureFormat::Rgb8Srgb, 3),
            (20, TextureFormat::Rgb8Linear, 3),
            (21, TextureFormat::Rg8Snorm, 2),
            (22, TextureFormat::R8Linear, 1),
            (23, TextureFormat::Rg8Snorm, 2),
        ] {
            let asset = compiler_asset(stable_id).unwrap();
            assert_eq!(asset.format, format);
            assert_eq!(asset.mips.last().unwrap().bytes.len(), channels);
            assert_eq!(
                asset.digest,
                wrela_machine::sha256::sha256_hex(&asset.identity_bytes())
            );
            let expected = match stable_id {
                19 => "c519fe04ffe1dadba6fec96f5b3bc190cf74e52a4be0718a76398ced9f97231b",
                20 => "0c72273818dc3668c904aef40595606bb8a4a178b70fca77571f4fc836f0f98f",
                21 => "935292783d93f9aa2cff29ca5ded6cf59c243804d91f92fc5a7ea055eee75826",
                22 => "a7eaae9a845e30a5e87d27c5184c133d55ec05fab268c7dfb08ff0160f01f8a7",
                23 => "e1b82b9eba13c9aa5cd8f0f7df75bc612ef87604d42051abacfcd630e927f5db",
                _ => unreachable!(),
            };
            assert_eq!(asset.digest, expected);
        }
    }

    #[test]
    fn sample_candidate_is_inside_certified_minmax() {
        let asset = compiler_asset(19).unwrap();
        let (candidate, interval) = sample(&asset, 0.99, 0.01, [0.4, 0.0], [0.0, 0.05]).unwrap();
        for (value, bound) in candidate.into_iter().zip(interval.channels) {
            assert!(bound.lo <= value && value <= bound.hi);
        }
    }

    #[test]
    fn certified_bounds_contain_an_independent_dense_footprint_oracle() {
        fn decode(format: TextureFormat, byte: u8) -> f64 {
            match format {
                TextureFormat::Rgb8Srgb => {
                    let encoded = f64::from(byte) / 255.0;
                    if encoded <= 0.040_45 {
                        encoded / 12.92
                    } else {
                        ((encoded + 0.055) / 1.055).powf(2.4)
                    }
                }
                TextureFormat::Rg8Snorm => (f64::from(byte as i8) / 127.0).max(-1.0),
                TextureFormat::Rgb8Linear | TextureFormat::R8Linear => f64::from(byte) / 255.0,
            }
        }

        fn dense_base_footprint(
            asset: &TextureAsset,
            u: f64,
            v: f64,
            du: [f64; 2],
            dv: [f64; 2],
        ) -> [f64; 3] {
            let mip = &asset.mips[0];
            let coordinate = |value: i64, size: u32, mode| match mode {
                WrapMode::Clamp => value.clamp(0, i64::from(size) - 1) as usize,
                WrapMode::Repeat => value.rem_euclid(i64::from(size)) as usize,
            };
            let map = |value: f64, mode| match mode {
                WrapMode::Clamp => value.clamp(0.0, 1.0),
                WrapMode::Repeat => value.rem_euclid(1.0),
            };
            let mut total = [0.0; 3];
            const GRID: usize = 128;
            for sample_y in 0..GRID {
                for sample_x in 0..GRID {
                    let footprint_u = (sample_x as f64 + 0.5) / GRID as f64 - 0.5;
                    let footprint_v = (sample_y as f64 + 0.5) / GRID as f64 - 0.5;
                    let sample_u = u + footprint_u * du[0] + footprint_v * dv[0];
                    let sample_v = v + footprint_u * du[1] + footprint_v * dv[1];
                    let x = map(sample_u, asset.wrap_u) * f64::from(mip.width) - 0.5;
                    let y = map(sample_v, asset.wrap_v) * f64::from(mip.height) - 0.5;
                    let x0 = x.floor() as i64;
                    let y0 = y.floor() as i64;
                    let fx = x - x.floor();
                    let fy = y - y.floor();
                    for (channel, output) in
                        total.iter_mut().enumerate().take(asset.format.channels())
                    {
                        let at = |texel_x: i64, texel_y: i64| {
                            let texel_x = coordinate(texel_x, mip.width, asset.wrap_u);
                            let texel_y = coordinate(texel_y, mip.height, asset.wrap_v);
                            decode(
                                asset.format,
                                mip.bytes[(texel_y * mip.width as usize + texel_x)
                                    * asset.format.channels()
                                    + channel],
                            )
                        };
                        let low = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
                        let high = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
                        *output += low * (1.0 - fy) + high * fy;
                    }
                }
            }
            total.map(|value| value / (GRID * GRID) as f64)
        }

        for (stable_id, u, v, du, dv) in [
            (19, 0.99, 0.01, [0.4, 0.0], [0.0, 0.05]),
            (20, -0.05, 1.05, [0.15, 0.02], [0.01, 0.1]),
            (21, 0.995, 0.25, [0.6, 0.0], [0.0, 0.1]),
            (22, 0.5, 0.5, [0.25, 0.0], [0.0, 0.25]),
        ] {
            let asset = compiler_asset(stable_id).unwrap();
            let (_, bounds) = sample(&asset, u, v, du, dv).unwrap();
            let oracle = dense_base_footprint(&asset, u, v, du, dv);
            for channel in 0..asset.format.channels() {
                // The oracle deliberately uses ordinary independent f64
                // interpolation. Its 16,384-sample accumulation can round a
                // convex combination a few ulps beyond an exact endpoint.
                // Bound that numerical noise by one epsilon per add instead
                // of weakening the certified interval itself.
                let oracle_rounding = (128 * 128) as f64 * f64::EPSILON;
                assert!(
                    bounds.channels[channel].lo - oracle_rounding <= oracle[channel]
                        && oracle[channel] <= bounds.channels[channel].hi + oracle_rounding,
                    "asset {stable_id} channel {channel}: oracle={} bounds={:?}",
                    oracle[channel],
                    bounds.channels[channel],
                );
            }
        }
    }

    #[test]
    fn repeat_crossing_is_an_explicit_uv_event() {
        assert_eq!(
            classify_uv_event(WrapMode::Repeat, WrapMode::Clamp, [0.99, 1.01], [0.2, 0.3],),
            Ok(UvEvent::RepeatSeamU)
        );
        assert_eq!(
            classify_uv_event(WrapMode::Repeat, WrapMode::Clamp, [0.2, 0.3], [0.2, 0.3],),
            Ok(UvEvent::Continuous)
        );
    }

    #[test]
    fn signed_slope_mips_and_bounds_use_signed_order() {
        let mips =
            build_mips(TextureFormat::Rg8Snorm, 2, 1, &[(-127_i8) as u8, 0, 127, 0]).unwrap();
        assert_eq!(mips[0].channel_min[0] as i8, -127);
        assert_eq!(mips[0].channel_max[0] as i8, 127);
        assert_eq!(mips[1].bytes[0] as i8, 0);
    }

    #[test]
    fn odd_mip_edges_average_only_actual_children() {
        let scalar = build_mips(TextureFormat::R8Linear, 3, 1, &[0, 100, 200]).unwrap();
        assert_eq!(scalar[1].bytes, vec![50, 200]);
        assert_eq!(scalar[2].bytes, vec![125]);

        let slopes = build_mips(TextureFormat::Rg8Snorm, 3, 1, &[0, 0, 0, 0, 127, 0]).unwrap();
        assert_eq!(slopes[1].bytes, vec![0, 0, 127, 0]);
        let moments = slopes[1].slope_moments.as_ref().unwrap();
        assert_eq!(moments[0], [0; 5]);
        assert_eq!(moments[1], [65_536, 0, 65_536, 0, 0]);
    }

    #[test]
    fn slope_moment_pyramid_preserves_local_second_moments() {
        let asset = compiler_asset(21).unwrap();
        let base = asset.mips[0].slope_moments.as_ref().unwrap();
        assert_eq!(base.len(), 4);
        assert_ne!(base[0], base[1]);
        for moments in base {
            assert!(moments[2] >= 0 && moments[4] >= 0);
            assert!(moments[2] * moments[4] >= moments[3] * moments[3]);
        }
        let coarse = asset.mips[1].slope_moments.as_ref().unwrap();
        assert_eq!(coarse.len(), 1);
        assert!(coarse[0][2] * coarse[0][4] >= coarse[0][3] * coarse[0][3]);
    }

    #[test]
    fn fine_slope_asset_preserves_negative_q16_moments_and_psd() {
        let asset = compiler_asset(23).unwrap();
        let base = asset.mips[0].slope_moments.as_ref().unwrap();
        assert!(base[0][0] < 0, "first signed slope must remain negative");
        assert_eq!(base[0][1], 0);
        for moments in base {
            assert!(moments[2] >= moments[0] * moments[0] / 65_536);
            assert!(moments[4] >= moments[1] * moments[1] / 65_536);
            assert!(moments[2] * moments[4] >= moments[3] * moments[3]);
        }
    }
}
