//! Versioned semantic validation for the shared FrameProgram v1 wire contract.
//!
//! This module depends only on pointer-free format records and machine
//! ceilings. Compiler analysis graphs and proof-stage types stay out of
//! `wrela-machine`.

use super::{FrameProgramModelV1, FrameProgramTableKindV1, FrameRecordV1};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedQPolicyV1 {
    pub exponent: i16,
    pub maximum_raw: i32,
    pub reset_width: u8,
    pub error_radius: i32,
}

/// Recompute the canonical v1 fixed-q family policy from the camera depth
/// envelope. This is shared by the compiler and decoder so a rehashed program
/// cannot weaken the certified radius or reset width.
pub fn derive_fixed_q_policy_v1(near: f64, far: f64) -> Option<FixedQPolicyV1> {
    if !near.is_finite() || !far.is_finite() || near <= 0.0 || near >= far {
        return None;
    }
    let q_lo = 1.0 / far;
    let q_hi = 1.0 / near;
    let span = q_hi - q_lo;
    let model_error = q_hi * f64::from(f32::EPSILON) * 8.0 + f64::EPSILON;
    derive_fixed_q_envelope_v1(q_lo, q_hi, span, 2.0 * span, 64, model_error)
}

pub fn derive_fixed_q_envelope_v1(
    q_lo: f64,
    q_hi: f64,
    dq_abs: f64,
    ddq_abs: f64,
    requested_width: u8,
    model_error: f64,
) -> Option<FixedQPolicyV1> {
    if !q_lo.is_finite()
        || !q_hi.is_finite()
        || !dq_abs.is_finite()
        || !ddq_abs.is_finite()
        || !model_error.is_finite()
        || q_lo > q_hi
        || dq_abs < 0.0
        || ddq_abs < 0.0
        || model_error < 0.0
        || requested_width == 0
    {
        return None;
    }
    let preferred_width = requested_width.min(32);
    let mut best = None;
    for exponent in -96_i16..=63 {
        let Some(q) = quantize_interval(q_lo, q_hi, exponent) else {
            continue;
        };
        let Some(dq) = quantize_interval(-dq_abs, dq_abs, exponent) else {
            continue;
        };
        let Some(ddq) = quantize_interval(-ddq_abs, ddq_abs, exponent) else {
            continue;
        };
        let scale = 2.0_f64.powi(i32::from(exponent));
        let model_radius = (model_error / scale).ceil();
        if model_radius > f64::from(i32::MAX) {
            continue;
        }
        let maximum_width = requested_width.min(64);
        let mut width = 32_u8.min(maximum_width).next_power_of_two();
        if width > maximum_width {
            width /= 2;
        }
        loop {
            if recurrence_fits(q, dq, ddq, width) {
                let Some(q_radius) = interval_radius(q) else {
                    break;
                };
                let Some(dq_radius) = interval_radius(dq) else {
                    break;
                };
                let Some(ddq_radius) = interval_radius(ddq) else {
                    break;
                };
                let triangular = i64::from(width) * i64::from(width.saturating_sub(1)) / 2;
                let Some(error_radius) = i64::from(q_radius)
                    .checked_add(i64::from(dq_radius) * i64::from(width))
                    .and_then(|value| value.checked_add(i64::from(ddq_radius) * triangular))
                    .and_then(|value| value.checked_add(model_radius as i64))
                else {
                    break;
                };
                let Some(maximum_raw) = recurrence_maximum(q, dq, ddq, width) else {
                    break;
                };
                let Ok(error_radius) = i32::try_from(error_radius) else {
                    break;
                };
                let policy = FixedQPolicyV1 {
                    exponent,
                    maximum_raw,
                    reset_width: width,
                    error_radius,
                };
                if width == preferred_width {
                    return Some(policy);
                }
                if best.is_none_or(|prior: FixedQPolicyV1| width > prior.reset_width) {
                    best = Some(policy);
                }
                break;
            }
            width /= 2;
            if width == 0 {
                break;
            }
        }
    }
    best
}

fn quantize_interval(lo: f64, hi: f64, exponent: i16) -> Option<(i32, i32)> {
    let scale = 2.0_f64.powi(i32::from(exponent));
    let lo = (lo / scale).floor();
    let hi = (hi / scale).ceil();
    if lo < f64::from(i32::MIN) || hi > f64::from(i32::MAX) {
        return None;
    }
    Some((lo as i32, hi as i32))
}

fn interval_radius((lo, hi): (i32, i32)) -> Option<i32> {
    let width = i64::from(hi).checked_sub(i64::from(lo))?;
    i32::try_from(width / 2 + width % 2).ok()
}

fn recurrence_fits(q: (i32, i32), dq: (i32, i32), ddq: (i32, i32), width: u8) -> bool {
    recurrence_corners(q, dq, ddq).all(|(q0, d1, d2)| {
        let mut q = i64::from(q0);
        let mut dq = i64::from(d1);
        for _ in 0..width {
            q += dq;
            dq += i64::from(d2);
            if q < i64::from(i32::MIN)
                || q > i64::from(i32::MAX)
                || dq < i64::from(i32::MIN)
                || dq > i64::from(i32::MAX)
            {
                return false;
            }
        }
        true
    })
}

fn recurrence_maximum(q: (i32, i32), dq: (i32, i32), ddq: (i32, i32), width: u8) -> Option<i32> {
    let mut maximum = 0_i64;
    for (q0, d1, d2) in recurrence_corners(q, dq, ddq) {
        let mut q = i64::from(q0);
        let mut dq = i64::from(d1);
        maximum = maximum.max(q.abs()).max(dq.abs()).max(i64::from(d2).abs());
        for _ in 0..width {
            q = q.checked_add(dq)?;
            dq = dq.checked_add(i64::from(d2))?;
            maximum = maximum.max(q.abs()).max(dq.abs());
        }
    }
    i32::try_from(maximum).ok()
}

fn recurrence_corners(
    q: (i32, i32),
    dq: (i32, i32),
    ddq: (i32, i32),
) -> impl Iterator<Item = (i32, i32, i32)> {
    [
        (q.0, dq.0, ddq.0),
        (q.0, dq.0, ddq.1),
        (q.0, dq.1, ddq.0),
        (q.0, dq.1, ddq.1),
        (q.1, dq.0, ddq.0),
        (q.1, dq.0, ddq.1),
        (q.1, dq.1, ddq.0),
        (q.1, dq.1, ddq.1),
    ]
    .into_iter()
}

fn exact_operands(
    kind: FrameProgramTableKindV1,
    record: &FrameRecordV1,
    expected: usize,
) -> Result<(), String> {
    if record.operands.len() != expected {
        return Err(format!(
            "pixels::verify: {} record {} opcode {} has {} operands, expected {expected}",
            kind.stable_name(),
            record.stable_id,
            record.tag,
            record.operands.len()
        ));
    }
    Ok(())
}

fn verify_operand_numeric_domains(
    kind: FrameProgramTableKindV1,
    record: &FrameRecordV1,
) -> Result<(), String> {
    if kind == FrameProgramTableKindV1::Scalar && matches!(record.tag, 1 | 2) {
        let encoded = record
            .operands
            .get(1)
            .copied()
            .ok_or_else(|| "pixels::verify: scalar constant is truncated".to_string())?;
        let finite = if record.tag == 1 {
            u32::try_from(encoded)
                .ok()
                .map(f32::from_bits)
                .is_some_and(f32::is_finite)
        } else {
            f64::from_bits(encoded).is_finite()
        };
        if !finite {
            return Err(format!(
                "pixels::verify: scalar {} has an invalid numeric encoding",
                record.stable_id
            ));
        }
    }
    Ok(())
}

fn verify_camera_light_post_numeric_domains(record: &FrameRecordV1) -> Result<(), String> {
    if record.operands.len() < 44 {
        return Err("pixels::verify: camera/light/post header is truncated".to_string());
    }
    let f32_at = |index: usize, label: &str| -> Result<f32, String> {
        let value = record
            .operands
            .get(index)
            .copied()
            .ok_or_else(|| format!("pixels::verify: camera/light/post {label} is truncated"))?;
        let bits = u32::try_from(value)
            .map_err(|_| format!("pixels::verify: camera/light/post {label} exceeds 32 bits"))?;
        let value = f32::from_bits(bits);
        if !value.is_finite() {
            return Err(format!(
                "pixels::verify: camera/light/post {label} is non-finite"
            ));
        }
        Ok(value)
    };
    let near = f64::from_bits(record.operands[4]);
    let far = f64::from_bits(record.operands[5]);
    let motion = f32_at(6, "camera motion")?;
    if !near.is_finite() || !far.is_finite() || near <= 0.0 || near >= far || motion < 0.0 {
        return Err(
            "pixels::verify: camera/light/post has invalid near/far/motion bounds".to_string(),
        );
    }
    let exposure_min = f32_at(16, "exposure minimum")?;
    let exposure_max = f32_at(17, "exposure maximum")?;
    if exposure_min > exposure_max {
        return Err("pixels::verify: camera/light/post has a reversed exposure range".to_string());
    }
    let expected_tone_len = match record.operands[24] {
        0 => super::LINEAR_TONE_LUT_V1.len(),
        1 => super::TRANSFER_TABLE_ENTRIES_V1,
        _ => 0,
    };
    if record.operands[25] != expected_tone_len as u64
        || record.operands[26] != 1
        || record.operands[27] != super::TRANSFER_TABLE_ENTRIES_V1 as u64
        || record.operands[28] != (-16_i64) as u64
        || record.operands[29] != 16
        || record.operands[30] != 1
        || record.operands[31] != 1
    {
        return Err(
            "pixels::verify: camera/light/post has an invalid sealed LUT reference".to_string(),
        );
    }
    let mut values = [0.0_f32; 12];
    for (target, index) in values.iter_mut().zip(32..44) {
        *target = f32_at(index, "world/environment bound")?;
    }
    for (label, lo, hi) in [
        ("world x", values[0], values[3]),
        ("world y", values[1], values[4]),
        ("world z", values[2], values[5]),
        ("environment r", values[6], values[9]),
        ("environment g", values[7], values[10]),
        ("environment b", values[8], values[11]),
    ] {
        if lo > hi {
            return Err(format!(
                "pixels::verify: camera/light/post has a reversed {label} range"
            ));
        }
    }
    Ok(())
}

fn verify_transform_operands(operands: &[u64], scalar_count: Option<usize>) -> Result<(), String> {
    let mut pending = vec![(operands, 0_usize)];
    while let Some((current, depth)) = pending.pop() {
        if depth > super::FRAME_PROGRAM_TRANSFORM_DEPTH_MAX_V1 {
            return Err(format!(
                "pixels::verify: field transform nesting exceeds {}",
                super::FRAME_PROGRAM_TRANSFORM_DEPTH_MAX_V1
            ));
        }
        let Some((&tag, rest)) = current.split_first() else {
            return Err("pixels::verify: field transform is truncated".to_string());
        };
        let scalar_operands = match tag {
            1 if rest.len() == 3 => Some(rest),
            2 if rest.len() == 9 => Some(rest),
            3 if rest.len() == 12 => Some(rest),
            4 if rest.len() == 1 => Some(rest),
            1 => {
                return Err("pixels::verify: translation transform has wrong arity".to_string());
            }
            2 => return Err("pixels::verify: rotation transform has wrong arity".to_string()),
            3 => return Err("pixels::verify: rigid transform has wrong arity".to_string()),
            4 => {
                return Err("pixels::verify: uniform-scale transform has wrong arity".to_string());
            }
            5 | 6 => {
                let Some((&count, mut tail)) = rest.split_first() else {
                    return Err("pixels::verify: transform sequence is truncated".to_string());
                };
                let count = usize::try_from(count).map_err(|_| {
                    "pixels::verify: transform sequence count exceeds usize".to_string()
                })?;
                let mut children = Vec::with_capacity(count.saturating_add(1));
                for _ in 0..count {
                    let Some((&len, after_len)) = tail.split_first() else {
                        return Err(
                            "pixels::verify: transform sequence step is truncated".to_string()
                        );
                    };
                    let len = usize::try_from(len).map_err(|_| {
                        "pixels::verify: transform sequence step length exceeds usize".to_string()
                    })?;
                    let (step, after) = after_len.split_at_checked(len).ok_or_else(|| {
                        "pixels::verify: transform sequence step exceeds its record".to_string()
                    })?;
                    children.push(step);
                    tail = after;
                }
                let Some((&len, after_len)) = tail.split_first() else {
                    return Err("pixels::verify: composed transform is truncated".to_string());
                };
                let len = usize::try_from(len).map_err(|_| {
                    "pixels::verify: composed transform length exceeds usize".to_string()
                })?;
                let (composed, after) = after_len.split_at_checked(len).ok_or_else(|| {
                    "pixels::verify: composed transform exceeds its record".to_string()
                })?;
                if !after.is_empty() {
                    return Err(
                        "pixels::verify: transform sequence has trailing operands".to_string()
                    );
                }
                children.push(composed);
                for child in children.into_iter().rev() {
                    pending.push((child, depth + 1));
                }
                None
            }
            _ => {
                return Err(format!(
                    "pixels::verify: field transform has unknown opcode {tag}"
                ));
            }
        };
        if let (Some(scalars), Some(count)) = (scalar_operands, scalar_count) {
            if let Some(scalar) = scalars.iter().find(|scalar| **scalar >= count as u64) {
                return Err(format!(
                    "pixels::verify: field transform names scalar {scalar} outside {count}"
                ));
            }
        }
    }
    Ok(())
}

struct OperandCursor<'a> {
    context: String,
    operands: &'a [u64],
    at: usize,
}

impl<'a> OperandCursor<'a> {
    fn new(kind: FrameProgramTableKindV1, record: &'a FrameRecordV1) -> Self {
        Self {
            context: format!(
                "{} record {} opcode {}",
                kind.stable_name(),
                record.stable_id,
                record.tag
            ),
            operands: &record.operands,
            at: 0,
        }
    }

    fn take(&mut self, label: &str) -> Result<u64, String> {
        let value =
            self.operands.get(self.at).copied().ok_or_else(|| {
                format!("pixels::verify: {} is truncated at {label}", self.context)
            })?;
        self.at += 1;
        Ok(value)
    }

    fn count(&mut self, label: &str) -> Result<usize, String> {
        usize::try_from(self.take(label)?)
            .map_err(|_| format!("pixels::verify: {} {label} exceeds usize", self.context))
    }

    fn skip(&mut self, count: usize, label: &str) -> Result<(), String> {
        self.at = self
            .at
            .checked_add(count)
            .ok_or_else(|| format!("pixels::verify: {} {label} count overflows", self.context))?;
        if self.at > self.operands.len() {
            return Err(format!(
                "pixels::verify: {} is truncated in {label}",
                self.context
            ));
        }
        Ok(())
    }

    fn boolean(&mut self, label: &str) -> Result<(), String> {
        let value = self.take(label)?;
        if value > 1 {
            return Err(format!(
                "pixels::verify: {} has non-boolean {label} {value}",
                self.context
            ));
        }
        Ok(())
    }

    fn enum_tag(
        &mut self,
        label: &str,
        range: std::ops::RangeInclusive<u64>,
    ) -> Result<(), String> {
        let value = self.take(label)?;
        if !range.contains(&value) {
            return Err(format!(
                "pixels::verify: {} has unknown {label} {value}",
                self.context
            ));
        }
        Ok(())
    }

