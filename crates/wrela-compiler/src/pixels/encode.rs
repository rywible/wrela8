//! Deterministic two-pass FrameProgram v1 encoder.

use super::binary_verify;
use super::diagnostics::PixelsError;
use super::program::{FrameRecord, VerifiedFrameProgram};
use super::version::{
    FRAME_PROGRAM_DIGEST_BYTES_V1, FRAME_PROGRAM_DIGEST_OFFSET_V1, FRAME_PROGRAM_HEADER_BYTES_V1,
    FRAME_PROGRAM_HOT_ALIGNMENT_V1, FRAME_PROGRAM_MAGIC_V1, FRAME_PROGRAM_MAX_BYTES_V1,
    FRAME_PROGRAM_TABLE_BYTES_V1, FRAME_PROGRAM_VERSION_V1, FrameProgramTableKindV1,
};

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn align(&mut self, alignment: usize) -> Result<(), PixelsError> {
        if alignment == 0 || !alignment.is_power_of_two() {
            return Err(PixelsError::Diagnostic(
                super::diagnostics::PixelsDiagnostic::internal("pixels::encode: invalid alignment"),
            ));
        }
        let padding = (alignment - self.bytes.len() % alignment) % alignment;
        self.bytes.resize(
            self.bytes
                .len()
                .checked_add(padding)
                .ok_or_else(|| internal("alignment overflow"))?,
            0,
        );
        Ok(())
    }

    // The v1 writer surface is frozen up front; later predeclared tables use
    // these widths even though P5's generic records currently store operands
    // as u64 immediates.
    #[allow(dead_code)]
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    #[allow(dead_code)]
    fn i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    #[allow(dead_code)]
    fn f32_bits(&mut self, bits: u32) {
        self.u32(bits);
    }

    fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }
}

#[derive(Clone, Copy)]
struct DirectoryEntry {
    kind: FrameProgramTableKindV1,
    count: u32,
    offset: u32,
    byte_len: u32,
}

fn internal(message: impl Into<String>) -> PixelsError {
    PixelsError::Diagnostic(super::diagnostics::PixelsDiagnostic::internal(format!(
        "pixels::encode: {}",
        message.into()
    )))
}

fn checked_u16(value: usize, what: &str) -> Result<u16, PixelsError> {
    u16::try_from(value).map_err(|_| internal(format!("{what} exceeds u16")))
}

fn checked_u32(value: usize, what: &str) -> Result<u32, PixelsError> {
    u32::try_from(value).map_err(|_| internal(format!("{what} exceeds u32")))
}

fn encode_record(
    writer: &mut Writer,
    record: &FrameRecord,
    operand_offset: u32,
) -> Result<(), PixelsError> {
    writer.u32(record.stable_id);
    writer.u16(record.tag);
    writer.u16(record.flags);
    writer.u32(operand_offset);
    writer.u16(checked_u16(record.operands.len(), "operand count")?);
    writer.u16(0);
    Ok(())
}

