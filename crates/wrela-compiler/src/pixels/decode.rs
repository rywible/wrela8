//! Bounds-checked, allocation-capped FrameProgram v1 host decoder.

use std::fmt;

use super::binary_verify;
use super::program::{FrameProgram, FrameRecord, FrameTable, VerifiedFrameProgram};
use super::version::{
    FRAME_PROGRAM_FORMAL_REVISION_V1, FRAME_PROGRAM_NUMERIC_REVISION_V1,
    FRAME_PROGRAM_RECORD_BYTES_V1, FrameProgramTableKindV1,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    ByteCap {
        found: usize,
        maximum: usize,
    },
    Truncated {
        needed: usize,
        found: usize,
    },
    WrongMagic,
    WrongVersion(u16),
    WrongHeaderBytes(u16),
    WrongRevision {
        numeric: u32,
        formal: u32,
    },
    ReservedNonzero,
    TotalBytes {
        header: usize,
        actual: usize,
    },
    TableCount(u16),
    UnknownTableKind(u16),
    NoncanonicalTableOrder {
        index: usize,
        expected: u16,
        found: u16,
    },
    RecordBytes {
        kind: u16,
        expected: u16,
        found: u16,
    },
    NoncanonicalEmptyTable(u16),
    TableLength {
        kind: u16,
        expected: u32,
        found: u32,
    },
    MisalignedTable {
        kind: u16,
        offset: u32,
    },
    TableBounds {
        kind: u16,
        offset: u32,
        byte_len: u32,
    },
    OverlappingTable(u16),
    NoncanonicalTableLayout(u16),
    PaddingNonzero,
    TrailingBytes,
    DigestMismatch,
    IntegerOverflow(&'static str),
    RecordReservedNonzero {
        kind: u16,
        index: u32,
    },
    ImmediateOwner {
        index: u32,
    },
    ImmediateAliased {
        index: u32,
    },
    ImmediateUnreferenced {
        index: u32,
    },
    OperandBounds {
        kind: u16,
        record: u32,
    },
    Semantic(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        bytes[offset..offset + 2]
            .try_into()
            .expect("verified range"),
    )
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("verified range"),
    )
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("verified range"),
    )
}

#[derive(Clone, Copy)]
struct Immediate {
    owner_kind: u16,
    owner_id: u32,
    ordinal: u32,
    value: u64,
}