    fn finish(self) -> Result<(), String> {
        if self.at != self.operands.len() {
            return Err(format!(
                "pixels::verify: {} has {} trailing operands",
                self.context,
                self.operands.len() - self.at
            ));
        }
        Ok(())
    }
}

fn verify_scalar_derivative_shape(cursor: &mut OperandCursor<'_>) -> Result<(), String> {
    let sources = cursor.count("scalar derivative source count")?;
    cursor.skip(sources, "scalar derivative sources")?;
    cursor.skip(5, "scalar derivative world bounds")?;
    let parameters = cursor.count("scalar derivative parameter count")?;
    cursor.skip(
        parameters.checked_mul(2).ok_or_else(|| {
            "pixels::verify: scalar derivative parameter count overflow".to_string()
        })?,
        "scalar derivative parameters",
    )?;
    cursor.boolean("scalar derivative frame-delta presence")?;
    cursor.skip(1, "scalar derivative frame-delta bound")?;
    cursor.boolean("scalar derivative second-frame-delta presence")?;
    cursor.skip(1, "scalar derivative second-frame-delta bound")?;
    Ok(())
}

fn verify_event_representation_shape(
    tag: u16,
    operands: &[u64],
    context: &FrameRecordV1,
) -> Result<(), String> {
    let synthetic = FrameRecordV1 {
        stable_id: context.stable_id,
        tag,
        flags: 0,
        operands: operands.to_vec(),
    };
    let mut cursor = OperandCursor::new(FrameProgramTableKindV1::Event, &synthetic);
    match tag {
        1 | 2 => cursor.skip(2, "linear/quadratic representation")?,
        3 | 10 => cursor.skip(1, "predicate representation")?,
        4 => {
            cursor.skip(3, "deformation roots")?;
            let derivative_len = cursor.count("scalar derivative payload length")?;
            let derivative_end = cursor.at.checked_add(derivative_len).ok_or_else(|| {
                "pixels::verify: scalar derivative payload length overflow".to_string()
            })?;
            if derivative_end > cursor.operands.len() {
                return Err("pixels::verify: scalar derivative payload is truncated".to_string());
            }
            let derivative = &cursor.operands[cursor.at..derivative_end];
            let mut nested = OperandCursor {
                context: cursor.context.clone(),
                operands: derivative,
                at: 0,
            };
            verify_scalar_derivative_shape(&mut nested)?;
            nested.finish()?;
            cursor.at = derivative_end;
            let phase_len = cursor.count("phase recurrence payload length")?;
            if phase_len != 21 {
                return Err(format!(
                    "pixels::verify: event record {} has phase recurrence length {phase_len}, expected 21",
                    context.stable_id
                ));
            }
            cursor.skip(phase_len, "phase recurrence payload")?;
            cursor.skip(4, "deformation Taylor contract")?;
        }
        5 => cursor.skip(14, "torus oracle")?,
        6 | 7 | 8 => {
            cursor.skip(
                match tag {
                    6 => 5,
                    7 => 4,
                    _ => 3,
                },
                "Taylor predicate roots",
            )?;
            let derivative_len = cursor.count("scalar derivative payload length")?;
            let derivative_end = cursor.at.checked_add(derivative_len).ok_or_else(|| {
                "pixels::verify: scalar derivative payload length overflow".to_string()
            })?;
            if derivative_end > cursor.operands.len() {
                return Err("pixels::verify: scalar derivative payload is truncated".to_string());
            }
            let derivative = &cursor.operands[cursor.at..derivative_end];
            let mut nested = OperandCursor {
                context: cursor.context.clone(),
                operands: derivative,
                at: 0,
            };
            verify_scalar_derivative_shape(&mut nested)?;
            nested.finish()?;
            cursor.at = derivative_end;
            cursor.skip(3, "Taylor predicate contract")?;
            if tag == 6 {
                let left_negated = operands.get(2).copied().unwrap_or(2);
                let right_negated = operands.get(3).copied().unwrap_or(2);
                if left_negated > 1 || right_negated > 1 {
                    return Err(
                        "pixels::verify: smooth-band event has non-boolean negation".to_string()
                    );
                }
            } else if tag == 7 {
                let left_negated = operands.get(2).copied().unwrap_or(2);
                let right_negated = operands.get(3).copied().unwrap_or(2);
                if left_negated > 1 || right_negated > 1 {
                    return Err(
                        "pixels::verify: smooth-tie event has non-boolean negation".to_string()
                    );
                }
            } else if !(1..=6).contains(&operands.get(2).copied().unwrap_or(0)) {
                return Err("pixels::verify: material event has unknown comparison".to_string());
            }
        }
        9 => {
            cursor.enum_tag("repeat axis", 1..=3)?;
            cursor.skip(2, "repeat boundary")?;
        }
        11 => {
            cursor.boolean("projected-boundary orientation")?;
            cursor.skip(1, "projected-boundary coordinate")?;
        }
        12 => {}
        13 => {
            cursor.skip(1, "depth numerator")?;
            for denominator in ["first denominator", "second denominator"] {
                cursor.skip(3, denominator)?;
                cursor.enum_tag("strict sign", 1..=2)?;
            }
        }
        14 => {
            cursor.skip(23, "Taylor depth derivatives")?;
            cursor.skip(8, "Taylor depth fallback bounds")?;
            cursor.boolean("Taylor depth strict-g-q requirement")?;
            cursor.boolean("Taylor depth fallback discard")?;
        }
        _ => unreachable!("event opcode checked before shape verification"),
    }
    cursor.finish()
}

fn verify_composition_shape(operands: &[u64], record: &FrameRecordV1) -> Result<(), String> {
    let synthetic = FrameRecordV1 {
        stable_id: record.stable_id,
        tag: record.tag,
        flags: 0,
        operands: operands.to_vec(),
    };
    let mut cursor = OperandCursor::new(FrameProgramTableKindV1::Feature, &synthetic);
    match cursor.take("composition opcode")? {
        1 => {
            cursor.skip(8, "specialized composition header")?;
            let order = cursor.count("composition coefficient-order count")?;
            cursor.skip(order, "composition coefficient order")?;
            let steps = cursor.count("composition step count")?;
            cursor.skip(
                steps
                    .checked_mul(5)
                    .ok_or_else(|| "pixels::verify: composition step count overflow".to_string())?,
                "composition steps",
            )?;
            let faces = cursor.count("composition correction-face count")?;
            for _ in 0..faces {
                cursor.enum_tag("composition correction sign", 1..=2)?;
                let order = cursor.count("correction coefficient-order count")?;
                cursor.skip(order, "correction coefficient order")?;
                let steps = cursor.count("correction step count")?;
                cursor.skip(
                    steps.checked_mul(5).ok_or_else(|| {
                        "pixels::verify: correction step count overflow".to_string()
                    })?,
                    "correction steps",
                )?;
            }
        }
        2 => cursor.skip(5, "interval-Taylor composition")?,
        opcode => {
            return Err(format!(
                "pixels::verify: feature record {} has unknown composition opcode {opcode}",
                record.stable_id
            ));
        }
    }
    cursor.finish()
}

fn texture_div_round_nearest(value: i64, denominator: i64) -> i64 {
    let magnitude = value.unsigned_abs();
    let denominator = denominator as u64;
    let rounded = (magnitude + denominator / 2) / denominator;
    if value < 0 {
        -(rounded as i64)
    } else {
        rounded as i64
    }
}

fn texture_slope_q16(byte: u8) -> i64 {
    let value = i64::from(byte as i8);
    if value <= -127 {
        -65_536
    } else {
        texture_div_round_nearest(value * 65_536, 127)
    }
}

fn texture_base_moments(bytes: &[u8]) -> Vec<[i64; 5]> {
    bytes
        .chunks_exact(2)
        .map(|texel| {
            let sx = texture_slope_q16(texel[0]);
            let sy = texture_slope_q16(texel[1]);
            [
                sx,
                sy,
                texture_div_round_nearest(sx * sx, 65_536),
                texture_div_round_nearest(sx * sy, 65_536),
                texture_div_round_nearest(sy * sy, 65_536),
            ]
        })
        .collect()
}

fn texture_downsample_moments(current: &[[i64; 5]], width: u32, height: u32) -> Vec<[i64; 5]> {
    let next_width = width.div_ceil(2);
    let next_height = height.div_ceil(2);
    let mut next = vec![[0_i64; 5]; next_width as usize * next_height as usize];
    for y in 0..next_height {
        for x in 0..next_width {
            let mut sums = [0_i64; 5];
            let mut count = 0_i64;
            for dy in 0..2 {
                for dx in 0..2 {
                    let sx = x * 2 + dx;
                    let sy = y * 2 + dy;
                    if sx >= width || sy >= height {
                        continue;
                    }
                    let source = current[sy as usize * width as usize + sx as usize];
                    for moment in 0..5 {
                        sums[moment] += source[moment];
                    }
                    count += 1;
                }
            }
            next[y as usize * next_width as usize + x as usize] =
                sums.map(|sum| texture_div_round_nearest(sum, count));
        }
    }
    next
}

