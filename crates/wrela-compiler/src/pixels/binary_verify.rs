//! Hostile byte-envelope verification shared by the encoder and decoder.

use std::ops::Range;

use super::decode::DecodeError;
use super::version::{
    FRAME_PROGRAM_DIGEST_BYTES_V1, FRAME_PROGRAM_DIGEST_OFFSET_V1, FRAME_PROGRAM_HEADER_BYTES_V1,
    FRAME_PROGRAM_HOT_ALIGNMENT_V1, FRAME_PROGRAM_MAGIC_V1, FRAME_PROGRAM_MAX_BYTES_V1,
    FRAME_PROGRAM_TABLE_BYTES_V1, FRAME_PROGRAM_VERSION_V1, FrameProgramTableKindV1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireTable {
    pub kind: FrameProgramTableKindV1,
    pub record_bytes: u16,
    pub count: u32,
    pub offset: u32,
    pub byte_len: u32,
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        bytes[offset..offset + 2]
            .try_into()
            .expect("checked header"),
    )
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("checked header"),
    )
}

pub fn verify_envelope(bytes: &[u8]) -> Result<Vec<WireTable>, DecodeError> {
    let cap = usize::try_from(FRAME_PROGRAM_MAX_BYTES_V1).unwrap_or(usize::MAX);
    if bytes.len() > cap {
        return Err(DecodeError::ByteCap {
            found: bytes.len(),
            maximum: cap,
        });
    }
    if bytes.len() < usize::from(FRAME_PROGRAM_HEADER_BYTES_V1) {
        return Err(DecodeError::Truncated {
            needed: usize::from(FRAME_PROGRAM_HEADER_BYTES_V1),
            found: bytes.len(),
        });
    }
    if bytes[0..8] != FRAME_PROGRAM_MAGIC_V1 {
        return Err(DecodeError::WrongMagic);
    }
    if u16_at(bytes, 8) != FRAME_PROGRAM_VERSION_V1 {
        return Err(DecodeError::WrongVersion(u16_at(bytes, 8)));
    }
    if u16_at(bytes, 10) != FRAME_PROGRAM_HEADER_BYTES_V1 {
        return Err(DecodeError::WrongHeaderBytes(u16_at(bytes, 10)));
    }
    if u16_at(bytes, 22) != 0 || bytes[34..48].iter().any(|byte| *byte != 0) {
        return Err(DecodeError::ReservedNonzero);
    }
    let total_bytes = usize::try_from(u32_at(bytes, 16))
        .map_err(|_| DecodeError::IntegerOverflow("total_bytes"))?;
    if total_bytes != bytes.len() {
        return Err(DecodeError::TotalBytes {
            header: total_bytes,
            actual: bytes.len(),
        });
    }
    let table_count = u16_at(bytes, 32);
    if table_count != FrameProgramTableKindV1::REQUIRED_COUNT {
        return Err(DecodeError::TableCount(table_count));
    }
    let directory_bytes = usize::from(table_count)
        .checked_mul(usize::from(FRAME_PROGRAM_TABLE_BYTES_V1))
        .ok_or(DecodeError::IntegerOverflow("directory bytes"))?;
    let directory_end = usize::from(FRAME_PROGRAM_HEADER_BYTES_V1)
        .checked_add(directory_bytes)
        .ok_or(DecodeError::IntegerOverflow("directory end"))?;
    if directory_end > bytes.len() {
        return Err(DecodeError::Truncated {
            needed: directory_end,
            found: bytes.len(),
        });
    }

    let mut tables = Vec::with_capacity(usize::from(table_count));
    let mut occupied: Vec<Range<usize>> = Vec::new();
    let mut canonical_end = directory_end;
    for (index, expected_kind) in FrameProgramTableKindV1::ALL.into_iter().enumerate() {
        let at = usize::from(FRAME_PROGRAM_HEADER_BYTES_V1)
            + index * usize::from(FRAME_PROGRAM_TABLE_BYTES_V1);
        let code = u16_at(bytes, at);
        let Some(kind) = FrameProgramTableKindV1::from_code(code) else {
            return Err(DecodeError::UnknownTableKind(code));
        };
        if kind != expected_kind {
            return Err(DecodeError::NoncanonicalTableOrder {
                index,
                expected: expected_kind.code(),
                found: code,
            });
        }
        let record_bytes = u16_at(bytes, at + 2);
        if record_bytes != kind.record_bytes() {
            return Err(DecodeError::RecordBytes {
                kind: code,
                expected: kind.record_bytes(),
                found: record_bytes,
            });
        }
        let count = u32_at(bytes, at + 4);
        let offset = u32_at(bytes, at + 8);
        let byte_len = u32_at(bytes, at + 12);
        if count == 0 {
            if offset != 0 || byte_len != 0 {
                return Err(DecodeError::NoncanonicalEmptyTable(code));
            }
        } else {
            let expected_len = count
                .checked_mul(u32::from(record_bytes))
                .ok_or(DecodeError::IntegerOverflow("table byte_len"))?;
            if byte_len != expected_len {
                return Err(DecodeError::TableLength {
                    kind: code,
                    expected: expected_len,
                    found: byte_len,
                });
            }
            if u64::from(offset) % FRAME_PROGRAM_HOT_ALIGNMENT_V1 != 0 {
                return Err(DecodeError::MisalignedTable { kind: code, offset });
            }
            let start = usize::try_from(offset)
                .map_err(|_| DecodeError::IntegerOverflow("table offset"))?;
            let end = start
                .checked_add(
                    usize::try_from(byte_len)
                        .map_err(|_| DecodeError::IntegerOverflow("table byte_len"))?,
                )
                .ok_or(DecodeError::IntegerOverflow("table end"))?;
            if start < directory_end || end > bytes.len() {
                return Err(DecodeError::TableBounds {
                    kind: code,
                    offset,
                    byte_len,
                });
            }
            if occupied
                .iter()
                .any(|range| start < range.end && range.start < end)
            {
                return Err(DecodeError::OverlappingTable(code));
            }
            let canonical_start = canonical_end
                .checked_add(
                    usize::try_from(FRAME_PROGRAM_HOT_ALIGNMENT_V1 - 1)
                        .map_err(|_| DecodeError::IntegerOverflow("table alignment"))?,
                )
                .map(|value| {
                    value
                        & !usize::try_from(FRAME_PROGRAM_HOT_ALIGNMENT_V1 - 1)
                            .expect("u64 alignment fits supported hosts")
                })
                .ok_or(DecodeError::IntegerOverflow("canonical table offset"))?;
            if start != canonical_start {
                return Err(DecodeError::NoncanonicalTableLayout(code));
            }
            occupied.push(start..end);
            canonical_end = end;
        }
        tables.push(WireTable {
            kind,
            record_bytes,
            count,
            offset,
            byte_len,
        });
    }
    occupied.sort_by_key(|range| range.start);
    let mut cursor = directory_end;
    for range in &occupied {
        if bytes[cursor..range.start].iter().any(|byte| *byte != 0) {
            return Err(DecodeError::PaddingNonzero);
        }
        cursor = range.end;
    }
    if cursor != bytes.len() {
        return Err(DecodeError::TrailingBytes);
    }

    let mut digest_input = bytes.to_vec();
    let found: [u8; FRAME_PROGRAM_DIGEST_BYTES_V1] = digest_input[FRAME_PROGRAM_DIGEST_OFFSET_V1
        ..FRAME_PROGRAM_DIGEST_OFFSET_V1 + FRAME_PROGRAM_DIGEST_BYTES_V1]
        .try_into()
        .expect("checked header");
    digest_input[FRAME_PROGRAM_DIGEST_OFFSET_V1
        ..FRAME_PROGRAM_DIGEST_OFFSET_V1 + FRAME_PROGRAM_DIGEST_BYTES_V1]
        .fill(0);
    let expected = wrela_machine::sha256::sha256(&digest_input);
    if found != expected {
        return Err(DecodeError::DigestMismatch);
    }
    Ok(tables)
}