pub fn decode(bytes: &[u8]) -> Result<VerifiedFrameProgram, DecodeError> {
    let wire_tables = binary_verify::verify_envelope(bytes)?;
    let numeric_revision = u32_at(bytes, 24);
    let formal_revision = u32_at(bytes, 28);
    if numeric_revision != FRAME_PROGRAM_NUMERIC_REVISION_V1
        || formal_revision != FRAME_PROGRAM_FORMAL_REVISION_V1
    {
        return Err(DecodeError::WrongRevision {
            numeric: numeric_revision,
            formal: formal_revision,
        });
    }
    let immediate_table = wire_tables
        .iter()
        .find(|table| table.kind == FrameProgramTableKindV1::Immediate)
        .expect("required namespace");
    let mut immediates = Vec::with_capacity(immediate_table.count as usize);
    for index in 0..immediate_table.count {
        let at = immediate_table.offset as usize
            + index as usize * usize::from(immediate_table.record_bytes);
        if u16_at(bytes, at + 2) != 0 || u32_at(bytes, at + 12) != 0 {
            return Err(DecodeError::RecordReservedNonzero {
                kind: FrameProgramTableKindV1::Immediate.code(),
                index,
            });
        }
        immediates.push(Immediate {
            owner_kind: u16_at(bytes, at),
            owner_id: u32_at(bytes, at + 4),
            ordinal: u32_at(bytes, at + 8),
            value: u64_at(bytes, at + 16),
        });
    }

    let mut tables = Vec::with_capacity(wire_tables.len());
    let mut used_immediates = vec![false; immediates.len()];
    for table in wire_tables {
        if table.kind == FrameProgramTableKindV1::Immediate {
            tables.push(FrameTable {
                kind: table.kind,
                records: Vec::new(),
            });
            continue;
        }
        if table.record_bytes != FRAME_PROGRAM_RECORD_BYTES_V1 {
            return Err(DecodeError::RecordBytes {
                kind: table.kind.code(),
                expected: FRAME_PROGRAM_RECORD_BYTES_V1,
                found: table.record_bytes,
            });
        }
        let mut records = Vec::with_capacity(table.count as usize);
        for index in 0..table.count {
            let at = table.offset as usize + index as usize * usize::from(table.record_bytes);
            let stable_id = u32_at(bytes, at);
            let tag = u16_at(bytes, at + 4);
            let flags = u16_at(bytes, at + 6);
            let operand_offset = u32_at(bytes, at + 8);
            let operand_count = u16_at(bytes, at + 12);
            if u16_at(bytes, at + 14) != 0 {
                return Err(DecodeError::RecordReservedNonzero {
                    kind: table.kind.code(),
                    index,
                });
            }
            let end = operand_offset
                .checked_add(u32::from(operand_count))
                .ok_or(DecodeError::IntegerOverflow("operand end"))?;
            let operands = immediates
                .get(operand_offset as usize..end as usize)
                .ok_or(DecodeError::OperandBounds {
                    kind: table.kind.code(),
                    record: index,
                })?;
            let mut values = Vec::with_capacity(operands.len());
            for (ordinal, immediate) in operands.iter().enumerate() {
                let immediate_index = operand_offset as usize + ordinal;
                if std::mem::replace(&mut used_immediates[immediate_index], true) {
                    return Err(DecodeError::ImmediateAliased {
                        index: operand_offset + ordinal as u32,
                    });
                }
                if immediate.owner_kind != table.kind.code()
                    || immediate.owner_id != stable_id
                    || immediate.ordinal != ordinal as u32
                {
                    return Err(DecodeError::ImmediateOwner {
                        index: operand_offset + ordinal as u32,
                    });
                }
                values.push(immediate.value);
            }
            records.push(FrameRecord {
                stable_id,
                tag,
                flags,
                operands: values,
            });
        }
        tables.push(FrameTable {
            kind: table.kind,
            records,
        });
    }
    if let Some(index) = used_immediates.iter().position(|used| !used) {
        return Err(DecodeError::ImmediateUnreferenced {
            index: u32::try_from(index)
                .map_err(|_| DecodeError::IntegerOverflow("unreferenced immediate index"))?,
        });
    }
    let program = FrameProgram {
        renderer_index: u16_at(bytes, 20),
        flags: u32_at(bytes, 12),
        numeric_revision,
        formal_revision,
        tables,
    };
    super::verify::check_program(program).map_err(DecodeError::Semantic)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Vec<u8> {
        crate::pixels::encode::encode(&crate::pixels::program::minimal_verified_frame_program())
            .unwrap()
    }

    fn rehash(bytes: &mut [u8]) {
        bytes[super::super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
            ..super::super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
                + super::super::version::FRAME_PROGRAM_DIGEST_BYTES_V1]
            .fill(0);
        let digest = wrela_machine::sha256::sha256(bytes);
        bytes[super::super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
            ..super::super::version::FRAME_PROGRAM_DIGEST_OFFSET_V1
                + super::super::version::FRAME_PROGRAM_DIGEST_BYTES_V1]
            .copy_from_slice(&digest);
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn every_truncation_is_a_structured_error() {
        let bytes = valid();
        for end in 0..bytes.len() {
            assert!(
                decode(&bytes[..end]).is_err(),
                "accepted truncation at byte {end}"
            );
        }
    }

    #[test]
    fn header_magic_version_reserved_and_digest_corruption_fail() {
        for offset in [0, 8, 10, 22, 34, 47, 48, 79] {
            let mut bytes = valid();
            bytes[offset] ^= 1;
            assert!(decode(&bytes).is_err(), "accepted corruption at {offset}");
        }
    }

    #[test]
    fn every_header_and_directory_field_is_verified_after_rehash() {
        for offset in [8, 10, 12, 16, 22, 24, 28, 32, 34] {
            let mut bytes = valid();
            bytes[offset] ^= if offset == 12 { 2 } else { 1 };
            rehash(&mut bytes);
            assert!(
                decode(&bytes).is_err(),
                "accepted rehashed header mutation at {offset}"
            );
        }
        let mut renderer = valid();
        put_u16(&mut renderer, 20, 7);
        rehash(&mut renderer);
        assert_eq!(decode(&renderer).unwrap().program().renderer_index, 7);

        let base = usize::from(super::super::version::FRAME_PROGRAM_HEADER_BYTES_V1);
        let stride = usize::from(super::super::version::FRAME_PROGRAM_TABLE_BYTES_V1);
        let tables = binary_verify::verify_envelope(&valid()).unwrap();
        for (index, table) in tables.iter().enumerate() {
            let at = base + index * stride;
            for field in 0..5 {
                let mut bytes = valid();
                match field {
                    0 => put_u16(&mut bytes, at, u16::MAX),
                    1 => put_u16(&mut bytes, at + 2, table.record_bytes.wrapping_add(1)),
                    2 => put_u32(&mut bytes, at + 4, table.count.wrapping_add(1)),
                    3 => put_u32(&mut bytes, at + 8, table.offset.wrapping_add(1)),
                    4 => put_u32(&mut bytes, at + 12, table.byte_len.wrapping_add(1)),
                    _ => unreachable!(),
                }
                rehash(&mut bytes);
                assert!(
                    decode(&bytes).is_err(),
                    "accepted directory mutation kind={} field={field}",
                    table.kind.stable_name()
                );
            }
        }
    }

    #[test]
    fn directory_overlap_misalignment_and_overflow_fail_before_records() {
        let base = usize::from(super::super::version::FRAME_PROGRAM_HEADER_BYTES_V1);
        let table_bytes = usize::from(super::super::version::FRAME_PROGRAM_TABLE_BYTES_V1);
        let mut valid_bytes = valid();
        let nonempty = binary_verify::verify_envelope(&valid_bytes)
            .unwrap()
            .into_iter()
            .filter(|table| table.count != 0)
            .collect::<Vec<_>>();
        assert!(nonempty.len() >= 2);

        let mut misaligned = valid_bytes.clone();
        let index = usize::from(nonempty[0].kind.code() - 1);
        misaligned[base + index * table_bytes + 8..base + index * table_bytes + 12]
            .copy_from_slice(&(nonempty[0].offset + 1).to_le_bytes());
        rehash(&mut misaligned);
        assert!(matches!(
            decode(&misaligned),
            Err(DecodeError::MisalignedTable { .. })
        ));

        let mut overlap = valid_bytes.clone();
        let first = nonempty[0];
        let second = nonempty[1];
        let second_index = usize::from(second.kind.code() - 1);
        overlap[base + second_index * table_bytes + 8..base + second_index * table_bytes + 12]
            .copy_from_slice(&first.offset.to_le_bytes());
        rehash(&mut overlap);
        assert!(matches!(
            decode(&overlap),
            Err(DecodeError::OverlappingTable(_))
        ));

        let mut overflow = valid_bytes.clone();
        let first_index = usize::from(first.kind.code() - 1);
        overflow[base + first_index * table_bytes + 4..base + first_index * table_bytes + 8]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        rehash(&mut overflow);
        assert!(matches!(
            decode(&overflow),
            Err(DecodeError::IntegerOverflow("table byte_len"))
        ));

        valid_bytes.push(0);
        assert!(matches!(
            decode(&valid_bytes),
            Err(DecodeError::TotalBytes { .. })
        ));
    }

    #[test]
    fn noncanonical_record_and_immediate_fields_fail_closed() {
        let bytes = valid();
        let tables = binary_verify::verify_envelope(&bytes).unwrap();
        let scalar = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Scalar)
            .unwrap();
        let immediate = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Immediate)
            .unwrap();

        let mut record_reserved = bytes.clone();
        record_reserved[scalar.offset as usize + 14] = 1;
        rehash(&mut record_reserved);
        assert!(matches!(
            decode(&record_reserved),
            Err(DecodeError::RecordReservedNonzero { .. })
        ));

        let mut immediate_owner = bytes.clone();
        immediate_owner[immediate.offset as usize] ^= 1;
        rehash(&mut immediate_owner);
        assert!(matches!(
            decode(&immediate_owner),
            Err(DecodeError::ImmediateOwner { .. })
        ));
    }

    #[test]
    fn every_populated_record_header_and_immediate_reserved_field_is_verified() {
        let original = valid();
        let tables = binary_verify::verify_envelope(&original).unwrap();
        for table in tables
            .iter()
            .filter(|table| table.count != 0 && table.kind != FrameProgramTableKindV1::Immediate)
        {
            for (field, offset) in [("tag", 4_usize), ("flags", 6), ("reserved", 14)] {
                let mut bytes = original.clone();
                put_u16(&mut bytes, table.offset as usize + offset, u16::MAX);
                rehash(&mut bytes);
                assert!(
                    decode(&bytes).is_err(),
                    "accepted {} {field} mutation",
                    table.kind.stable_name()
                );
            }
        }
        let immediate = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Immediate)
            .unwrap();
        for offset in [2_usize, 12] {
            let mut bytes = original.clone();
            bytes[immediate.offset as usize + offset] = 1;
            rehash(&mut bytes);
            assert!(
                decode(&bytes).is_err(),
                "accepted immediate reserved mutation at {offset}"
            );
        }
    }

    #[test]
    #[ignore = "exhaustive wire mutation corpus belongs in verify-deep"]
    fn deterministic_single_bit_wire_field_corpus_is_complete_and_fail_closed() {
        use std::collections::BTreeSet;

        let original = valid();
        let tables = binary_verify::verify_envelope(&original).unwrap();
        let original_program = decode(&original).unwrap();
        let header_bytes = usize::from(super::super::version::FRAME_PROGRAM_HEADER_BYTES_V1);
        let directory_bytes = usize::from(FrameProgramTableKindV1::REQUIRED_COUNT)
            * usize::from(super::super::version::FRAME_PROGRAM_TABLE_BYTES_V1);
        let mut field_bytes = BTreeSet::new();
        field_bytes.extend(0..header_bytes + directory_bytes);
        for table in tables.iter().filter(|table| table.count != 0) {
            field_bytes.extend(
                usize::try_from(table.offset).unwrap()
                    ..usize::try_from(table.offset + table.byte_len).unwrap(),
            );
        }
        let expected_field_bytes = header_bytes
            + directory_bytes
            + tables
                .iter()
                .map(|table| usize::try_from(table.byte_len).unwrap())
                .sum::<usize>();
        assert_eq!(field_bytes.len(), expected_field_bytes);

        let mut mutations = 0_usize;
        for byte in field_bytes {
            for bit in 0..8 {
                let mut mutated = original.clone();
                mutated[byte] ^= 1 << bit;
                assert!(
                    decode(&mutated).is_err(),
                    "accepted digest-sealed single-bit mutation at byte {byte} bit {bit}"
                );
                mutations += 1;
            }
        }
        assert_eq!(mutations, expected_field_bytes * 8);

        let mut strict_header_bits = 0_usize;
        for range in [0..12, 16..20, 22..48] {
            for byte in range {
                for bit in 0..8 {
                    let mut mutated = original.clone();
                    mutated[byte] ^= 1 << bit;
                    rehash(&mut mutated);
                    assert!(
                        decode(&mutated).is_err(),
                        "accepted rehashed strict header mutation at byte {byte} bit {bit}"
                    );
                    strict_header_bits += 1;
                }
            }
        }
        assert_eq!(strict_header_bits, (12 + 4 + 26) * 8);
        for bit in 0..32 {
            let mut mutated = original.clone();
            mutated[12 + bit / 8] ^= 1 << (bit % 8);
            rehash(&mut mutated);
            if bit == 0 {
                let decoded = decode(&mutated).expect("defined frame-program flag");
                assert_ne!(decoded, original_program);
            } else {
                assert!(
                    decode(&mutated).is_err(),
                    "accepted unknown frame-program flag bit {bit}"
                );
            }
        }
        for bit in 0..16 {
            let mut mutated = original.clone();
            mutated[20 + bit / 8] ^= 1 << (bit % 8);
            rehash(&mut mutated);
            let decoded = decode(&mutated).expect("renderer index is payload");
            assert_ne!(decoded, original_program);
        }

        let directory_start = header_bytes;
        let mut rehashed_directory_mutations = 0_usize;
        for byte in directory_start..directory_start + directory_bytes {
            for bit in 0..8 {
                let mut mutated = original.clone();
                mutated[byte] ^= 1 << bit;
                rehash(&mut mutated);
                assert!(
                    decode(&mutated).is_err(),
                    "accepted rehashed directory mutation at byte {byte} bit {bit}"
                );
                rehashed_directory_mutations += 1;
            }
        }
        assert_eq!(rehashed_directory_mutations, directory_bytes * 8);

        let mut tag_bits = 0_usize;
        let mut flag_bits = 0_usize;
        let mut reserved_bits = 0_usize;
        let mut populated_records = 0_usize;
        for table in tables
            .iter()
            .filter(|table| table.count != 0 && table.kind != FrameProgramTableKindV1::Immediate)
        {
            for record in 0..table.count {
                populated_records += 1;
                let at = usize::try_from(table.offset).unwrap()
                    + usize::try_from(record).unwrap() * usize::from(table.record_bytes);
                for (field, offset, covered) in [
                    ("tag", 4_usize, &mut tag_bits),
                    ("flags", 6_usize, &mut flag_bits),
                ] {
                    for bit in 0..16 {
                        let mut mutated = original.clone();
                        mutated[at + offset + bit / 8] ^= 1 << (bit % 8);
                        rehash(&mut mutated);
                        if let Ok(decoded) = decode(&mutated) {
                            assert_ne!(
                                decoded,
                                original_program,
                                "single-bit {field} mutation was silently ignored for {} record {record}",
                                table.kind.stable_name()
                            );
                        }
                        *covered += 1;
                    }
                }
                for bit in 0..16 {
                    let mut mutated = original.clone();
                    mutated[at + 14 + bit / 8] ^= 1 << (bit % 8);
                    rehash(&mut mutated);
                    assert!(
                        matches!(
                            decode(&mutated),
                            Err(DecodeError::RecordReservedNonzero { .. })
                        ),
                        "accepted reserved bit {bit} for {} record {record}",
                        table.kind.stable_name()
                    );
                    reserved_bits += 1;
                }
            }
        }
        assert_eq!(tag_bits, populated_records * 16);
        assert_eq!(flag_bits, populated_records * 16);
        assert_eq!(reserved_bits, populated_records * 16);

        let immediate = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Immediate)
            .unwrap();
        let mut immediate_owner_kind_bits = 0_usize;
        let mut immediate_reserved_bits = 0_usize;
        for record in 0..immediate.count {
            let at = usize::try_from(immediate.offset).unwrap()
                + usize::try_from(record).unwrap() * usize::from(immediate.record_bytes);
            for bit in 0..16 {
                let mut mutated = original.clone();
                mutated[at + bit / 8] ^= 1 << (bit % 8);
                rehash(&mut mutated);
                assert!(
                    decode(&mutated).is_err(),
                    "accepted immediate owner-kind bit {bit} for record {record}"
                );
                immediate_owner_kind_bits += 1;
            }
            for (offset, bits) in [(2_usize, 16_usize), (12_usize, 32_usize)] {
                for bit in 0..bits {
                    let mut mutated = original.clone();
                    mutated[at + offset + bit / 8] ^= 1 << (bit % 8);
                    rehash(&mut mutated);
                    assert!(
                        matches!(
                            decode(&mutated),
                            Err(DecodeError::RecordReservedNonzero { .. })
                        ),
                        "accepted immediate reserved bit {bit} at field offset {offset} for record {record}"
                    );
                    immediate_reserved_bits += 1;
                }
            }
        }
        assert_eq!(
            immediate_owner_kind_bits,
            usize::try_from(immediate.count).unwrap() * 16
        );
        assert_eq!(
            immediate_reserved_bits,
            usize::try_from(immediate.count).unwrap() * 48
        );
    }

    #[test]
    fn rehashed_operand_mutation_reaches_semantic_shape_verification() {
        let mut bytes = valid();
        let tables = binary_verify::verify_envelope(&bytes).unwrap();
        let object = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Object)
            .unwrap();
        let immediate = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Immediate)
            .unwrap();
        let operand_offset = u32_at(&bytes, object.offset as usize + 8);
        let primitive_count_value = immediate.offset as usize
            + (operand_offset as usize + 4) * usize::from(immediate.record_bytes)
            + 16;
        put_u64(&mut bytes, primitive_count_value, u64::MAX);
        rehash(&mut bytes);
        assert!(matches!(
            decode(&bytes),
            Err(DecodeError::Semantic(message))
                if message.contains("invalid numeric encoding")
                    || message.contains("primitive occurrence")
        ));
    }

    #[test]
    fn rehashed_nonfinite_and_wide_f32_constants_fail_closed() {
        for poisoned in [u64::from(f32::NAN.to_bits()), u64::MAX] {
            let mut bytes = valid();
            let tables = binary_verify::verify_envelope(&bytes).unwrap();
            let scalar = tables
                .iter()
                .find(|table| table.kind == FrameProgramTableKindV1::Scalar)
                .unwrap();
            let immediate = tables
                .iter()
                .find(|table| table.kind == FrameProgramTableKindV1::Immediate)
                .unwrap();
            assert_eq!(u16_at(&bytes, scalar.offset as usize + 4), 1);
            let operand_offset = u32_at(&bytes, scalar.offset as usize + 8);
            let value = immediate.offset as usize
                + (operand_offset as usize + 1) * usize::from(immediate.record_bytes)
                + 16;
            put_u64(&mut bytes, value, poisoned);
            rehash(&mut bytes);
            assert!(
                matches!(decode(&bytes), Err(DecodeError::Semantic(_))),
                "accepted poisoned f32 word {poisoned:#x}"
            );
        }
    }

    #[test]
    fn sealed_variable_shape_ids_and_transform_depth_fail_closed() {
        let base = crate::pixels::program::minimal_verified_frame_program()
            .program()
            .clone();
        let sealed = |program| {
            crate::pixels::encode::encode(&crate::pixels::program::VerifiedFrameProgram::new(
                program,
            ))
            .unwrap()
        };

        let mut transformed = base.clone();
        let fields = &mut transformed
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::Field)
            .unwrap()
            .records;
        fields.push(crate::pixels::program::FrameRecord {
            stable_id: fields.len() as u32,
            tag: 27,
            flags: 0,
            operands: vec![0, 0, 1, 0, 0, 0],
        });
        let verified = crate::pixels::verify::check_program(transformed.clone()).unwrap();
        assert!(decode(&crate::pixels::encode::encode(&verified).unwrap()).is_ok());

        transformed
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::Field)
            .unwrap()
            .records
            .last_mut()
            .unwrap()
            .operands[4] = u64::MAX;
        assert!(matches!(
            decode(&sealed(transformed)),
            Err(DecodeError::Semantic(message)) if message.contains("transform names scalar")
        ));

        let mut too_deep = base.clone();
        let mut transform = vec![1, 0, 0, 0];
        for _ in 0..=wrela_machine::pixels::FRAME_PROGRAM_TRANSFORM_DEPTH_MAX_V1 {
            let mut wrapper = vec![6, 0, transform.len() as u64];
            wrapper.extend(transform);
            transform = wrapper;
        }
        let fields = &mut too_deep
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::Field)
            .unwrap()
            .records;
        let mut operands = vec![0, 0];
        operands.extend(transform);
        fields.push(crate::pixels::program::FrameRecord {
            stable_id: fields.len() as u32,
            tag: 27,
            flags: 0,
            operands,
        });
        assert!(matches!(
            decode(&sealed(too_deep)),
            Err(DecodeError::Semantic(message)) if message.contains("nesting exceeds")
        ));

        let mut bad_path = base;
        let object = bad_path
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::Object)
            .unwrap()
            .records
            .first_mut()
            .unwrap();
        object.operands[15] = u64::MAX;
        assert!(matches!(
            decode(&sealed(bad_path)),
            Err(DecodeError::Semantic(message)) if message.contains("primitive path names field")
        ));
    }

    #[test]
    fn canonical_layout_rejects_trailing_and_unowned_immediates() {
        let mut trailing = valid();
        trailing.extend([0; 64]);
        let trailing_len = u32::try_from(trailing.len()).unwrap();
        put_u32(&mut trailing, 16, trailing_len);
        rehash(&mut trailing);
        assert!(matches!(decode(&trailing), Err(DecodeError::TrailingBytes)));

        let mut unowned = valid();
        let tables = binary_verify::verify_envelope(&unowned).unwrap();
        let scalar = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Scalar)
            .unwrap();
        let immediate = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Immediate)
            .unwrap();
        put_u32(&mut unowned, scalar.offset as usize + 8, 1);
        put_u16(&mut unowned, scalar.offset as usize + 12, 1);
        put_u32(
            &mut unowned,
            immediate.offset as usize + usize::from(immediate.record_bytes) + 8,
            0,
        );
        rehash(&mut unowned);
        assert!(matches!(
            decode(&unowned),
            Err(DecodeError::ImmediateUnreferenced { index: 0 })
        ));
    }

    #[test]
    fn immediate_operands_cannot_alias_between_records() {
        let mut bytes = valid();
        let tables = binary_verify::verify_envelope(&bytes).unwrap();
        let field = tables
            .iter()
            .find(|table| table.kind == FrameProgramTableKindV1::Field)
            .unwrap();
        put_u32(&mut bytes, field.offset as usize + 8, 0);
        rehash(&mut bytes);
        assert!(matches!(
            decode(&bytes),
            Err(DecodeError::ImmediateAliased { index: 0 })
        ));
    }

    #[test]
    fn physically_reordered_nonoverlapping_tables_are_rejected() {
        let mut bytes = valid();
        let tables = binary_verify::verify_envelope(&bytes).unwrap();
        let nonempty = tables
            .iter()
            .filter(|table| table.count != 0)
            .collect::<Vec<_>>();
        assert!(nonempty.len() >= 2);
        let first = nonempty[0];
        let first_payload =
            bytes[first.offset as usize..(first.offset + first.byte_len) as usize].to_vec();
        bytes[first.offset as usize..(first.offset + first.byte_len) as usize].fill(0);
        let aligned = bytes.len().next_multiple_of(
            usize::try_from(super::super::version::FRAME_PROGRAM_HOT_ALIGNMENT_V1).unwrap(),
        );
        bytes.resize(aligned, 0);
        let relocated = u32::try_from(bytes.len()).unwrap();
        bytes.extend_from_slice(&first_payload);
        let total_bytes = u32::try_from(bytes.len()).unwrap();
        put_u32(&mut bytes, 16, total_bytes);
        let directory = usize::from(super::super::version::FRAME_PROGRAM_HEADER_BYTES_V1)
            + usize::from(first.kind.code() - 1)
                * usize::from(super::super::version::FRAME_PROGRAM_TABLE_BYTES_V1);
        put_u32(&mut bytes, directory + 8, relocated);
        rehash(&mut bytes);
        assert!(matches!(
            decode(&bytes),
            Err(DecodeError::NoncanonicalTableLayout(_))
        ));
    }
}
