//! Versioned semantic validation for the shared FrameProgram v1 wire contract.
//!
//! This module depends only on pointer-free format records and machine
//! ceilings. Compiler analysis graphs and proof-stage types stay out of
//! `wrela-machine`.

use super::{FrameProgramModelV1, FrameProgramTableKindV1, FrameRecordV1};

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
    let mut values = [0.0_f32; 12];
    for (target, index) in values.iter_mut().zip(22..34) {
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
            1 => exact_operands(kind, record, 11)?,
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
            exact_operands(kind, record, 34)?;
            verify_camera_light_post_numeric_domains(record)?;
            let booleans = [record.operands[18], record.operands[19]];
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
            for (slot, kind) in record.operands[8..16].iter().copied().enumerate() {
                if kind > 4 || (slot >= record.operands[7] as usize && kind != 0) {
                    return Err(format!(
                        "pixels::verify: camera/light/post light slot {slot} has invalid kind tag {kind}"
                    ));
                }
            }
        }
        FrameProgramTableKindV1::FixedDomain => match record.tag {
            1 => exact_operands(kind, record, 31)?,
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

pub fn verify_frame_program_model_v1(program: &FrameProgramModelV1) -> Result<(), String> {
    if program.numeric_revision != super::FRAME_PROGRAM_NUMERIC_REVISION_V1
        || program.formal_revision != super::FRAME_PROGRAM_FORMAL_REVISION_V1
    {
        return Err(format!(
            "pixels::verify: unsupported revisions numeric={} formal={}",
            program.numeric_revision, program.formal_revision
        ));
    }
    if program.flags & !1 != 0 {
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
            FrameProgramTableKindV1::Texture
                | FrameProgramTableKindV1::ShadingSummary
                | FrameProgramTableKindV1::Transparency
                | FrameProgramTableKindV1::Probe
                | FrameProgramTableKindV1::Kinetic
                | FrameProgramTableKindV1::DebugName
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
                FrameProgramTableKindV1::Parameter => record.tag == 1,
                FrameProgramTableKindV1::Event => (1..=14).contains(&record.tag),
                FrameProgramTableKindV1::Csg => (1..=6).contains(&record.tag),
                FrameProgramTableKindV1::FixedDomain => matches!(
                    record.tag,
                    1..=4 | 10..=17 | 20..=28 | 30..=35
                ),
                FrameProgramTableKindV1::CameraLightPost => record.tag == 1,
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
    }
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
                for &scalar in &record.operands {
                    if scalar >= scalar_count as u64 {
                        return Err(format!(
                            "pixels::verify: material {} names scalar {scalar} outside {scalar_count}",
                            record.stable_id
                        ));
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
}