pub fn encode(program: &VerifiedFrameProgram) -> Result<Vec<u8>, PixelsError> {
    let program = program.program();
    let mut table_payloads = Vec::<(FrameProgramTableKindV1, Vec<u8>, u32)>::new();
    let mut immediates = Vec::<(u16, u32, u32, u64)>::new();
    for kind in FrameProgramTableKindV1::ALL {
        if kind == FrameProgramTableKindV1::Immediate {
            table_payloads.push((kind, Vec::new(), 0));
            continue;
        }
        let table = program
            .table(kind)
            .ok_or_else(|| internal(format!("missing table namespace {}", kind.stable_name())))?;
        let mut writer = Writer::default();
        for record in &table.records {
            let operand_offset = checked_u32(immediates.len(), "immediate offset")?;
            encode_record(&mut writer, record, operand_offset)?;
            for (ordinal, value) in record.operands.iter().copied().enumerate() {
                immediates.push((
                    kind.code(),
                    record.stable_id,
                    checked_u32(ordinal, "operand ordinal")?,
                    value,
                ));
            }
        }
        table_payloads.push((
            kind,
            writer.bytes,
            checked_u32(table.records.len(), "table record count")?,
        ));
    }
    let mut immediate_writer = Writer::default();
    for (owner_kind, owner_id, ordinal, value) in &immediates {
        immediate_writer.u16(*owner_kind);
        immediate_writer.u16(0);
        immediate_writer.u32(*owner_id);
        immediate_writer.u32(*ordinal);
        immediate_writer.u32(0);
        immediate_writer.u64(*value);
    }
    let immediate_count = checked_u32(immediates.len(), "immediate record count")?;
    let immediate = table_payloads
        .iter_mut()
        .find(|(kind, _, _)| *kind == FrameProgramTableKindV1::Immediate)
        .expect("namespace complete");
    immediate.1 = immediate_writer.bytes;
    immediate.2 = immediate_count;

    let directory_bytes = usize::from(FrameProgramTableKindV1::REQUIRED_COUNT)
        .checked_mul(usize::from(FRAME_PROGRAM_TABLE_BYTES_V1))
        .ok_or_else(|| internal("directory size overflow"))?;
    let mut cursor = usize::from(FRAME_PROGRAM_HEADER_BYTES_V1)
        .checked_add(directory_bytes)
        .ok_or_else(|| internal("header+directory overflow"))?;
    let mut directory = Vec::with_capacity(table_payloads.len());
    for (kind, bytes, count) in &table_payloads {
        if *count == 0 {
            if !bytes.is_empty() {
                return Err(internal(format!(
                    "empty {} table has payload bytes",
                    kind.stable_name()
                )));
            }
            directory.push(DirectoryEntry {
                kind: *kind,
                count: 0,
                offset: 0,
                byte_len: 0,
            });
            continue;
        }
        let alignment = usize::try_from(FRAME_PROGRAM_HOT_ALIGNMENT_V1)
            .map_err(|_| internal("alignment exceeds usize"))?;
        cursor = cursor
            .checked_add((alignment - cursor % alignment) % alignment)
            .ok_or_else(|| internal("table alignment overflow"))?;
        let offset = checked_u32(cursor, "table offset")?;
        let byte_len = checked_u32(bytes.len(), "table byte length")?;
        cursor = cursor
            .checked_add(bytes.len())
            .ok_or_else(|| internal("table end overflow"))?;
        directory.push(DirectoryEntry {
            kind: *kind,
            count: *count,
            offset,
            byte_len,
        });
    }
    let max = usize::try_from(FRAME_PROGRAM_MAX_BYTES_V1).unwrap_or(usize::MAX);
    if cursor > max {
        return Err(internal(format!(
            "frame program needs {cursor} bytes, exceeding {max}"
        )));
    }
    let total_bytes = checked_u32(cursor, "total bytes")?;

    let mut writer = Writer::default();
    writer.bytes(&FRAME_PROGRAM_MAGIC_V1);
    writer.u16(FRAME_PROGRAM_VERSION_V1);
    writer.u16(FRAME_PROGRAM_HEADER_BYTES_V1);
    writer.u32(program.flags);
    writer.u32(total_bytes);
    writer.u16(program.renderer_index);
    writer.u16(0);
    writer.u32(program.numeric_revision);
    writer.u32(program.formal_revision);
    writer.u16(FrameProgramTableKindV1::REQUIRED_COUNT);
    writer.bytes(&[0; 14]);
    writer.bytes(&[0; FRAME_PROGRAM_DIGEST_BYTES_V1]);
    for entry in &directory {
        writer.u16(entry.kind.code());
        writer.u16(entry.kind.record_bytes());
        writer.u32(entry.count);
        writer.u32(entry.offset);
        writer.u32(entry.byte_len);
    }
    for ((_, payload, count), entry) in table_payloads.iter().zip(&directory) {
        if *count == 0 {
            continue;
        }
        writer.align(
            usize::try_from(FRAME_PROGRAM_HOT_ALIGNMENT_V1)
                .map_err(|_| internal("alignment exceeds usize"))?,
        )?;
        if writer.bytes.len() != entry.offset as usize {
            return Err(internal("two-pass table offset mismatch"));
        }
        writer.bytes(payload);
    }
    if writer.bytes.len() != cursor {
        return Err(internal("two-pass total byte mismatch"));
    }
    let digest = wrela_machine::sha256::sha256(&writer.bytes);
    writer.bytes[FRAME_PROGRAM_DIGEST_OFFSET_V1
        ..FRAME_PROGRAM_DIGEST_OFFSET_V1 + FRAME_PROGRAM_DIGEST_BYTES_V1]
        .copy_from_slice(&digest);
    binary_verify::verify_envelope(&writer.bytes).map_err(|error| {
        internal(format!(
            "byte-level verifier rejected compiler output: {error}"
        ))
    })?;
    Ok(writer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_deterministic_aligned_and_round_trips() {
        let program = crate::pixels::program::minimal_verified_frame_program();
        let first = encode(&program).unwrap();
        let second = encode(&program).unwrap();
        assert_eq!(first, second);
        let decoded = crate::pixels::decode::decode(&first).unwrap();
        assert_eq!(decoded, program);
        let tables = binary_verify::verify_envelope(&first).unwrap();
        for table in tables.iter().filter(|table| table.count != 0) {
            assert_eq!(u64::from(table.offset) % FRAME_PROGRAM_HOT_ALIGNMENT_V1, 0);
        }
        let directory_end = usize::from(FRAME_PROGRAM_HEADER_BYTES_V1)
            + usize::from(FrameProgramTableKindV1::REQUIRED_COUNT)
                * usize::from(FRAME_PROGRAM_TABLE_BYTES_V1);
        let mut cursor = directory_end;
        for table in tables.iter().filter(|table| table.count != 0) {
            assert!(
                first[cursor..table.offset as usize]
                    .iter()
                    .all(|byte| *byte == 0)
            );
            cursor = (table.offset + table.byte_len) as usize;
        }
        assert_eq!(
            wrela_machine::sha256::sha256_hex(&first),
            "0211bbf0b14a6b786a348d503ee6ca014d0823f72eaabd0d314aca24a1605ec3"
        );
    }

    #[test]
    fn predeclared_future_tables_are_canonical_empty_entries() {
        let bytes = encode(&crate::pixels::program::minimal_verified_frame_program()).unwrap();
        let tables = binary_verify::verify_envelope(&bytes).unwrap();
        for kind in [
            FrameProgramTableKindV1::Transparency,
            FrameProgramTableKindV1::Probe,
            FrameProgramTableKindV1::Kinetic,
            FrameProgramTableKindV1::DebugName,
        ] {
            let table = tables.iter().find(|table| table.kind == kind).unwrap();
            assert_eq!((table.count, table.offset, table.byte_len), (0, 0, 0));
        }
    }

    #[test]
    fn checked_conversions_and_alignment_fail_without_truncation() {
        assert!(checked_u16(usize::from(u16::MAX), "test").is_ok());
        assert!(checked_u16(usize::from(u16::MAX) + 1, "test").is_err());
        assert!(checked_u32(u32::MAX as usize, "test").is_ok());
        if usize::BITS > u32::BITS {
            assert!(checked_u32(u32::MAX as usize + 1, "test").is_err());
        }
        let mut writer = Writer::default();
        assert!(writer.align(0).is_err());
        assert!(writer.align(3).is_err());
    }
}