fn verify_texture_shape(record: &FrameRecordV1) -> Result<(), String> {
    let mut cursor = OperandCursor::new(FrameProgramTableKindV1::Texture, record);
    let stable_id = u32::try_from(cursor.take("texture stable ID")?)
        .map_err(|_| "pixels::verify: texture stable ID exceeds u32".to_string())?;
    let format = cursor.take("texture format")?;
    let channels = match format {
        1 | 2 => 3_usize,
        3 => 2,
        4 => 1,
        _ => {
            return Err(format!(
                "pixels::verify: texture {stable_id} has unknown format {format}"
            ));
        }
    };
    let width = u32::try_from(cursor.take("texture width")?)
        .map_err(|_| "pixels::verify: texture width exceeds u32".to_string())?;
    let height = u32::try_from(cursor.take("texture height")?)
        .map_err(|_| "pixels::verify: texture height exceeds u32".to_string())?;
    if width == 0 || height == 0 {
        return Err(format!(
            "pixels::verify: texture {stable_id} has an empty extent"
        ));
    }
    let wrap_u = cursor.take("texture u wrap")?;
    let wrap_v = cursor.take("texture v wrap")?;
    if wrap_u > 1 || wrap_v > 1 {
        return Err(format!(
            "pixels::verify: texture {stable_id} has invalid wrap tags"
        ));
    }
    let mip_count = cursor.count("texture mip count")?;
    if mip_count == 0 {
        return Err(format!(
            "pixels::verify: texture {stable_id} has no mip levels"
        ));
    }
    let mut sealed_digest = [0_u8; 32];
    for chunk in sealed_digest.chunks_exact_mut(8) {
        chunk.copy_from_slice(&cursor.take("texture digest")?.to_le_bytes());
    }
    const SEALED_ASSETS: [(u32, u64, u64, u64, [u8; 32]); 5] = [
        (
            19,
            1,
            1,
            1,
            [
                0xc5, 0x19, 0xfe, 0x04, 0xff, 0xe1, 0xda, 0xdb, 0xa6, 0xfe, 0xc9, 0x6f, 0x5b, 0x3b,
                0xc1, 0x90, 0xcf, 0x74, 0xe5, 0x2a, 0x4b, 0xe0, 0x71, 0x8a, 0x76, 0x39, 0x8c, 0xed,
                0x9f, 0x97, 0x23, 0x1b,
            ],
        ),
        (
            20,
            2,
            0,
            0,
            [
                0x0c, 0x72, 0x27, 0x38, 0x18, 0xdc, 0x36, 0x68, 0xc9, 0x04, 0xae, 0xf4, 0x05, 0x95,
                0x60, 0x6b, 0xb8, 0xa4, 0xa1, 0x78, 0xb7, 0x0f, 0xca, 0x77, 0x57, 0x1f, 0x4f, 0xc8,
                0x36, 0xf0, 0xf9, 0x8f,
            ],
        ),
        (
            21,
            3,
            1,
            1,
            [
                0x93, 0x52, 0x92, 0x78, 0x3d, 0x93, 0xf9, 0xaa, 0x2c, 0xff, 0x29, 0xca, 0x5d, 0xed,
                0x6c, 0xf5, 0x9c, 0x24, 0x38, 0x04, 0xd9, 0x1f, 0x92, 0xfc, 0x5a, 0x7e, 0xa0, 0x55,
                0xee, 0xe7, 0x58, 0x26,
            ],
        ),
        (
            22,
            4,
            0,
            0,
            [
                0xa7, 0xea, 0xae, 0x9a, 0x84, 0x5e, 0x30, 0xa5, 0xe8, 0x7d, 0x27, 0xc5, 0x18, 0x4c,
                0x13, 0x3d, 0x55, 0xec, 0x05, 0xfa, 0xb2, 0x68, 0xc7, 0xdf, 0xb0, 0x8f, 0xf0, 0x16,
                0x0f, 0x01, 0xf8, 0xa7,
            ],
        ),
        (
            23,
            3,
            1,
            1,
            [
                0xe1, 0xb8, 0x2b, 0x9e, 0xba, 0x13, 0xc9, 0xaa, 0x5c, 0xd8, 0xf0, 0xf7, 0xdf, 0x75,
                0xbc, 0x61, 0x2e, 0xf8, 0x76, 0x04, 0xd4, 0x20, 0x51, 0xab, 0xac, 0xfc, 0xd6, 0x30,
                0xe9, 0x27, 0xf5, 0xdb,
            ],
        ),
    ];
    let sealed = SEALED_ASSETS.iter().find(|entry| entry.0 == stable_id);
    if width != 2
        || height != 2
        || !sealed.is_some_and(|entry| {
            (format, wrap_u, wrap_v, sealed_digest) == (entry.1, entry.2, entry.3, entry.4)
        })
    {
        return Err(format!(
            "pixels::verify: texture {stable_id} is not a sealed compiler-owned v1 asset"
        ));
    }
    let mut identity = Vec::new();
    identity.extend_from_slice(b"wrela-texture-v1\0");
    identity.extend_from_slice(&stable_id.to_le_bytes());
    identity.extend_from_slice(&format.to_le_bytes());
    identity.extend([wrap_u as u8, wrap_v as u8]);
    let (mut expected_width, mut expected_height) = (width, height);
    let mut expected_slope_moments: Option<Vec<[i64; 5]>> = None;
    for level in 0..mip_count {
        let mip_width = u32::try_from(cursor.take("mip width")?)
            .map_err(|_| "pixels::verify: mip width exceeds u32".to_string())?;
        let mip_height = u32::try_from(cursor.take("mip height")?)
            .map_err(|_| "pixels::verify: mip height exceeds u32".to_string())?;
        if mip_width != expected_width || mip_height != expected_height {
            return Err(format!(
                "pixels::verify: texture {stable_id} mip {level} has noncanonical dimensions"
            ));
        }
        let byte_count = cursor.count("mip byte count")?;
        let expected_bytes = usize::try_from(mip_width)
            .ok()
            .and_then(|w| {
                usize::try_from(mip_height)
                    .ok()
                    .and_then(|h| w.checked_mul(h))
            })
            .and_then(|pixels| pixels.checked_mul(channels))
            .ok_or_else(|| "pixels::verify: texture mip byte count overflow".to_string())?;
        if byte_count != expected_bytes {
            return Err(format!(
                "pixels::verify: texture {stable_id} mip {level} has {byte_count} bytes, expected {expected_bytes}"
            ));
        }
        identity.extend_from_slice(&mip_width.to_le_bytes());
        identity.extend_from_slice(&mip_height.to_le_bytes());
        let mut bytes = Vec::with_capacity(byte_count);
        for _ in 0..byte_count {
            let byte = u8::try_from(cursor.take("mip byte")?)
                .map_err(|_| "pixels::verify: texture mip byte exceeds u8".to_string())?;
            bytes.push(byte);
            identity.push(byte);
        }
        if cursor.count("mip channel count")? != channels {
            return Err(format!(
                "pixels::verify: texture {stable_id} mip {level} has the wrong channel count"
            ));
        }
        let mut minimum = vec![u8::MAX; channels];
        let mut maximum = vec![u8::MIN; channels];
        for texel in bytes.chunks_exact(channels) {
            for channel in 0..channels {
                if format == 3 {
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
        for expected in minimum.into_iter().chain(maximum) {
            let found = u8::try_from(cursor.take("mip channel bound")?)
                .map_err(|_| "pixels::verify: texture channel bound exceeds u8".to_string())?;
            if found != expected {
                return Err(format!(
                    "pixels::verify: texture {stable_id} mip {level} has an invalid channel bound"
                ));
            }
            identity.push(found);
        }
        let has_moments = cursor.take("mip slope moments")?;
        if has_moments > 1 || (has_moments == 1) != (format == 3) {
            return Err(format!(
                "pixels::verify: texture {stable_id} mip {level} has invalid slope-moment presence"
            ));
        }
        if has_moments == 1 {
            let expected = expected_slope_moments
                .take()
                .unwrap_or_else(|| texture_base_moments(&bytes));
            if cursor.count("mip slope moment texel count")? != expected.len() {
                return Err(format!(
                    "pixels::verify: texture {stable_id} mip {level} has the wrong slope-moment texel count"
                ));
            }
            identity.extend_from_slice(&(expected.len() as u64).to_le_bytes());
            for texel in &expected {
                for expected_moment in texel {
                    let encoded = cursor.take("mip slope moment")?;
                    if encoded as i64 != *expected_moment {
                        return Err(format!(
                            "pixels::verify: texture {stable_id} mip {level} has an invalid slope moment"
                        ));
                    }
                    identity.extend_from_slice(&expected_moment.to_le_bytes());
                }
            }
            expected_slope_moments =
                Some(texture_downsample_moments(&expected, mip_width, mip_height));
        }
        expected_width = expected_width.div_ceil(2);
        expected_height = expected_height.div_ceil(2);
    }
    cursor.finish()?;
    if expected_width != 1 || expected_height != 1 {
        return Err(format!(
            "pixels::verify: texture {stable_id} mip chain is incomplete"
        ));
    }
    if crate::sha256::sha256(&identity) != sealed_digest {
        return Err(format!(
            "pixels::verify: texture {stable_id} digest mismatch"
        ));
    }
    Ok(())
}

fn verify_secondary_bvh_shape(record: &FrameRecordV1) -> Result<(), String> {
    #[derive(Clone, Copy, PartialEq)]
    struct Bounds {
        min: [f64; 3],
        max: [f64; 3],
    }
    impl Bounds {
        fn valid(self) -> bool {
            self.min.into_iter().chain(self.max).all(f64::is_finite)
                && (0..3).all(|axis| self.min[axis] <= self.max[axis])
        }
        fn union(self, other: Self) -> Self {
            Self {
                min: std::array::from_fn(|axis| self.min[axis].min(other.min[axis])),
                max: std::array::from_fn(|axis| self.max[axis].max(other.max[axis])),
            }
        }
    }
    #[derive(Clone, Copy)]
    struct Node {
        bounds: Bounds,
        first: usize,
        count: usize,
        left: Option<usize>,
        right: Option<usize>,
    }
    let mut cursor = OperandCursor::new(FrameProgramTableKindV1::ShadingSummary, record);
    let object_count = cursor.count("secondary object count")?;
    let node_count = cursor.count("secondary node count")?;
    let root = cursor.count("secondary root")?;
    let stack_capacity = cursor.count("secondary stack capacity")?;
    if object_count == 0 || node_count == 0 || root != 0 || stack_capacity == 0 {
        return Err("pixels::verify: secondary BVH has an invalid header".to_string());
    }
    let read_bounds = |cursor: &mut OperandCursor<'_>| -> Result<Bounds, String> {
        let mut values = [0.0; 6];
        for value in &mut values {
            *value = f64::from_bits(cursor.take("secondary bound")?);
        }
        let bounds = Bounds {
            min: [values[0], values[1], values[2]],
            max: [values[3], values[4], values[5]],
        };
        if !bounds.valid() {
            return Err("pixels::verify: secondary BVH has an invalid bound".to_string());
        }
        Ok(bounds)
    };
    let mut objects = Vec::with_capacity(object_count);
    let mut object_ids = std::collections::BTreeSet::new();
    for _ in 0..object_count {
        let object = u32::try_from(cursor.take("secondary object ID")?)
            .map_err(|_| "pixels::verify: secondary object ID exceeds u32".to_string())?;
        let feature_first = u32::try_from(cursor.take("secondary feature first")?)
            .map_err(|_| "pixels::verify: secondary feature ID exceeds u32".to_string())?;
        let feature_count = u32::try_from(cursor.take("secondary feature count")?)
            .map_err(|_| "pixels::verify: secondary feature count exceeds u32".to_string())?;
        if !object_ids.insert(object)
            || feature_count == 0
            || feature_first.checked_add(feature_count).is_none()
        {
            return Err("pixels::verify: secondary BVH has an invalid object range".to_string());
        }
        objects.push(read_bounds(&mut cursor)?);
    }
    let decode_child = |value: u64| -> Result<Option<usize>, String> {
        if value == u64::MAX {
            Ok(None)
        } else {
            usize::try_from(value)
                .map(Some)
                .map_err(|_| "pixels::verify: secondary child exceeds usize".to_string())
        }
    };
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let bounds = read_bounds(&mut cursor)?;
        let first = cursor.count("secondary leaf first")?;
        let count = cursor.count("secondary leaf count")?;
        let left = decode_child(cursor.take("secondary left child")?)?;
        let right = decode_child(cursor.take("secondary right child")?)?;
        nodes.push(Node {
            bounds,
            first,
            count,
            left,
            right,
        });
    }
    cursor.finish()?;
    let mut reached = vec![false; node_count];
    let mut pending = vec![root];
    while let Some(index) = pending.pop() {
        if index >= node_count || std::mem::replace(&mut reached[index], true) {
            return Err("pixels::verify: secondary BVH is cyclic or aliases a child".to_string());
        }
        let node = nodes[index];
        match (node.left, node.right) {
            (None, None) => {
                let end = node
                    .first
                    .checked_add(node.count)
                    .ok_or_else(|| "pixels::verify: secondary leaf range overflows".to_string())?;
                if node.count == 0 || node.count > 2 || end > objects.len() {
                    return Err("pixels::verify: secondary BVH has an invalid leaf".to_string());
                }
                let exact = objects[node.first..end]
                    .iter()
                    .copied()
                    .reduce(Bounds::union)
                    .expect("nonempty leaf");
                if exact != node.bounds {
                    return Err("pixels::verify: secondary leaf bound is not canonical".to_string());
                }
            }
            (Some(left), Some(right)) => {
                if left <= index || right <= index || left >= node_count || right >= node_count {
                    return Err(
                        "pixels::verify: secondary BVH has an invalid child index".to_string()
                    );
                }
                let left_node = nodes[left];
                let right_node = nodes[right];
                if node.count <= 2
                    || left_node.first != node.first
                    || left_node.first.checked_add(left_node.count) != Some(right_node.first)
                    || left_node.count.checked_add(right_node.count) != Some(node.count)
                    || left_node.bounds.union(right_node.bounds) != node.bounds
                {
                    return Err(
                        "pixels::verify: secondary internal node is not canonical".to_string()
                    );
                }
                pending.extend([right, left]);
            }
            _ => return Err("pixels::verify: secondary BVH has only one child".to_string()),
        }
    }
    if reached.into_iter().any(|value| !value) {
        return Err("pixels::verify: secondary BVH has unreachable nodes".to_string());
    }
    Ok(())
}

fn verify_transfer_table_shape(record: &FrameRecordV1) -> Result<(), String> {
    const ENTRY_COUNT: usize = 4097;
    const HEADER_WORDS: usize = 5;
    let expected_digest = match record.tag {
        4 => "834b92da2dc0efaa7ffeee438f95a9de53988abcfa0d122f55329ec01e1ebf6f",
        5 => "28c6391387185672fd824973e342a185f7cc90d487be3d966821412509213201",
        _ => return Err("pixels::verify: unknown transfer-table tag".to_string()),
    };
    let packed_words = ENTRY_COUNT.div_ceil(4);
    exact_operands(
        FrameProgramTableKindV1::ShadingSummary,
        record,
        HEADER_WORDS + packed_words,
    )?;
    if record.operands[0] != ENTRY_COUNT as u64 {
        return Err("pixels::verify: transfer table has the wrong entry count".to_string());
    }
    let mut sealed_digest = [0_u8; 32];
    for (bytes, word) in sealed_digest
        .chunks_exact_mut(8)
        .zip(&record.operands[1..5])
    {
        bytes.copy_from_slice(&word.to_le_bytes());
    }
    let mut payload = Vec::with_capacity(ENTRY_COUNT * 2);
    for (word_index, word) in record.operands[HEADER_WORDS..].iter().enumerate() {
        for lane in 0..4 {
            let entry = word_index * 4 + lane;
            let value = ((word >> (lane * 16)) & 0xffff) as u16;
            if entry < ENTRY_COUNT {
                payload.extend_from_slice(&value.to_le_bytes());
            } else if value != 0 {
                return Err("pixels::verify: transfer table has nonzero tail padding".to_string());
            }
        }
    }
    if crate::sha256::sha256(&payload) != sealed_digest
        || crate::sha256::sha256_hex(&payload) != expected_digest
    {
        return Err(
            "pixels::verify: transfer table payload or digest is not canonical".to_string(),
        );
    }
    let mut values = payload
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let first = values.next().expect("sealed table is nonempty");
    let mut previous = first;
    for value in values {
        if value < previous {
            return Err("pixels::verify: transfer table is not monotone".to_string());
        }
        previous = value;
    }
    if first != 0 || previous != u16::MAX {
        return Err("pixels::verify: transfer table endpoints are not [0,65535]".to_string());
    }
    Ok(())
}

pub fn verify_frame_record_shape_v1(
    kind: FrameProgramTableKindV1,
    record: &FrameRecordV1,
) -> Result<(), String> {
    verify_operand_numeric_domains(kind, record)?;
    match kind {
        FrameProgramTableKindV1::Scalar => {
            let expected = match record.tag {
                1 | 2 | 6..=8 | 13 | 14 => 2,
                3..=5 => 1,
                9..=12 | 15 | 16 | 18..=21 | 24 | 32 => 3,
                17 | 25 | 27 | 28 | 31 => 4,
                22 => 7,
                23 => 8,
                26 => 6,
                30 => 5,
                29 => {
                    let count = record.operands.get(2).copied().ok_or_else(|| {
                        "pixels::verify: scalar select-index is truncated".to_string()
                    })?;
                    3_usize
                        .checked_add(usize::try_from(count).map_err(|_| {
                            "pixels::verify: scalar option count exceeds usize".to_string()
                        })?)
                        .ok_or_else(|| "pixels::verify: scalar option count overflow".to_string())?
                }
                _ => return Ok(()),
            };
            exact_operands(kind, record, expected)?;
            if record.operands[0] > 7 {
                return Err(format!(
                    "pixels::verify: scalar {} has unknown dependency tag {}",
                    record.stable_id, record.operands[0]
                ));
            }
            match record.tag {
                6 | 7 | 23 | 26 if record.operands[1] > 2 => {
                    return Err(format!(
                        "pixels::verify: scalar {} has invalid vector component {}",
                        record.stable_id, record.operands[1]
                    ));
                }
                18..=21 if record.operands[2] != u64::from(record.tag - 17) => {
                    return Err(format!(
                        "pixels::verify: scalar {} has a mismatched semantic opcode",
                        record.stable_id
                    ));
                }
                26 if record.operands[2] != 5 => {
                    return Err(format!(
                        "pixels::verify: scalar {} has a mismatched normalize semantic",
                        record.stable_id
                    ));
                }
                27 if !(1..=6).contains(&record.operands[1]) => {
                    return Err(format!(
                        "pixels::verify: scalar {} has unknown comparison opcode {}",
                        record.stable_id, record.operands[1]
                    ));
                }
                30 if record.operands[4] != 6 => {
                    return Err(format!(
                        "pixels::verify: scalar {} has a mismatched smooth-min semantic",
                        record.stable_id
                    ));
                }
                31 if record.operands[3] != 7 => {
                    return Err(format!(
                        "pixels::verify: scalar {} has a mismatched finite-color semantic",
                        record.stable_id
                    ));
                }
                32 if record.operands[2] != 8 => {
                    return Err(format!(
                        "pixels::verify: scalar {} has a mismatched roughness semantic",
                        record.stable_id
                    ));
                }
                _ => {}
            }
        }
        FrameProgramTableKindV1::Field => match record.tag {
            1 | 2 => exact_operands(kind, record, 5)?,
            3 => exact_operands(kind, record, 7)?,
            4..=6 => exact_operands(kind, record, 8)?,
            7 | 8 => exact_operands(kind, record, 9)?,
            20..=22 => exact_operands(kind, record, 3)?,
            23..=25 => exact_operands(kind, record, 4)?,
            26 | 30 => exact_operands(kind, record, 2)?,
            27 => {
                let transform = record.operands.get(2..).ok_or_else(|| {
                    "pixels::verify: transformed field record is truncated".to_string()
                })?;
                verify_transform_operands(transform, None)?;
            }
            28 => {
                exact_operands(kind, record, 6)?;
                if !(1..=3).contains(&record.operands[2]) || record.operands[4] == 0 {
                    return Err(format!(
                        "pixels::verify: repeat field {} has invalid axis/count",
                        record.stable_id
                    ));
                }
            }
            29 => {
                exact_operands(kind, record, 11)?;
                if record.operands[10] != 1 {
                    return Err(format!(
                        "pixels::verify: displaced field {} has unknown derivation",
                        record.stable_id
                    ));
                }
            }
            _ => {}
        },
        FrameProgramTableKindV1::Object => {
            let mut cursor = OperandCursor::new(kind, record);
            cursor.skip(4, "object identity")?;
            let primitive_occurrences = cursor.count("primitive occurrence count")?;
            let repeat_instances = cursor.count("repeat instance count")?;
            cursor.skip(8, "object support and world bounds")?;
            for _ in 0..primitive_occurrences {
                let path = cursor.count("primitive occurrence path length")?;
                cursor.skip(
                    path.checked_mul(2).ok_or_else(|| {
                        "pixels::verify: primitive occurrence path length overflow".to_string()
                    })?,
                    "primitive occurrence path",
                )?;
            }
            for _ in 0..repeat_instances {
                cursor.skip(1, "repeat field")?;
                let equivalents = cursor.count("repeat equivalent-field count")?;
                cursor.skip(equivalents, "repeat equivalent fields")?;
                cursor.enum_tag("repeat axis", 1..=3)?;
                cursor.skip(4, "repeat index and period")?;
            }
            cursor.finish()?;
        }
        FrameProgramTableKindV1::Feature => {
            let mut cursor = OperandCursor::new(kind, record);
            cursor.skip(20, "feature fixed header")?;
            let validity = cursor.count("validity predicate count")?;
            cursor.skip(validity, "validity predicates")?;
            cursor.enum_tag("orientation", 1..=4)?;
            match cursor.take("q-seed opcode")? {
                1 => {
                    cursor.skip(3, "affine q-seed obligation")?;
                    cursor.enum_tag("affine q-seed sign", 1..=2)?;
                }
                2 => {
                    cursor.skip(3, "quadratic q-seed leading coefficient")?;
                    let sign = cursor.take("quadratic q-seed sign")?;
                    if sign > 2 {
                        return Err(format!(
                            "pixels::verify: feature {} has unknown optional strict sign {sign}",
                            record.stable_id
                        ));
                    }
                    cursor.boolean("quadratic linear fallback")?;
                    cursor.boolean("quadratic generic fallback")?;
                }
                3 => {}
                opcode => {
                    return Err(format!(
                        "pixels::verify: feature {} has unknown q-seed opcode {opcode}",
                        record.stable_id
                    ));
                }
            }
            match cursor.take("root-isolation opcode")? {
                1 => {}
                2 => {
                    cursor.boolean("root-isolation linear fallback")?;
                    cursor.boolean("root-isolation generic fallback")?;
                }
                3 => {
                    cursor.skip(2, "root-isolation depths")?;
                    cursor.boolean("root-isolation preserve-roots")?;
                }
                opcode => {
                    return Err(format!(
                        "pixels::verify: feature {} has unknown root-isolation opcode {opcode}",
                        record.stable_id
                    ));
                }
            }
            let composition_len = cursor.count("composition payload length")?;
            let composition_end = cursor
                .at
                .checked_add(composition_len)
                .ok_or_else(|| "pixels::verify: composition payload length overflow".to_string())?;
            let composition = cursor
                .operands
                .get(cursor.at..composition_end)
                .ok_or_else(|| "pixels::verify: composition payload is truncated".to_string())?;
            verify_composition_shape(composition, record)?;
            cursor.at = composition_end;
            cursor.boolean("deformed-predictor flag")?;
            let parameters = cursor.count("influencing parameter count")?;
            cursor.skip(parameters, "influencing parameters")?;
            let path = cursor.count("occurrence path length")?;
            cursor.boolean("shared-boundary flag")?;
            cursor.skip(
                path.checked_mul(2)
                    .ok_or_else(|| "pixels::verify: occurrence path overflow".to_string())?,
                "occurrence path",
            )?;
            cursor.skip(6, "feature world bounds")?;
            cursor.finish()?;
        }
        FrameProgramTableKindV1::Material => match record.tag {
            1 => {
                exact_operands(kind, record, 17)?;
                match record.operands[11] {
                    0 if record.operands[12..15] == [u64::MAX; 3] => {}
                    1 if record.operands[12] != u64::MAX
                        && record.operands[13] != u64::MAX
                        && record.operands[14] == u64::MAX => {}
                    2 if record.operands[12] == u64::MAX
                        && record.operands[13] == u64::MAX
                        && matches!(record.operands[14], 21 | 23) => {}
                    _ => {
                        return Err(
                            "pixels::verify: material has invalid normal-detail operands"
                                .to_string(),
                        );
                    }
                }
                match (record.operands[15], record.operands[16]) {
                    (u64::MAX, u64::MAX) => {}
                    (19..=23, 0..=3) => {}
                    _ => {
                        return Err(
                            "pixels::verify: material has invalid texture/filter operands"
                                .to_string(),
                        );
                    }
                }
            }
            2 => exact_operands(kind, record, 3)?,
            3 => {
                let count = record.operands.first().copied().ok_or_else(|| {
                    "pixels::verify: material identity table is truncated".to_string()
                })?;
                exact_operands(
                    kind,
                    record,
                    1_usize
                        .checked_add(usize::try_from(count).map_err(|_| {
                            "pixels::verify: material identity count exceeds usize".to_string()
                        })?)
                        .ok_or_else(|| {
                            "pixels::verify: material identity count overflow".to_string()
                        })?,
                )?;
            }
            _ => {}
        },
        FrameProgramTableKindV1::Texture => verify_texture_shape(record)?,
        FrameProgramTableKindV1::ShadingSummary => match record.tag {
            1 => {
                let scalar_count = record.operands.get(6).copied().ok_or_else(|| {
                    "pixels::verify: shading summary scalar count is truncated".to_string()
                })?;
                let expected = usize::try_from(scalar_count)
                    .ok()
                    .and_then(|count| count.checked_mul(3))
                    .and_then(|count| count.checked_add(15))
                    .ok_or_else(|| {
                        "pixels::verify: shading summary operand count overflow".to_string()
                    })?;
                exact_operands(kind, record, expected)?;
                let basis = record.operands[3];
                let rank = record.operands[4];
                let basis_rank_valid = match basis {
                    1..=3 | 5 => rank == 0,
                    4 => (1..=4).contains(&rank),
                    _ => false,
                };
                if record.operands[2] & !0x7f != 0 || !basis_rank_valid || record.operands[5] > 25 {
                    return Err(format!(
                        "pixels::verify: shading summary {} has invalid inputs/basis/rank/anchors",
                        record.stable_id
                    ));
                }
                let scalar_end = 7 + usize::try_from(scalar_count).expect("count checked") * 3;
                for triple in record.operands[7..scalar_end].chunks_exact(3) {
                    let lo = f64::from_bits(triple[1]);
                    let hi = f64::from_bits(triple[2]);
                    if !lo.is_finite() || !hi.is_finite() || lo > hi {
                        return Err(format!(
                            "pixels::verify: shading summary {} has an invalid scalar range",
                            record.stable_id
                        ));
                    }
                }
                for texture in &record.operands[scalar_end..scalar_end + 2] {
                    if *texture != u64::MAX && !(19..=23).contains(texture) {
                        return Err(format!(
                            "pixels::verify: shading summary {} references an unsealed texture",
                            record.stable_id
                        ));
                    }
                }
                if record.operands[scalar_end + 2..scalar_end + 4]
                    .iter()
                    .any(|source| !(1..=8).contains(source))
                    || record.operands[scalar_end + 4] != 0
                    || record.operands[scalar_end + 5..scalar_end + 8]
                        .iter()
                        .any(|bits| f64::from_bits(*bits) != 0.0)
                {
                    return Err(format!(
                        "pixels::verify: exact shading summary {} has nonzero coefficients/residual",
                        record.stable_id
                    ));
                }
            }
            2 => {
                exact_operands(kind, record, 7)?;
                if record.operands[0] > 8
                    || record.operands[1] > 1
                    || record.operands[4] != 5
                    || record.operands[5] != 4
                    || record.operands[6] != 25
                {
                    return Err("pixels::verify: shading summary config is invalid".to_string());
                }
                let radius = f32::from_bits(u32::try_from(record.operands[2]).map_err(|_| {
                    "pixels::verify: shading summary AO radius exceeds f32 bits".to_string()
                })?);
                let strength = f32::from_bits(u32::try_from(record.operands[3]).map_err(|_| {
                    "pixels::verify: shading summary AO strength exceeds f32 bits".to_string()
                })?);
                if !radius.is_finite()
                    || radius <= 0.0
                    || !strength.is_finite()
                    || !(0.0..=1.0).contains(&strength)
                {
                    return Err("pixels::verify: shading summary AO values are invalid".to_string());
                }
            }
            3 => verify_secondary_bvh_shape(record)?,
            4 | 5 => verify_transfer_table_shape(record)?,
            6 => {
                exact_operands(kind, record, 21)?;
                if record.operands[0] >= 8 {
                    return Err("pixels::verify: light range slot exceeds v1 capacity".to_string());
                }
                let mut values = [0.0_f32; 20];
                for (index, value) in values.iter_mut().enumerate() {
                    *value = f32::from_bits(u32::try_from(record.operands[index + 1]).map_err(
                        |_| "pixels::verify: light range component exceeds f32 bits".to_string(),
                    )?);
                }
                if values.into_iter().any(|value| !value.is_finite())
                    || values[0] > values[3]
                    || values[1] > values[4]
                    || values[2] > values[5]
                    || values[6] <= 0.0
                    || values[7..10].iter().any(|value| *value < 0.0)
                    || values[10] < 0.0
                    || values[11] > values[14]
                    || values[12] > values[15]
                    || values[13] > values[16]
                    || values[17..20].iter().any(|value| *value < 0.0)
                {
                    return Err(
                        "pixels::verify: light range/influence contract is not finite and ordered"
                            .to_string(),
                    );
                }
            }
            _ => {}
        },
        FrameProgramTableKindV1::Parameter => {
            let path_count =
                record.operands.get(7).copied().ok_or_else(|| {
                    "pixels::verify: parameter path count is truncated".to_string()
                })?;
            let expected = 8_usize
                .checked_add(usize::try_from(path_count).map_err(|_| {
                    "pixels::verify: parameter path count exceeds usize".to_string()
                })?)
                .and_then(|value| value.checked_add(usize::from(record.flags & 1) * 2))
                .ok_or_else(|| "pixels::verify: parameter operand count overflow".to_string())?;
            exact_operands(kind, record, expected)?;
            if !(1..=12).contains(&record.operands[2]) || record.operands[6] & !0x7f != 0 {
                return Err(format!(
                    "pixels::verify: parameter {} has an invalid type/use tag",
                    record.stable_id
                ));
            }
        }
        FrameProgramTableKindV1::Csg => {
            exact_operands(kind, record, usize::from(record.tag == 1))?;
        }
        FrameProgramTableKindV1::Event => {
            let mut cursor = OperandCursor::new(kind, record);
            cursor.enum_tag("event kind", 1..=12)?;
            cursor.skip(10, "event capacity and spans")?;
            let participants = cursor.count("participant count")?;
            let coefficients = cursor.count("coefficient dependency count")?;
            for _ in 0..participants {
                cursor.enum_tag("participant kind", 1..=4)?;
                cursor.skip(1, "participant ID")?;
            }
            cursor.skip(coefficients, "coefficient dependencies")?;
            let representation_len = cursor.count("representation payload length")?;
            let representation_end =
                cursor.at.checked_add(representation_len).ok_or_else(|| {
                    "pixels::verify: event representation length overflow".to_string()
                })?;
            let representation = cursor
                .operands
                .get(cursor.at..representation_end)
                .ok_or_else(|| "pixels::verify: event representation is truncated".to_string())?;
            verify_event_representation_shape(record.tag, representation, record)?;
            cursor.at = representation_end;
            for label in ["negative side", "zero side", "positive side"] {
                cursor.enum_tag(label, 1..=19)?;
            }
            cursor.finish()?;
        }
        FrameProgramTableKindV1::CameraLightPost => {
            exact_operands(kind, record, 44)?;
            verify_camera_light_post_numeric_domains(record)?;
            let booleans = [record.operands[18], record.operands[21]];
            if booleans.iter().any(|value| *value > 1)
                || record.operands[0] == 0
                || record.operands[1] == 0
                || record.operands[2] == 0
                || record.operands[3] == 0
                || record.operands[7] > 8
            {
                return Err(
                    "pixels::verify: camera/light/post record has invalid capacities".to_string(),
                );
            }
            let ao_radius = f32::from_bits(u32::try_from(record.operands[19]).map_err(|_| {
                "pixels::verify: camera/light/post AO radius exceeds 32 bits".to_string()
            })?);
            let ao_strength = f32::from_bits(u32::try_from(record.operands[20]).map_err(|_| {
                "pixels::verify: camera/light/post AO strength exceeds 32 bits".to_string()
            })?);
            if !ao_radius.is_finite()
                || ao_radius <= 0.0
                || !ao_strength.is_finite()
                || !(0.0..=1.0).contains(&ao_strength)
            {
                return Err(
                    "pixels::verify: camera/light/post has invalid AO parameters".to_string(),
                );
            }
            for (slot, kind) in record.operands[8..16].iter().copied().enumerate() {
                if kind > 4 || (slot >= record.operands[7] as usize && kind != 0) {
                    return Err(format!(
                        "pixels::verify: camera/light/post light slot {slot} has invalid kind tag {kind}"
                    ));
                }
            }
        }
        FrameProgramTableKindV1::Transparency => match record.tag {
            1 => {
                exact_operands(kind, record, 12)?;
                if !(1..=4).contains(&record.operands[1])
                    || record.operands[10] > 1
                    || record.operands[11] > 1
                {
                    return Err(
                        "pixels::verify: transparency material class is invalid".to_string()
                    );
                }
                let lo = f64::from_bits(record.operands[2]);
                let hi = f64::from_bits(record.operands[3]);
                if !lo.is_finite() || !hi.is_finite() || lo < 0.0 || lo > hi || hi > 1.0 {
                    return Err(
                        "pixels::verify: transparency opacity interval is invalid".to_string()
                    );
                }
                if record.operands[4..10]
                    .iter()
                    .map(|bits| f64::from_bits(*bits))
                    .any(|value| !value.is_finite() || value < 0.0)
                {
                    return Err(
                        "pixels::verify: transparency radiance bound is invalid".to_string()
                    );
                }
            }
            2 => {
                exact_operands(kind, record, 5)?;
                let capacity = record.operands[0];
                let leaves = record.operands[1];
                if capacity > 64
                    || (capacity == 0) != (leaves == 0)
                    || (leaves != 0 && (!leaves.is_power_of_two() || leaves < capacity))
                    || record.operands[2..] != [1, 1, 0]
                {
                    return Err(
                        "pixels::verify: transparency transfer-tree contract is invalid"
                            .to_string(),
                    );
                }
            }
            3 => {
                exact_operands(kind, record, 9)?;
                if record.operands[..3]
                    .iter()
                    .map(|bits| f64::from_bits(*bits))
                    .any(|value| !value.is_finite() || value < 0.0)
                {
                    return Err(
                        "pixels::verify: transparency suffix radiance is invalid".to_string()
                    );
                }
                for bits in &record.operands[3..] {
                    let value = f32::from_bits(u32::try_from(*bits).map_err(|_| {
                        "pixels::verify: transparency environment exceeds f32 bits".to_string()
                    })?);
                    if !value.is_finite() || value < 0.0 {
                        return Err(
                            "pixels::verify: transparency environment bound is invalid".to_string()
                        );
                    }
                }
            }
            _ => {}
        },
        FrameProgramTableKindV1::Probe => match record.tag {
            1 => {
                exact_operands(kind, record, 18)?;
                if record.operands[0] != 1
                    || record.operands[1] > 1
                    || !(1..=3).contains(&record.operands[2])
                    || record.operands[3] == 0
                    || record.operands[3] > 16
                    || record.operands[4] == 0
                    || record.operands[4] > 8
                    || record.operands[5] == 0
                    || record.operands[5] > 16
                    || record.operands[7] == 0
                    || record.operands[8] != record.operands[7]
                    || record.operands[11] != 288
                    || record.operands[12] != 32
                    || record.operands[13] != 9
                {
                    return Err("pixels::verify: probe header contract is invalid".to_string());
                }
                let spacing =
                    f32::from_bits(u32::try_from(record.operands[6]).map_err(|_| {
                        "pixels::verify: probe spacing exceeds f32 bits".to_string()
                    })?);
                if !spacing.is_finite() || spacing <= 0.0 {
                    return Err("pixels::verify: probe spacing is invalid".to_string());
                }
                if record.operands[9]
                    != record.operands[7].checked_mul(32).ok_or_else(|| {
                        "pixels::verify: probe all-invalid ray count overflow".to_string()
                    })?
                {
                    return Err(
                        "pixels::verify: probe all-invalid ray count is inconsistent".to_string(),
                    );
                }
            }
            2 => {
                exact_operands(kind, record, 7)?;
                if record.operands[0] >= 3 || record.operands[1..4].contains(&0) {
                    return Err("pixels::verify: probe level contract is invalid".to_string());
                }
                let spacing = f32::from_bits(u32::try_from(record.operands[4]).map_err(|_| {
                    "pixels::verify: probe level spacing exceeds f32 bits".to_string()
                })?);
                if !spacing.is_finite() || spacing <= 0.0 {
                    return Err("pixels::verify: probe level spacing is invalid".to_string());
                }
            }
            3 => {
                exact_operands(kind, record, 14)?;
                if record.operands[0] >= 32 {
                    return Err("pixels::verify: probe direction ID exceeds v1 table".to_string());
                }
                for bits in &record.operands[1..] {
                    let value = f32::from_bits(u32::try_from(*bits).map_err(|_| {
                        "pixels::verify: probe numeric table entry exceeds f32 bits".to_string()
                    })?);
                    if !value.is_finite() {
                        return Err(
                            "pixels::verify: probe numeric table contains nonfinite data"
                                .to_string(),
                        );
                    }
                }
            }
            4 => {
                exact_operands(kind, record, 9)?;
                if !(1..=4).contains(&record.operands[0])
                    || record.operands[2..]
                        .iter()
                        .map(|bits| f64::from_bits(*bits))
                        .any(|value| !value.is_finite())
                    || f64::from_bits(record.operands[2]) > f64::from_bits(record.operands[5])
                    || f64::from_bits(record.operands[3]) > f64::from_bits(record.operands[6])
                    || f64::from_bits(record.operands[4]) > f64::from_bits(record.operands[7])
                    || f64::from_bits(record.operands[8]) < 0.0
                {
                    return Err("pixels::verify: probe dependency bound is invalid".to_string());
                }
            }
            _ => {}
        },
        FrameProgramTableKindV1::FixedDomain => match record.tag {
            1 => exact_operands(kind, record, 31)?,
            5 => {
                exact_operands(kind, record, 4)?;
                let exponent = record.operands[0] as i64;
                if !(-96..=63).contains(&exponent)
                    || record.operands[1] > i32::MAX as u64
                    || !record.operands[2].is_power_of_two()
                    || record.operands[2] > 64
                    || record.operands[3] > i32::MAX as u64
                {
                    return Err(
                        "pixels::verify: fixed-q domain record is outside v1 bounds".to_string()
                    );
                }
            }
            2 => {
                let mut cursor = OperandCursor::new(kind, record);
                cursor.skip(2, "exclusion identity")?;
                cursor.enum_tag("exclusion reason", 1..=11)?;
                cursor.skip(3, "exclusion interval and proof")?;
                let dependencies = cursor.count("exclusion dependency count")?;
                cursor.skip(dependencies, "exclusion dependencies")?;
                let subject_len = cursor.count("exclusion subject length")?;
                let subject_end = cursor.at.checked_add(subject_len).ok_or_else(|| {
                    "pixels::verify: exclusion subject length overflow".to_string()
                })?;
                let subject = cursor
                    .operands
                    .get(cursor.at..subject_end)
                    .ok_or_else(|| "pixels::verify: exclusion subject is truncated".to_string())?;
                match subject.first().copied() {
                    Some(1) if subject.len() == 2 => {}
                    Some(2)
                        if subject.len() == 5
                            && (1..=12).contains(&subject[1])
                            && subject[2..=3]
                                .iter()
                                .all(|value| *value == 0 || *value <= u64::from(u32::MAX) + 1) => {}
                    Some(3) if subject.len() == 3 => {}
                    _ => {
                        return Err(format!(
                            "pixels::verify: exclusion {} has malformed subject",
                            record.stable_id
                        ));
                    }
                }
                cursor.at = subject_end;
                cursor.finish()?;
            }
            3 => {
                exact_operands(kind, record, 2)?;
                if !(1..=11).contains(&record.operands[1]) {
                    return Err(format!(
                        "pixels::verify: positive-margin proof {} has unknown rule {}",
                        record.stable_id, record.operands[1]
                    ));
                }
            }
            4 => {
                let mut cursor = OperandCursor::new(kind, record);
                cursor.skip(1, "Bernstein proof ID")?;
                let axes = cursor.count("Bernstein box axis count")?;
                cursor.skip(
                    axes.checked_mul(2).ok_or_else(|| {
                        "pixels::verify: Bernstein box axis count overflow".to_string()
                    })?,
                    "Bernstein box",
                )?;
                cursor.skip(2, "Bernstein optional roots")?;
                let degrees = cursor.count("Bernstein degree count")?;
                cursor.skip(degrees, "Bernstein degrees")?;
                let orders = cursor.count("Bernstein coefficient-order count")?;
                for _ in 0..orders {
                    let order = cursor.count("Bernstein coefficient-order length")?;
                    cursor.skip(order, "Bernstein coefficient order")?;
                }
                cursor.skip(1, "Bernstein conversion radius")?;
                let nodes = cursor.count("Bernstein subdivision-node count")?;
                for _ in 0..nodes {
                    cursor.skip(3, "Bernstein subdivision node")?;
                    let sign = cursor.take("Bernstein subdivision sign")?;
                    if sign > 2 {
                        return Err(format!(
                            "pixels::verify: Bernstein proof {} has unknown optional node sign {sign}",
                            record.stable_id
                        ));
                    }
                    cursor.skip(1, "Bernstein subdivision margin")?;
                }
                cursor.enum_tag("Bernstein strict sign", 1..=2)?;
                cursor.skip(1, "Bernstein minimum margin")?;
                cursor.finish()?;
            }
            10..=12 | 17 | 32 | 33 => exact_operands(kind, record, 2)?,
            13..=16 | 31 | 34 => exact_operands(kind, record, 3)?,
            20 => {
                let mut cursor = OperandCursor::new(kind, record);
                cursor.skip(1, "polynomial ID")?;
                let terms = cursor.count("polynomial term count")?;
                cursor.skip(6, "polynomial degrees and coefficient program")?;
                for _ in 0..terms {
                    cursor.skip(6, "polynomial term")?;
                    let parameters = cursor.count("polynomial parameter exponent count")?;
                    cursor.skip(
                        parameters.checked_mul(2).ok_or_else(|| {
                            "pixels::verify: polynomial parameter exponent count overflow"
                                .to_string()
                        })?,
                        "polynomial parameter exponents",
                    )?;
                }
                cursor.finish()?;
            }
            21 => {
                exact_operands(kind, record, 8)?;
                if !(1..=2).contains(&record.operands[7]) {
                    return Err(format!(
                        "pixels::verify: rational {} has unknown strict sign {}",
                        record.stable_id, record.operands[7]
                    ));
                }
            }
            22 => {
                exact_operands(kind, record, 4)?;
                if !(1..=5).contains(&record.operands[2]) {
                    return Err(format!(
                        "pixels::verify: predicate {} has unknown sense {}",
                        record.stable_id, record.operands[2]
                    ));
                }
            }
            23 => {
                let mut cursor = OperandCursor::new(kind, record);
                cursor.skip(24, "derivative bundle roots")?;
                let parameters = cursor.count("derivative parameter count")?;
                for _ in 0..parameters {
                    cursor.skip(2, "derivative parameter roots")?;
                    let has_rate = cursor.take("declared-rate presence")?;
                    if has_rate > 1 {
                        return Err(format!(
                            "pixels::verify: derivative bundle {} has non-boolean rate presence {has_rate}",
                            record.stable_id
                        ));
                    }
                    if has_rate == 1 {
                        cursor.skip(2, "declared-rate bounds")?;
                    }
                }
                let influencing = cursor.count("derivative influencing-parameter count")?;
                cursor.skip(influencing, "derivative influencing parameters")?;
                cursor.finish()?;
            }
            24 => {
                let mut cursor = OperandCursor::new(kind, record);
                cursor.skip(1, "derivative cluster object")?;
                let leaves = cursor.count("derivative cluster leaf count")?;
                for _ in 0..leaves {
                    let path = cursor.count("derivative cluster path length")?;
                    cursor.skip(
                        path.checked_mul(2).ok_or_else(|| {
                            "pixels::verify: derivative cluster path length overflow".to_string()
                        })?,
                        "derivative cluster path",
                    )?;
                }
                let bundles = cursor.count("derivative cluster bundle count")?;
                cursor.skip(bundles, "derivative cluster bundles")?;
                cursor.skip(1, "derivative cluster scalar root")?;
                let sources = cursor.count("derivative cluster scalar-source count")?;
                cursor.skip(sources, "derivative cluster scalar sources")?;
                cursor.skip(7, "derivative cluster value/world bounds")?;
                let parameters = cursor.count("derivative cluster parameter-bound count")?;
                cursor.skip(
                    parameters.checked_mul(2).ok_or_else(|| {
                        "pixels::verify: derivative cluster parameter count overflow".to_string()
                    })?,
                    "derivative cluster parameter bounds",
                )?;
                cursor.skip(8, "derivative cluster frame/Taylor contract")?;
                cursor.boolean("derivative cluster boundary-event requirement")?;
                cursor.finish()?;
            }
            25 => {
                exact_operands(kind, record, 42)?;
                if record.operands[19] != 1 {
                    return Err(format!(
                        "pixels::verify: projective deformation {} has unknown tube method {}",
                        record.stable_id, record.operands[19]
                    ));
                }
            }
            26 => {
                let mut cursor = OperandCursor::new(kind, record);
                cursor.skip(5, "repeat template header")?;
                cursor.boolean("repeat certificate-fixes-instance")?;
                let instances = cursor.count("repeat instance count")?;
                for _ in 0..instances {
                    cursor.skip(1, "repeat instance object")?;
                    let translations = cursor.count("repeat translation count")?;
                    for _ in 0..translations {
                        cursor.skip(1, "repeat translation field")?;
                        cursor.enum_tag("repeat translation axis", 1..=3)?;
                        cursor.skip(6, "repeat translation index and bounds")?;
                    }
                }
                let events = cursor.count("repeat wrap-event count")?;
                for _ in 0..events {
                    cursor.skip(1, "repeat wrap-event field")?;
                    cursor.enum_tag("repeat wrap-event axis", 1..=3)?;
                    cursor.skip(4, "repeat wrap-event index and boundary")?;
                }
                cursor.finish()?;
            }
            27 => {
                exact_operands(kind, record, 14)?;
                if record.operands[2] != 1 {
                    return Err(format!(
                        "pixels::verify: deformation template {} has unknown derivation {}",
                        record.stable_id, record.operands[2]
                    ));
                }
            }
            28 => {
                let mut cursor = OperandCursor::new(kind, record);
                cursor.skip(1, "material-event predicate")?;
                cursor.enum_tag("material-event kind", 1..=3)?;
                cursor.skip(1, "material-event crossing bound")?;
                let owners = cursor.count("material-event owner count")?;
                cursor.skip(owners, "material-event owners")?;
                let features = cursor.count("material-event feature-owner count")?;
                cursor.skip(features, "material-event feature owners")?;
                cursor.finish()?;
            }
            30 => {
                let mut cursor = OperandCursor::new(kind, record);
                let record_kind = cursor.take("local-index record kind")?;
                cursor.enum_tag("local-index kind", 0..=5)?;
                match record_kind {
                    0 => {
                        cursor.skip(3, "local-index header counts")?;
                    }
                    1 => {
                        cursor.skip(2, "local-index chunk index and offset")?;
                        let payload = cursor.count("local-index chunk payload count")?;
                        cursor.skip(payload, "local-index chunk payload")?;
                    }
                    _ => {
                        return Err(format!(
                            "pixels::verify: local-index record {} has unknown record kind {record_kind}",
                            record.stable_id
                        ));
                    }
                }
                cursor.finish()?;
            }
            35 => exact_operands(kind, record, 14)?,
            _ => unreachable!("fixed-domain opcode checked before shape verification"),
        },
        _ => {}
    }
    Ok(())
}

fn verify_local_index_chunks(program: &FrameProgramModelV1) -> Result<(), String> {
    let fixed = program
        .table(FrameProgramTableKindV1::FixedDomain)
        .expect("namespace checked");
    let mut records = fixed.records.iter().filter(|record| record.tag == 30);
    const CHUNK_HEADER_OPERANDS: usize = 5;
    let chunk_payload = super::FRAME_PROGRAM_MAX_OPERANDS_V1 - CHUNK_HEADER_OPERANDS;
    for expected_kind in 0..6_u64 {
        let header = records.next().ok_or_else(|| {
            format!("pixels::verify: local-index {expected_kind} header is missing")
        })?;
        if header.operands[0] != 0 || header.operands[1] != expected_kind {
            return Err(format!(
                "pixels::verify: local-index {expected_kind} has a noncanonical header"
            ));
        }
        let cells = usize::try_from(header.operands[2])
            .map_err(|_| "pixels::verify: local-index cell count exceeds usize".to_string())?;
        let ids = usize::try_from(header.operands[3])
            .map_err(|_| "pixels::verify: local-index ID count exceeds usize".to_string())?;
        let chunk_count = usize::try_from(header.operands[4])
            .map_err(|_| "pixels::verify: local-index chunk count exceeds usize".to_string())?;
        let expected_payload = cells
            .checked_mul(2)
            .and_then(|count| count.checked_add(ids))
            .ok_or_else(|| "pixels::verify: local-index payload count overflow".to_string())?;
        if chunk_count != expected_payload.div_ceil(chunk_payload) {
            return Err(format!(
                "pixels::verify: local-index {expected_kind} has noncanonical chunk count {chunk_count}"
            ));
        }
        let mut payload = Vec::new();
        for chunk_index in 0..chunk_count {
            let chunk = records.next().ok_or_else(|| {
                format!(
                    "pixels::verify: local-index {expected_kind} chunk {chunk_index} is missing"
                )
            })?;
            let expected_len = (expected_payload - payload.len()).min(chunk_payload);
            if chunk.operands[0] != 1
                || chunk.operands[1] != expected_kind
                || chunk.operands[2] != chunk_index as u64
                || chunk.operands[3] != payload.len() as u64
                || chunk.operands[4] != expected_len as u64
            {
                return Err(format!(
                    "pixels::verify: local-index {expected_kind} chunk {chunk_index} is noncanonical"
                ));
            }
            payload.extend_from_slice(&chunk.operands[CHUNK_HEADER_OPERANDS..]);
        }
        if payload.len() != expected_payload {
            return Err(format!(
                "pixels::verify: local-index {expected_kind} payload has {} values, expected {expected_payload}",
                payload.len()
            ));
        }
        let id_values = &payload[cells * 2..];
        let mut next_id = 0_usize;
        for cell in 0..cells {
            let offset = usize::try_from(payload[cell * 2])
                .map_err(|_| "pixels::verify: local-index offset exceeds usize".to_string())?;
            let count = usize::try_from(payload[cell * 2 + 1])
                .map_err(|_| "pixels::verify: local-index count exceeds usize".to_string())?;
            if offset != next_id {
                return Err(format!(
                    "pixels::verify: local-index {expected_kind} cell {cell} starts at {offset}, expected {next_id}"
                ));
            }
            next_id = next_id
                .checked_add(count)
                .ok_or_else(|| "pixels::verify: local-index cell end overflows".to_string())?;
            let values = id_values.get(offset..next_id).ok_or_else(|| {
                format!(
                    "pixels::verify: local-index {expected_kind} cell {cell} exceeds its ID payload"
                )
            })?;
            if values.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(format!(
                    "pixels::verify: local-index {expected_kind} cell {cell} IDs are not strictly ordered"
                ));
            }
        }
        if next_id != ids {
            return Err(format!(
                "pixels::verify: local-index {expected_kind} cells cover {next_id} IDs, expected {ids}"
            ));
        }
    }
    if let Some(record) = records.next() {
        return Err(format!(
            "pixels::verify: unexpected trailing local-index record {}",
            record.stable_id
        ));
    }
    Ok(())
}

fn verify_sealed_numeric_policy(program: &FrameProgramModelV1) -> Result<(), String> {
    let camera_table = program
        .table(FrameProgramTableKindV1::CameraLightPost)
        .expect("namespace checked");
    let [camera] = camera_table.records.as_slice() else {
        return Err(format!(
            "pixels::verify: frame program has {} camera/light/post records, expected exactly one",
            camera_table.records.len()
        ));
    };
    let fixed = program
        .table(FrameProgramTableKindV1::FixedDomain)
        .expect("namespace checked");
    let policies = fixed
        .records
        .iter()
        .filter(|record| record.tag == 5)
        .collect::<Vec<_>>();
    let [record] = policies.as_slice() else {
        return Err(format!(
            "pixels::verify: frame program has {} fixed-q policy records, expected exactly one",
            policies.len()
        ));
    };
    let near = f64::from_bits(camera.operands[4]);
    let far = f64::from_bits(camera.operands[5]);
    let expected = derive_fixed_q_policy_v1(near, far)
        .ok_or_else(|| "pixels::verify: camera bounds have no v1 fixed-q policy".to_string())?;
    let actual = FixedQPolicyV1 {
        exponent: i16::try_from(record.operands[0] as i64)
            .map_err(|_| "pixels::verify: fixed-q exponent is not signed i16".to_string())?,
        maximum_raw: i32::try_from(record.operands[1])
            .map_err(|_| "pixels::verify: fixed-q maximum exceeds i32".to_string())?,
        reset_width: u8::try_from(record.operands[2])
            .map_err(|_| "pixels::verify: fixed-q reset width exceeds u8".to_string())?,
        error_radius: i32::try_from(record.operands[3])
            .map_err(|_| "pixels::verify: fixed-q error radius exceeds i32".to_string())?,
    };
    if actual != expected {
        return Err(format!(
            "pixels::verify: fixed-q policy {actual:?} does not match sealed camera policy {expected:?}"
        ));
    }
    Ok(())
}

fn verify_sealed_light_contracts(program: &FrameProgramModelV1) -> Result<(), String> {
    let camera = program
        .table(FrameProgramTableKindV1::CameraLightPost)
        .and_then(|table| table.records.first())
        .ok_or_else(|| "pixels::verify: missing camera/light/post contract".to_string())?;
    let summaries = program
        .table(FrameProgramTableKindV1::ShadingSummary)
        .expect("canonical namespace checked");
    for record in summaries.records.iter().filter(|record| record.tag == 6) {
        let slot = usize::try_from(record.operands[0])
            .map_err(|_| "pixels::verify: light contract slot exceeds usize".to_string())?;
        if record.operands[12..18] != camera.operands[32..38] {
            return Err(format!(
                "pixels::verify: light contract slot {slot} influence bounds differ from the sealed world"
            ));
        }
        let kind = camera.operands[8 + slot];
        for channel in 0..3 {
            let radiance = f32::from_bits(
                u32::try_from(record.operands[8 + channel])
                    .expect("light range shape checked before linked contract"),
            );
            let maximum = match kind {
                0 => 0.0,
                1 => radiance * 16_777_216.0,
                2..=4 => radiance,
                _ => unreachable!("camera light kind checked before linked contract"),
            };
            if !maximum.is_finite() || record.operands[18 + channel] != u64::from(maximum.to_bits())
            {
                return Err(format!(
                    "pixels::verify: light contract slot {slot} channel {channel} has a noncanonical maximum incident-radiance bound"
                ));
            }
        }
    }
    Ok(())
}

fn verify_p9_shading_links(program: &FrameProgramModelV1) -> Result<(), String> {
    let summaries = program
        .table(FrameProgramTableKindV1::ShadingSummary)
        .expect("canonical namespace checked");
    for (tag, label) in [
        (2, "configuration"),
        (3, "secondary BVH"),
        (4, "filmic table"),
        (5, "sRGB table"),
    ] {
        let count = summaries
            .records
            .iter()
            .filter(|record| record.tag == tag)
            .count();
        if count != 1 {
            return Err(format!(
                "pixels::verify: shading-summary table has {count} {label} records, expected exactly one"
            ));
        }
    }

    let textures = program
        .table(FrameProgramTableKindV1::Texture)
        .expect("canonical namespace checked");
    let mut texture_ids = std::collections::BTreeSet::new();
    for record in &textures.records {
        let asset = record
            .operands
            .first()
            .copied()
            .ok_or_else(|| "pixels::verify: texture record is truncated".to_string())?;
        if !texture_ids.insert(asset) {
            return Err(format!(
                "pixels::verify: compiler-owned texture asset {asset} is duplicated"
            ));
        }
    }

    let materials = program
        .table(FrameProgramTableKindV1::Material)
        .expect("canonical namespace checked");
    for record in materials.records.iter().filter(|record| record.tag == 1) {
        for &asset in [record.operands[14], record.operands[15]]
            .iter()
            .filter(|asset| **asset != u64::MAX)
        {
            if !texture_ids.contains(&asset) {
                return Err(format!(
                    "pixels::verify: material {} references missing texture asset {asset}",
                    record.stable_id
                ));
            }
        }
    }

    let scalar_count = program.record_count(FrameProgramTableKindV1::Scalar) as u64;
    let identity_ids = program
        .table(FrameProgramTableKindV1::FixedDomain)
        .expect("canonical namespace checked")
        .records
        .iter()
        .filter(|record| record.tag == 33)
        .map(|record| record.operands[0])
        .collect::<std::collections::BTreeSet<_>>();
    let mut descriptors = std::collections::BTreeSet::new();
    for record in summaries.records.iter().filter(|record| record.tag == 1) {
        let identity = record.operands[0];
        let material = usize::try_from(record.operands[1])
            .map_err(|_| "pixels::verify: shading-summary material exceeds usize".to_string())?;
        if !identity_ids.contains(&identity) {
            return Err(format!(
                "pixels::verify: shading summary {} names unknown identity set {identity}",
                record.stable_id
            ));
        }
        if materials
            .records
            .get(material)
            .is_none_or(|material| material.tag != 1)
        {
            return Err(format!(
                "pixels::verify: shading summary {} names non-sample material {material}",
                record.stable_id
            ));
        }
        if !descriptors.insert((identity, material)) {
            return Err(format!(
                "pixels::verify: shading summary {} duplicates identity/material ({identity},{material})",
                record.stable_id
            ));
        }
        let summary_scalar_count = usize::try_from(record.operands[6]).map_err(|_| {
            "pixels::verify: shading-summary scalar count exceeds usize".to_string()
        })?;
        for triple in record.operands[7..7 + summary_scalar_count * 3].chunks_exact(3) {
            if triple[0] >= scalar_count {
                return Err(format!(
                    "pixels::verify: shading summary {} names scalar {} outside {scalar_count}",
                    record.stable_id, triple[0]
                ));
            }
        }
        let tail = 7 + summary_scalar_count * 3;
        for &asset in record.operands[tail..tail + 2]
            .iter()
            .filter(|asset| **asset != u64::MAX)
        {
            if !texture_ids.contains(&asset) {
                return Err(format!(
                    "pixels::verify: shading summary {} references missing texture asset {asset}",
                    record.stable_id
                ));
            }
        }
    }
    Ok(())
}

fn verify_p10_transparency_probe_links(program: &FrameProgramModelV1) -> Result<(), String> {
    let fixed_capacities = program
        .table(FrameProgramTableKindV1::FixedDomain)
        .expect("canonical namespace checked")
        .records
        .iter()
        .find(|record| record.tag == 1)
        .ok_or_else(|| "pixels::verify: missing capacity record".to_string())?;
    let materials = program
        .table(FrameProgramTableKindV1::Material)
        .expect("canonical namespace checked");
    let transparency = program
        .table(FrameProgramTableKindV1::Transparency)
        .expect("canonical namespace checked");
    let classes = transparency
        .records
        .iter()
        .filter(|record| record.tag == 1)
        .collect::<Vec<_>>();
    if classes.len() != materials.records.len()
        || classes
            .iter()
            .enumerate()
            .any(|(material, record)| record.operands[0] != material as u64)
    {
        return Err(
            "pixels::verify: transparency classes do not cover material IDs canonically"
                .to_string(),
        );
    }
    let trees = transparency
        .records
        .iter()
        .filter(|record| record.tag == 2)
        .collect::<Vec<_>>();
    let tails = transparency
        .records
        .iter()
        .filter(|record| record.tag == 3)
        .collect::<Vec<_>>();
    if trees.len() != 1 || tails.len() != 1 {
        return Err(
            "pixels::verify: transparency table requires one tree and one tail contract"
                .to_string(),
        );
    }
    if trees[0].operands[0] != fixed_capacities.operands[13] {
        return Err(
            "pixels::verify: transparency tree capacity differs from renderer capacity".to_string(),
        );
    }
    let has_transparency = classes.iter().any(|record| record.operands[11] == 1);
    if ((program.flags >> 1) & 1 == 1) != has_transparency {
        return Err("pixels::verify: transparency header flag is inconsistent".to_string());
    }

    let probe_enabled = program
        .table(FrameProgramTableKindV1::CameraLightPost)
        .and_then(|table| table.records.first())
        .is_some_and(|record| record.operands[21] == 1);
    let probes = program
        .table(FrameProgramTableKindV1::Probe)
        .expect("canonical namespace checked");
    if !probe_enabled {
        if !probes.records.is_empty()
            || fixed_capacities.operands[15] != 0
            || program.flags & (1 << 2) != 0
        {
            return Err("pixels::verify: disabled probe contract has runtime state".to_string());
        }
        return Ok(());
    }
    if program.flags & (1 << 2) == 0 {
        return Err(
            "pixels::verify: enabled probe contract is missing its header flag".to_string(),
        );
    }
    let headers = probes
        .records
        .iter()
        .filter(|record| record.tag == 1)
        .collect::<Vec<_>>();
    let levels = probes
        .records
        .iter()
        .filter(|record| record.tag == 2)
        .collect::<Vec<_>>();
    let directions = probes
        .records
        .iter()
        .filter(|record| record.tag == 3)
        .collect::<Vec<_>>();
    let dependencies = probes
        .records
        .iter()
        .filter(|record| record.tag == 4)
        .collect::<Vec<_>>();
    let [header] = headers.as_slice() else {
        return Err("pixels::verify: probe table requires exactly one header".to_string());
    };
    if levels.len() != header.operands[2] as usize
        || directions.len() != 32
        || header.operands[10] != fixed_capacities.operands[15]
    {
        return Err("pixels::verify: probe table counts differ from sealed capacities".to_string());
    }
    let per_level = header.operands[3]
        .checked_mul(header.operands[4])
        .and_then(|value| value.checked_mul(header.operands[5]))
        .ok_or_else(|| "pixels::verify: probe dimension product overflow".to_string())?;
    if header.operands[7] != per_level * header.operands[2] {
        return Err("pixels::verify: probe count differs from level dimensions".to_string());
    }
    let base_spacing = f32::from_bits(header.operands[6] as u32);
    for (level, record) in levels.iter().enumerate() {
        let expected_spacing = base_spacing * 4.0_f32.powi(level as i32);
        if record.operands[0] != level as u64
            || record.operands[1..4] != header.operands[3..6]
            || f32::from_bits(record.operands[4] as u32).to_bits() != expected_spacing.to_bits()
            || record.operands[5] != level as u64 * per_level
            || record.operands[6] != per_level
        {
            return Err("pixels::verify: probe levels are not canonical".to_string());
        }
    }
    let mut direction_bytes = Vec::with_capacity(32 * 13 * 4);
    let mut weight_sum = 0.0_f64;
    for (direction, record) in directions.iter().enumerate() {
        if record.operands[0] != direction as u64 {
            return Err("pixels::verify: probe direction IDs are not ordered".to_string());
        }
        for (index, bits) in record.operands[1..].iter().enumerate() {
            let bits = u32::try_from(*bits)
                .map_err(|_| "pixels::verify: probe direction bits exceed u32".to_string())?;
            if index == 3 {
                weight_sum += f64::from(f32::from_bits(bits));
            }
            direction_bytes.extend(bits.to_le_bytes());
        }
    }
    if (weight_sum - std::f64::consts::TAU * 2.0).abs() > 2.0e-6 {
        return Err("pixels::verify: probe solid-angle weights do not cover 4pi".to_string());
    }
    let actual_digest = crate::sha256::sha256(&direction_bytes);
    let mut sealed_digest = [0_u8; 32];
    for (index, word) in header.operands[14..18].iter().enumerate() {
        sealed_digest[index * 8..(index + 1) * 8].copy_from_slice(&word.to_le_bytes());
    }
    if actual_digest != sealed_digest {
        return Err("pixels::verify: probe direction/SH table digest mismatch".to_string());
    }
    let mut dependency_ids = std::collections::BTreeSet::new();
    let mut object_ids = std::collections::BTreeSet::new();
    let mut light_ids = std::collections::BTreeSet::new();
    let mut material_ids = std::collections::BTreeSet::new();
    let mut environment_ids = std::collections::BTreeSet::new();
    for dependency in &dependencies {
        let kind = dependency.operands[0];
        let stable_id = dependency.operands[1];
        if !dependency_ids.insert((kind, stable_id)) {
            return Err("pixels::verify: probe dependency IDs are duplicated".to_string());
        }
        if kind == 1 {
            if stable_id >= program.record_count(FrameProgramTableKindV1::Object) as u64 {
                return Err("pixels::verify: probe dependency object ID is invalid".to_string());
            }
            object_ids.insert(stable_id);
        } else if kind == 2 {
            light_ids.insert(stable_id);
        } else if kind == 3 {
            material_ids.insert(stable_id);
        } else if kind == 4 {
            environment_ids.insert(stable_id);
        }
    }
    let light_capacity = program
        .table(FrameProgramTableKindV1::CameraLightPost)
        .and_then(|table| table.records.first())
        .map_or(0, |record| record.operands[7]);
    if object_ids != (0..program.record_count(FrameProgramTableKindV1::Object) as u64).collect()
        || light_ids != (0..light_capacity).collect()
        || material_ids != (0..materials.records.len() as u64).collect()
        || environment_ids != std::collections::BTreeSet::from([0])
    {
        return Err(
            "pixels::verify: probe dependencies omit geometry, light, material, or environment"
                .to_string(),
        );
    }
    Ok(())
}

pub fn verify_frame_program_model_v1(program: &FrameProgramModelV1) -> Result<(), String> {
    if program.numeric_revision != super::FRAME_PROGRAM_NUMERIC_REVISION_V1
        || program.formal_revision != super::FRAME_PROGRAM_FORMAL_REVISION_V1
    {
        return Err(format!(
            "pixels::verify: unsupported revisions numeric={} formal={}",
            program.numeric_revision, program.formal_revision
        ));
    }
    if program.flags & !7 != 0 {
        return Err(format!(
            "pixels::verify: unknown frame-program flags {:#x}",
            program.flags
        ));
    }
    if program.tables.len() != FrameProgramTableKindV1::ALL.len() {
        return Err(format!(
            "pixels::verify: frame program has {} tables, expected {}",
            program.tables.len(),
            FrameProgramTableKindV1::ALL.len()
        ));
    }
    for (table, expected) in program.tables.iter().zip(FrameProgramTableKindV1::ALL) {
        if table.kind != expected {
            return Err(format!(
                "pixels::verify: table order names {} where {} is required",
                table.kind.stable_name(),
                expected.stable_name()
            ));
        }
        if table.kind == FrameProgramTableKindV1::Immediate && !table.records.is_empty() {
            return Err(
                "pixels::verify: rich immediate table must be derived from record operands"
                    .to_string(),
            );
        }
        if matches!(
            table.kind,
            FrameProgramTableKindV1::Kinetic | FrameProgramTableKindV1::DebugName
        ) && !table.records.is_empty()
        {
            return Err(format!(
                "pixels::verify: predeclared {} table must be empty at P5",
                table.kind.stable_name()
            ));
        }
        for (index, record) in table.records.iter().enumerate() {
            if record.stable_id != index as u32 {
                return Err(format!(
                    "pixels::verify: {} record {} has noncanonical stable ID {}",
                    table.kind.stable_name(),
                    index,
                    record.stable_id
                ));
            }
            let known_tag = match table.kind {
                FrameProgramTableKindV1::Scalar => (1..=32).contains(&record.tag),
                FrameProgramTableKindV1::Field => {
                    (1..=8).contains(&record.tag) || (20..=30).contains(&record.tag)
                }
                FrameProgramTableKindV1::Object => record.tag == 1,
                FrameProgramTableKindV1::Feature => (1..=3).contains(&record.tag),
                FrameProgramTableKindV1::Material => (1..=3).contains(&record.tag),
                FrameProgramTableKindV1::Texture => record.tag == 1,
                FrameProgramTableKindV1::ShadingSummary => (1..=6).contains(&record.tag),
                FrameProgramTableKindV1::Parameter => record.tag == 1,
                FrameProgramTableKindV1::Event => (1..=14).contains(&record.tag),
                FrameProgramTableKindV1::Csg => (1..=6).contains(&record.tag),
                FrameProgramTableKindV1::FixedDomain => matches!(
                    record.tag,
                    1..=5 | 10..=17 | 20..=28 | 30..=35
                ),
                FrameProgramTableKindV1::CameraLightPost => record.tag == 1,
                FrameProgramTableKindV1::Transparency => (1..=3).contains(&record.tag),
                FrameProgramTableKindV1::Probe => (1..=4).contains(&record.tag),
                _ => false,
            };
            if !known_tag {
                return Err(format!(
                    "pixels::verify: {} record {} has unknown opcode {}",
                    table.kind.stable_name(),
                    index,
                    record.tag,
                ));
            }
            let allowed_flags = if table.kind == FrameProgramTableKindV1::Parameter {
                1
            } else {
                0
            };
            if record.flags & !allowed_flags != 0 {
                return Err(format!(
                    "pixels::verify: {} record {} has unknown flags {:#x}",
                    table.kind.stable_name(),
                    index,
                    record.flags,
                ));
            }
            if record.operands.len() > usize::from(u16::MAX) {
                return Err(format!(
                    "pixels::verify: {} record {} has too many operands",
                    table.kind.stable_name(),
                    index
                ));
            }
            verify_frame_record_shape_v1(table.kind, record)?;
        }
        if table.kind == FrameProgramTableKindV1::ShadingSummary {
            let light_ranges = table
                .records
                .iter()
                .filter(|record| record.tag == 6)
                .collect::<Vec<_>>();
            if light_ranges.len() != 8
                || light_ranges
                    .iter()
                    .enumerate()
                    .any(|(slot, record)| record.operands[0] != slot as u64)
            {
                return Err(
                    "pixels::verify: shading summary must seal exactly eight ordered light ranges"
                        .to_string(),
                );
            }
        }
    }
    verify_sealed_numeric_policy(program)?;
    verify_sealed_light_contracts(program)?;
    verify_p9_shading_links(program)?;
    verify_p10_transparency_probe_links(program)?;
    verify_local_index_chunks(program)?;
    let object_count = program.record_count(FrameProgramTableKindV1::Object);
    let field_count = program.record_count(FrameProgramTableKindV1::Field);
    let feature_count = program.record_count(FrameProgramTableKindV1::Feature);
    let scalar_count = program.record_count(FrameProgramTableKindV1::Scalar);
    let parameter_count = program.record_count(FrameProgramTableKindV1::Parameter);
    let scalar_table = program
        .table(FrameProgramTableKindV1::Scalar)
        .expect("namespace checked");
    for record in &scalar_table.records {
        let predecessor = |operand: usize| -> Result<(), String> {
            let child = record.operands.get(operand).copied().ok_or_else(|| {
                format!("pixels::verify: scalar {} is truncated", record.stable_id)
            })?;
            if child >= u64::from(record.stable_id) {
                return Err(format!(
                    "pixels::verify: scalar {} names non-predecessor {child}",
                    record.stable_id
                ));
            }
            Ok(())
        };
        match record.tag {
            8 => {
                let parameter = record.operands.get(1).copied().ok_or_else(|| {
                    format!(
                        "pixels::verify: scalar {} parameter is truncated",
                        record.stable_id
                    )
                })?;
                if parameter >= parameter_count as u64 {
                    return Err(format!(
                        "pixels::verify: scalar {} names parameter {parameter} outside {parameter_count}",
                        record.stable_id
                    ));
                }
            }
            9..=12 | 15 | 16 => {
                predecessor(1)?;
                predecessor(2)?;
            }
            13 | 14 | 18..=21 | 32 => predecessor(1)?,
            17 | 28 | 30 => {
                predecessor(1)?;
                predecessor(2)?;
                predecessor(3)?;
            }
            22 => {
                for operand in 1..=6 {
                    predecessor(operand)?;
                }
            }
            23 => {
                for operand in 2..=7 {
                    predecessor(operand)?;
                }
            }
            24 => {
                predecessor(1)?;
                predecessor(2)?;
            }
            25 => {
                predecessor(1)?;
                predecessor(2)?;
                predecessor(3)?;
            }
            26 => {
                for operand in 3..=5 {
                    predecessor(operand)?;
                }
            }
            27 => {
                predecessor(2)?;
                predecessor(3)?;
            }
            29 => {
                predecessor(1)?;
                let count = record.operands.get(2).copied().ok_or_else(|| {
                    format!(
                        "pixels::verify: scalar {} select index is truncated",
                        record.stable_id
                    )
                })?;
                let count = usize::try_from(count)
                    .map_err(|_| "pixels::verify: scalar option count exceeds usize".to_string())?;
                if record.operands.len() != 3 + count {
                    return Err(format!(
                        "pixels::verify: scalar {} option count disagrees with operands",
                        record.stable_id
                    ));
                }
                for operand in 3..record.operands.len() {
                    predecessor(operand)?;
                }
            }
            31 => {
                predecessor(1)?;
                predecessor(2)?;
            }
            _ => {}
        }
    }
    for record in &program
        .table(FrameProgramTableKindV1::Field)
        .expect("namespace checked")
        .records
    {
        let scalar = record
            .operands
            .first()
            .copied()
            .ok_or_else(|| "pixels::verify: field record is truncated".to_string())?;
        if scalar >= scalar_count as u64 {
            return Err(format!(
                "pixels::verify: field {} names scalar {scalar} outside {scalar_count}",
                record.stable_id
            ));
        }
        let child_operands: &[usize] = match record.tag {
            20..=25 => &[1, 2],
            26..=30 => &[1],
            _ => &[],
        };
        for &operand in child_operands {
            let child = record.operands.get(operand).copied().ok_or_else(|| {
                format!("pixels::verify: field {} is truncated", record.stable_id)
            })?;
            if child >= u64::from(record.stable_id) {
                return Err(format!(
                    "pixels::verify: field {} names non-predecessor {child}",
                    record.stable_id
                ));
            }
        }
        let scalar_operands: Vec<usize> = match record.tag {
            1 | 2 => (1..=4).collect(),
            3 => (1..=6).collect(),
            4..=8 => (1..record.operands.len()).collect(),
            23..=25 => vec![3],
            28 => vec![5],
            29 => (2..=9).collect(),
            _ => Vec::new(),
        };
        for operand in scalar_operands {
            let scalar = record.operands.get(operand).copied().ok_or_else(|| {
                format!("pixels::verify: field {} is truncated", record.stable_id)
            })?;
            if scalar >= scalar_count as u64 {
                return Err(format!(
                    "pixels::verify: field {} names scalar {scalar} outside {scalar_count}",
                    record.stable_id
                ));
            }
        }
        if record.tag == 27 {
            verify_transform_operands(&record.operands[2..], Some(scalar_count))?;
        }
    }
    let material_table = program
        .table(FrameProgramTableKindV1::Material)
        .expect("namespace checked");
    for record in &material_table.records {
        match record.tag {
            1 => {
                for &scalar in &record.operands[..11] {
                    if scalar >= scalar_count as u64 {
                        return Err(format!(
                            "pixels::verify: material {} names scalar {scalar} outside {scalar_count}",
                            record.stable_id
                        ));
                    }
                }
                if record.operands[11] == 1 {
                    for &scalar in &record.operands[12..14] {
                        if scalar >= scalar_count as u64 {
                            return Err(format!(
                                "pixels::verify: material {} names normal scalar {scalar} outside {scalar_count}",
                                record.stable_id
                            ));
                        }
                    }
                }
            }
            2 => {
                let predicate = record.operands.first().copied().ok_or_else(|| {
                    format!("pixels::verify: material {} is truncated", record.stable_id)
                })?;
                if predicate >= scalar_count as u64 {
                    return Err(format!(
                        "pixels::verify: material {} names scalar {predicate} outside {scalar_count}",
                        record.stable_id
                    ));
                }
                for operand in 1..=2 {
                    let child = record.operands.get(operand).copied().ok_or_else(|| {
                        format!("pixels::verify: material {} is truncated", record.stable_id)
                    })?;
                    if child >= u64::from(record.stable_id) {
                        return Err(format!(
                            "pixels::verify: material {} names non-predecessor {child}",
                            record.stable_id
                        ));
                    }
                }
            }
            3 => {
                let count = record.operands.first().copied().ok_or_else(|| {
                    format!("pixels::verify: material {} is truncated", record.stable_id)
                })?;
                if record.operands.len() != 1 + count as usize {
                    return Err(format!(
                        "pixels::verify: material {} identity count disagrees with operands",
                        record.stable_id
                    ));
                }
                for &child in &record.operands[1..] {
                    if child >= u64::from(record.stable_id) {
                        return Err(format!(
                            "pixels::verify: material {} names non-predecessor {child}",
                            record.stable_id
                        ));
                    }
                }
            }
            _ => unreachable!("opcode checked"),
        }
    }
    for record in &program
        .table(FrameProgramTableKindV1::Parameter)
        .expect("namespace checked")
        .records
    {
        let immutable = record
            .operands
            .get(5)
            .copied()
            .ok_or_else(|| "pixels::verify: parameter record is truncated".to_string())?;
        if immutable > 1 {
            return Err(format!(
                "pixels::verify: parameter {} has non-boolean immutable value {immutable}",
                record.stable_id
            ));
        }
        let lo = record
            .operands
            .get(3)
            .copied()
            .map(f64::from_bits)
            .ok_or_else(|| "pixels::verify: parameter range is truncated".to_string())?;
        let hi = record
            .operands
            .get(4)
            .copied()
            .map(f64::from_bits)
            .ok_or_else(|| "pixels::verify: parameter range is truncated".to_string())?;
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            return Err(format!(
                "pixels::verify: parameter {} has invalid range [{lo},{hi}]",
                record.stable_id
            ));
        }
    }
    for record in &program
        .table(FrameProgramTableKindV1::Object)
        .expect("namespace checked")
        .records
    {
        if record.operands.first().copied() != Some(u64::from(record.stable_id)) {
            return Err(format!(
                "pixels::verify: object {} has a mismatched embedded ID",
                record.stable_id
            ));
        }
        let source_root = record.operands[1];
        if source_root >= field_count as u64 {
            return Err(format!(
                "pixels::verify: object {} names field {source_root} outside {field_count}",
                record.stable_id
            ));
        }
        let scalar = record
            .operands
            .get(2)
            .copied()
            .ok_or_else(|| "pixels::verify: object record is truncated".to_string())?;
        if scalar >= scalar_count as u64 {
            return Err(format!(
                "pixels::verify: object {} names scalar {} outside {}",
                record.stable_id, scalar, scalar_count
            ));
        }
        let support_lo = record
            .operands
            .get(6)
            .copied()
            .map(f64::from_bits)
            .ok_or_else(|| "pixels::verify: object support interval is truncated".to_string())?;
        let support_hi = record
            .operands
            .get(7)
            .copied()
            .map(f64::from_bits)
            .ok_or_else(|| "pixels::verify: object support interval is truncated".to_string())?;
        if !support_lo.is_finite()
            || !support_hi.is_finite()
            || support_lo < 0.0
            || support_lo > support_hi
        {
            return Err(format!(
                "pixels::verify: object {} has invalid support interval [{support_lo},{support_hi}]",
                record.stable_id
            ));
        }
        let primitive_count = usize::try_from(record.operands[4])
            .map_err(|_| "pixels::verify: primitive occurrence count exceeds usize".to_string())?;
        let repeat_count = usize::try_from(record.operands[5])
            .map_err(|_| "pixels::verify: repeat instance count exceeds usize".to_string())?;
        let fields = &program
            .table(FrameProgramTableKindV1::Field)
            .expect("namespace checked")
            .records;
        let mut at = 14_usize;
        for _ in 0..primitive_count {
            let path_len = usize::try_from(record.operands[at])
                .map_err(|_| "pixels::verify: primitive path length exceeds usize".to_string())?;
            at += 1;
            let mut path = Vec::with_capacity(path_len);
            for _ in 0..path_len {
                let field_id = record.operands[at];
                let child_slot = record.operands[at + 1];
                let _field = usize::try_from(field_id)
                    .ok()
                    .and_then(|index| fields.get(index))
                    .ok_or_else(|| {
                        format!(
                            "pixels::verify: object {} primitive path names field {field_id} outside {field_count}",
                            record.stable_id
                        )
                    })?;
                if child_slot > 1 {
                    return Err(format!(
                        "pixels::verify: object {} primitive path has invalid child slot {child_slot}",
                        record.stable_id
                    ));
                }
                path.push((field_id, child_slot));
                at += 2;
            }
            let Some(&(leaf_id, leaf_slot)) = path.first() else {
                return Err(format!(
                    "pixels::verify: object {} has an empty primitive path",
                    record.stable_id
                ));
            };
            let leaf = &fields[usize::try_from(leaf_id).expect("field range checked")];
            if !(1..=8).contains(&leaf.tag) || leaf_slot != 0 {
                return Err(format!(
                    "pixels::verify: object {} primitive path starts at non-primitive field {leaf_id}",
                    record.stable_id
                ));
            }
            for pair in path.windows(2) {
                let (child, _) = pair[0];
                let (parent_id, slot) = pair[1];
                let parent = &fields[usize::try_from(parent_id).expect("field range checked")];
                let operand = match (parent.tag, slot) {
                    (20..=25, 0) | (26..=30, 0) => 1,
                    (20..=25, 1) => 2,
                    _ => {
                        return Err(format!(
                            "pixels::verify: object {} path uses unavailable child slot {slot} on field {parent_id}",
                            record.stable_id
                        ));
                    }
                };
                if parent.operands.get(operand).copied() != Some(child) {
                    return Err(format!(
                        "pixels::verify: object {} path edge {child}->{parent_id} does not match field operands",
                        record.stable_id
                    ));
                }
            }
            if path.last().map(|step| step.0) != Some(source_root) {
                return Err(format!(
                    "pixels::verify: object {} primitive path does not end at source field {source_root}",
                    record.stable_id
                ));
            }
        }
        for _ in 0..repeat_count {
            let repeat_field = record.operands[at];
            let equivalents = usize::try_from(record.operands[at + 1])
                .map_err(|_| "pixels::verify: repeat equivalent count exceeds usize".to_string())?;
            if usize::try_from(repeat_field)
                .ok()
                .and_then(|index| fields.get(index))
                .is_none_or(|field| field.tag != 28)
            {
                return Err(format!(
                    "pixels::verify: object {} repeat names non-repeat field {repeat_field}",
                    record.stable_id
                ));
            }
            at += 2;
            for equivalent in &record.operands[at..at + equivalents] {
                if *equivalent >= field_count as u64 {
                    return Err(format!(
                        "pixels::verify: object {} repeat names field {equivalent} outside {field_count}",
                        record.stable_id
                    ));
                }
            }
            at += equivalents;
            let period_lo = f64::from_bits(record.operands[at + 3]);
            let period_hi = f64::from_bits(record.operands[at + 4]);
            if !period_lo.is_finite()
                || !period_hi.is_finite()
                || period_lo <= 0.0
                || period_lo > period_hi
            {
                return Err(format!(
                    "pixels::verify: object {} repeat has invalid period [{period_lo},{period_hi}]",
                    record.stable_id
                ));
            }
            at += 5;
        }
        if at != record.operands.len() {
            return Err(format!(
                "pixels::verify: object {} nested records do not consume all operands",
                record.stable_id
            ));
        }
    }
    let mut features_per_object = vec![0_u64; object_count];
    for record in &program
        .table(FrameProgramTableKindV1::Feature)
        .expect("namespace checked")
        .records
    {
        let object = record
            .operands
            .first()
            .copied()
            .ok_or_else(|| "pixels::verify: feature record is truncated".to_string())?;
        if object >= object_count as u64 {
            return Err(format!(
                "pixels::verify: feature {} names object {} outside {}",
                record.stable_id, object, object_count
            ));
        }
        let primitive = record.operands[1];
        let scalar = record.operands[4];
        if primitive >= field_count as u64 || scalar >= scalar_count as u64 {
            return Err(format!(
                "pixels::verify: feature {} names field/scalar ({primitive},{scalar}) outside ({field_count},{scalar_count})",
                record.stable_id
            ));
        }
        features_per_object[usize::try_from(object)
            .map_err(|_| "pixels::verify: feature object exceeds usize".to_string())?] += 1;
        for (lo_index, hi_index) in [(16, 17)] {
            let lo = record
                .operands
                .get(lo_index)
                .copied()
                .map(f64::from_bits)
                .ok_or_else(|| "pixels::verify: feature q interval is truncated".to_string())?;
            let hi = record
                .operands
                .get(hi_index)
                .copied()
                .map(f64::from_bits)
                .ok_or_else(|| "pixels::verify: feature q interval is truncated".to_string())?;
            if !lo.is_finite() || !hi.is_finite() || lo > hi || lo <= 0.0 {
                return Err(format!(
                    "pixels::verify: feature {} has invalid positive-q interval [{lo},{hi}]",
                    record.stable_id
                ));
            }
        }
        let bounds = record
            .operands
            .get(record.operands.len().saturating_sub(6)..)
            .ok_or_else(|| "pixels::verify: feature world bounds are truncated".to_string())?;
        for axis in 0..3 {
            let lo = f64::from_bits(bounds[axis]);
            let hi = f64::from_bits(bounds[axis + 3]);
            if !lo.is_finite() || !hi.is_finite() || lo > hi {
                return Err(format!(
                    "pixels::verify: feature {} has invalid world bound axis {axis} [{lo},{hi}]",
                    record.stable_id
                ));
            }
        }
    }
    let fixed_table = program
        .table(FrameProgramTableKindV1::FixedDomain)
        .expect("namespace checked");
    let material_event_count = fixed_table
        .records
        .iter()
        .filter(|record| record.tag == 28)
        .count();
    let coefficient_count = fixed_table
        .records
        .iter()
        .filter(|record| (10..=17).contains(&record.tag))
        .count();
    for record in &program
        .table(FrameProgramTableKindV1::Event)
        .expect("namespace checked")
        .records
    {
        let participants = usize::try_from(record.operands[11])
            .map_err(|_| "pixels::verify: event participant count exceeds usize".to_string())?;
        let coefficients = usize::try_from(record.operands[12]).map_err(|_| {
            "pixels::verify: event coefficient dependency count exceeds usize".to_string()
        })?;
        let mut at = 13;
        for _ in 0..participants {
            let participant_kind = record.operands[at];
            let participant = record.operands[at + 1];
            let limit = match participant_kind {
                1 => feature_count,
                2 => object_count,
                3 => field_count,
                4 => material_event_count,
                _ => unreachable!("participant opcode checked by record-shape verifier"),
            };
            if participant >= limit as u64 {
                return Err(format!(
                    "pixels::verify: event {} participant kind {participant_kind} names {participant} outside {limit}",
                    record.stable_id
                ));
            }
            at += 2;
        }
        for dependency in &record.operands[at..at + coefficients] {
            if *dependency >= coefficient_count as u64 {
                return Err(format!(
                    "pixels::verify: event {} names coefficient {dependency} outside {coefficient_count}",
                    record.stable_id
                ));
            }
        }
    }
    for (object, count) in features_per_object.into_iter().enumerate() {
        if count == 0 {
            return Err(format!(
                "pixels::verify: object {object} owns no feature and is not marked empty/pruned"
            ));
        }
    }
    let mut depth = 0_i64;
    for record in &program
        .table(FrameProgramTableKindV1::Csg)
        .expect("namespace checked")
        .records
    {
        match record.tag {
            1 => {
                let object = record
                    .operands
                    .first()
                    .copied()
                    .ok_or_else(|| "pixels::verify: CSG Push is truncated".to_string())?;
                if object >= object_count as u64 {
                    return Err(format!(
                        "pixels::verify: CSG Push names object {object} outside {object_count}"
                    ));
                }
                depth += 1;
            }
            2 if depth >= 1 => {}
            3 | 4 if depth >= 2 => depth -= 1,
            5 | 6
                if program
                    .table(FrameProgramTableKindV1::Csg)
                    .expect("namespace checked")
                    .records
                    .len()
                    == 1 =>
            {
                depth = 1
            }
            _ => {
                return Err(format!(
                    "pixels::verify: CSG opcode {} underflows or is noncanonical",
                    record.tag
                ));
            }
        }
    }
    if depth != 1 {
        return Err(format!(
            "pixels::verify: CSG program leaves stack depth {depth}, expected 1"
        ));
    }
    let fixed = fixed_table;
    let capacities = fixed
        .records
        .first()
        .filter(|record| record.tag == 1)
        .ok_or_else(|| "pixels::verify: capacity record is missing".to_string())?;
    if capacities.operands.len() != 31 {
        return Err(format!(
            "pixels::verify: capacity record has {} fields, expected 31",
            capacities.operands.len()
        ));
    }
    let state_bytes = capacities.operands[16];
    if state_bytes > crate::layout::PIXELS_STATE_BYTES_MAX {
        return Err(format!(
            "P025: renderer-generated image memory needs {state_bytes} mutable bytes, exceeding {}",
            crate::layout::PIXELS_STATE_BYTES_MAX
        ));
    }
    let counts = [
        (
            "objects",
            capacities.operands[1],
            program.record_count(FrameProgramTableKindV1::Object) as u64,
        ),
        (
            "features",
            capacities.operands[2],
            program.record_count(FrameProgramTableKindV1::Feature) as u64,
        ),
        (
            "parameters",
            capacities.operands[3],
            program.record_count(FrameProgramTableKindV1::Parameter) as u64,
        ),
    ];
    for (name, capacity, required) in counts {
        if capacity < required {
            return Err(format!(
                "pixels::verify: {name} capacity {capacity} is below exact requirement {required}"
            ));
        }
    }
    let coefficient_records = fixed
        .records
        .iter()
        .filter(|record| (10..=17).contains(&record.tag))
        .collect::<Vec<_>>();
    if capacities.operands[18] < coefficient_records.len() as u64 {
        return Err(format!(
            "pixels::verify: coefficient capacity {} is below {} records",
            capacities.operands[18],
            coefficient_records.len()
        ));
    }
    for (index, record) in coefficient_records.iter().enumerate() {
        if record.operands.first().copied() != Some(index as u64) {
            return Err(format!(
                "pixels::verify: coefficient record {index} has noncanonical embedded ID"
            ));
        }
        let predecessor = |operand: usize| -> Result<(), String> {
            let id = record.operands.get(operand).copied().ok_or_else(|| {
                format!("pixels::verify: coefficient record {index} is truncated")
            })?;
            if id >= index as u64 {
                return Err(format!(
                    "pixels::verify: coefficient record {index} names non-predecessor {id}"
                ));
            }
            Ok(())
        };
        match record.tag {
            15 | 16 => {
                predecessor(1)?;
                predecessor(2)?;
            }
            17 => predecessor(1)?,
            _ => {}
        }
    }
    for exclusion in fixed.records.iter().filter(|record| record.tag == 2) {
        let lo = exclusion
            .operands
            .get(3)
            .copied()
            .map(f64::from_bits)
            .ok_or_else(|| "pixels::verify: exclusion interval is truncated".to_string())?;
        let hi = exclusion
            .operands
            .get(4)
            .copied()
            .map(f64::from_bits)
            .ok_or_else(|| "pixels::verify: exclusion interval is truncated".to_string())?;
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            return Err(format!(
                "pixels::verify: exclusion {} has invalid interval [{lo},{hi}]",
                exclusion.stable_id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shading_summary_descriptor(basis: u64, rank: u64, anchors: u64) -> FrameRecordV1 {
        FrameRecordV1 {
            stable_id: 0,
            tag: 1,
            flags: 0,
            operands: vec![
                0,
                0,
                0,
                basis,
                rank,
                anchors,
                0,
                u64::MAX,
                u64::MAX,
                8,
                8,
                0,
                0.0_f64.to_bits(),
                0.0_f64.to_bits(),
                0.0_f64.to_bits(),
            ],
        }
    }

    fn nested_transform(depth: usize) -> Vec<u64> {
        let mut transform = vec![1, 0, 0, 0];
        for _ in 0..depth {
            let mut wrapper = vec![6, 0, transform.len() as u64];
            wrapper.extend(transform);
            transform = wrapper;
        }
        transform
    }

    #[test]
    fn transform_verification_checks_scalar_ids_and_has_a_hard_depth_ceiling() {
        let exact = nested_transform(super::super::FRAME_PROGRAM_TRANSFORM_DEPTH_MAX_V1);
        verify_transform_operands(&exact, Some(1)).expect("exact transform depth ceiling");

        let over = nested_transform(super::super::FRAME_PROGRAM_TRANSFORM_DEPTH_MAX_V1 + 1);
        let error = verify_transform_operands(&over, Some(1)).expect_err("one over depth ceiling");
        assert!(error.contains("nesting exceeds"), "{error}");

        let error = verify_transform_operands(&[1, 0, u64::MAX, 0], Some(1))
            .expect_err("dangling transform scalar");
        assert!(error.contains("names scalar"), "{error}");
    }

    #[test]
    fn fixed_q_domain_shape_is_bounded_and_signed() {
        let valid = FrameRecordV1 {
            stable_id: 1,
            tag: 5,
            flags: 0,
            operands: vec![(-8_i64) as u64, i32::MAX as u64, 32, 7],
        };
        verify_frame_record_shape_v1(FrameProgramTableKindV1::FixedDomain, &valid)
            .expect("valid fixed-q domain");

        for operands in [
            vec![(-97_i64) as u64, 1, 32, 0],
            vec![64, 1, 32, 0],
            vec![0, i32::MAX as u64 + 1, 32, 0],
            vec![0, 1, 3, 0],
            vec![0, 1, 128, 0],
            vec![0, 1, 32, i32::MAX as u64 + 1],
        ] {
            let invalid = FrameRecordV1 {
                operands,
                ..valid.clone()
            };
            let error =
                verify_frame_record_shape_v1(FrameProgramTableKindV1::FixedDomain, &invalid)
                    .expect_err("out-of-range fixed-q record");
            assert!(error.contains("outside v1 bounds"), "{error}");
        }
    }

    #[test]
    fn shading_summary_basis_rank_and_anchor_shape_is_canonical() {
        for (basis, rank) in [(1, 0), (2, 0), (3, 0), (4, 1), (4, 4), (5, 0)] {
            verify_frame_record_shape_v1(
                FrameProgramTableKindV1::ShadingSummary,
                &shading_summary_descriptor(basis, rank, 25),
            )
            .expect("canonical shading-summary descriptor");
        }

        for (basis, rank, anchors) in [
            (0, 0, 0),
            (6, 0, 0),
            (1, 1, 0),
            (2, 4, 0),
            (4, 0, 0),
            (4, 5, 0),
            (5, 1, 0),
            (5, 0, 26),
        ] {
            let error = verify_frame_record_shape_v1(
                FrameProgramTableKindV1::ShadingSummary,
                &shading_summary_descriptor(basis, rank, anchors),
            )
            .expect_err("noncanonical shading-summary descriptor");
            assert!(error.contains("inputs/basis/rank/anchors"), "{error}");
        }
    }
}
