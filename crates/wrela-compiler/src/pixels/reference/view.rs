//! Allocation-free, checked access to a sealed `FrameProgram v1` byte image.

use wrela_machine::pixels::{
    FRAME_PROGRAM_DIGEST_BYTES_V1, FRAME_PROGRAM_DIGEST_OFFSET_V1,
    FRAME_PROGRAM_FORMAL_REVISION_V1, FRAME_PROGRAM_HEADER_BYTES_V1, FRAME_PROGRAM_MAGIC_V1,
    FRAME_PROGRAM_NUMERIC_REVISION_V1, FRAME_PROGRAM_TABLE_BYTES_V1, FRAME_PROGRAM_VERSION_V1,
    FrameProgramTableKindV1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramViewError {
    Truncated,
    WrongMagic,
    WrongVersion,
    WrongDigest,
    WrongRenderer,
    WrongRevision,
    WrongFlags,
    WrongTableCount,
    ReservedNonzero,
    ProgramIndex,
    MalformedTable,
    MalformedRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SealedProgramContract {
    pub renderer_index: u16,
    pub flags: u32,
    pub digest: [u8; FRAME_PROGRAM_DIGEST_BYTES_V1],
    pub table_counts: [u32; FrameProgramTableKindV1::ALL.len()],
    pub table_offsets: [u32; FrameProgramTableKindV1::ALL.len()],
    pub table_byte_lens: [u32; FrameProgramTableKindV1::ALL.len()],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramHeader {
    pub renderer_index: u16,
    pub total_bytes: u32,
    pub digest: [u8; FRAME_PROGRAM_DIGEST_BYTES_V1],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordView<'a> {
    pub stable_id: u32,
    pub tag: u16,
    pub flags: u16,
    owner_kind: FrameProgramTableKindV1,
    operand_offset: u32,
    operand_count: u16,
    bytes: &'a [u8],
    immediate: TableView,
}

impl RecordView<'_> {
    pub fn operand(&self, index: usize) -> Result<u64, ProgramViewError> {
        if index >= usize::from(self.operand_count) {
            return Err(ProgramViewError::ProgramIndex);
        }
        let ordinal = self
            .operand_offset
            .checked_add(u32::try_from(index).map_err(|_| ProgramViewError::ProgramIndex)?)
            .ok_or(ProgramViewError::ProgramIndex)?;
        if ordinal >= self.immediate.count {
            return Err(ProgramViewError::MalformedRecord);
        }
        let at = table_record_offset(self.immediate, ordinal)?;
        let owner_kind = read_u16(self.bytes, at)?;
        let reserved0 = read_u16(self.bytes, at + 2)?;
        let owner_id = read_u32(self.bytes, at + 4)?;
        let found_ordinal = read_u32(self.bytes, at + 8)?;
        let reserved1 = read_u32(self.bytes, at + 12)?;
        if owner_kind != self.owner_kind.code()
            || owner_id != self.stable_id
            || found_ordinal != u32::try_from(index).map_err(|_| ProgramViewError::ProgramIndex)?
            || reserved0 != 0
            || reserved1 != 0
        {
            return Err(ProgramViewError::MalformedRecord);
        }
        read_u64(self.bytes, at + 16)
    }

    pub const fn operand_count(&self) -> u16 {
        self.operand_count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TableView {
    count: u32,
    offset: u32,
    byte_len: u32,
    record_bytes: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameProgramView<'a> {
    bytes: &'a [u8],
    header: ProgramHeader,
    immediate: TableView,
}

impl<'a> FrameProgramView<'a> {
    pub fn new(bytes: &'a [u8], contract: SealedProgramContract) -> Result<Self, ProgramViewError> {
        if bytes.len() < usize::from(FRAME_PROGRAM_HEADER_BYTES_V1) {
            return Err(ProgramViewError::Truncated);
        }
        if bytes.get(0..8) != Some(FRAME_PROGRAM_MAGIC_V1.as_slice()) {
            return Err(ProgramViewError::WrongMagic);
        }
        if read_u16(bytes, 8)? != FRAME_PROGRAM_VERSION_V1
            || read_u16(bytes, 10)? != FRAME_PROGRAM_HEADER_BYTES_V1
        {
            return Err(ProgramViewError::WrongVersion);
        }
        if read_u32(bytes, 12)? != contract.flags {
            return Err(ProgramViewError::WrongFlags);
        }
        let total_bytes = read_u32(bytes, 16)?;
        if usize::try_from(total_bytes).ok() != Some(bytes.len()) {
            return Err(ProgramViewError::Truncated);
        }
        let renderer_index = read_u16(bytes, 20)?;
        if renderer_index != contract.renderer_index {
            return Err(ProgramViewError::WrongRenderer);
        }
        if read_u16(bytes, 22)? != 0 || bytes[34..48].iter().any(|byte| *byte != 0) {
            return Err(ProgramViewError::ReservedNonzero);
        }
        if read_u32(bytes, 24)? != FRAME_PROGRAM_NUMERIC_REVISION_V1
            || read_u32(bytes, 28)? != FRAME_PROGRAM_FORMAL_REVISION_V1
        {
            return Err(ProgramViewError::WrongRevision);
        }
        if read_u16(bytes, 32)? != FrameProgramTableKindV1::REQUIRED_COUNT {
            return Err(ProgramViewError::WrongTableCount);
        }
        let digest: [u8; FRAME_PROGRAM_DIGEST_BYTES_V1] = bytes[FRAME_PROGRAM_DIGEST_OFFSET_V1
            ..FRAME_PROGRAM_DIGEST_OFFSET_V1 + FRAME_PROGRAM_DIGEST_BYTES_V1]
            .try_into()
            .map_err(|_| ProgramViewError::Truncated)?;
        if digest != contract.digest {
            return Err(ProgramViewError::WrongDigest);
        }

        let view = Self {
            bytes,
            header: ProgramHeader {
                renderer_index,
                total_bytes,
                digest,
            },
            immediate: TableView {
                count: 0,
                offset: 0,
                byte_len: 0,
                record_bytes: 0,
            },
        };
        let mut immediate = None;
        for (index, kind) in FrameProgramTableKindV1::ALL.into_iter().enumerate() {
            let table = view.table_at(index, kind)?;
            if table.count != contract.table_counts[index]
                || table.offset != contract.table_offsets[index]
                || table.byte_len != contract.table_byte_lens[index]
            {
                return Err(ProgramViewError::MalformedTable);
            }
            if kind == FrameProgramTableKindV1::Immediate {
                immediate = Some(table);
            }
        }
        Ok(Self {
            immediate: immediate.ok_or(ProgramViewError::WrongTableCount)?,
            ..view
        })
    }

    pub const fn header(&self) -> ProgramHeader {
        self.header
    }

    pub fn count(&self, kind: FrameProgramTableKindV1) -> Result<u32, ProgramViewError> {
        Ok(self.table(kind)?.count)
    }

    pub fn record(
        &self,
        kind: FrameProgramTableKindV1,
        id: u32,
    ) -> Result<RecordView<'a>, ProgramViewError> {
        if kind == FrameProgramTableKindV1::Immediate {
            return Err(ProgramViewError::MalformedRecord);
        }
        let table = self.table(kind)?;
        if id >= table.count {
            return Err(ProgramViewError::ProgramIndex);
        }
        let at = table_record_offset(table, id)?;
        let stable_id = read_u32(self.bytes, at)?;
        let tag = read_u16(self.bytes, at + 4)?;
        let flags = read_u16(self.bytes, at + 6)?;
        let operand_offset = read_u32(self.bytes, at + 8)?;
        let operand_count = read_u16(self.bytes, at + 12)?;
        let reserved = read_u16(self.bytes, at + 14)?;
        if stable_id != id
            || reserved != 0
            || operand_offset
                .checked_add(u32::from(operand_count))
                .is_none_or(|end| end > self.immediate.count)
        {
            return Err(ProgramViewError::MalformedRecord);
        }
        Ok(RecordView {
            stable_id,
            tag,
            flags,
            owner_kind: kind,
            operand_offset,
            operand_count,
            bytes: self.bytes,
            immediate: self.immediate,
        })
    }

    fn table(&self, kind: FrameProgramTableKindV1) -> Result<TableView, ProgramViewError> {
        let index = FrameProgramTableKindV1::ALL
            .iter()
            .position(|candidate| *candidate == kind)
            .ok_or(ProgramViewError::ProgramIndex)?;
        self.table_at(index, kind)
    }

    fn table_at(
        &self,
        index: usize,
        expected: FrameProgramTableKindV1,
    ) -> Result<TableView, ProgramViewError> {
        let at = usize::from(FRAME_PROGRAM_HEADER_BYTES_V1)
            .checked_add(
                index
                    .checked_mul(usize::from(FRAME_PROGRAM_TABLE_BYTES_V1))
                    .ok_or(ProgramViewError::MalformedTable)?,
            )
            .ok_or(ProgramViewError::MalformedTable)?;
        let kind = FrameProgramTableKindV1::from_code(read_u16(self.bytes, at)?)
            .ok_or(ProgramViewError::MalformedTable)?;
        let record_bytes = read_u16(self.bytes, at + 2)?;
        let count = read_u32(self.bytes, at + 4)?;
        let offset = read_u32(self.bytes, at + 8)?;
        let byte_len = read_u32(self.bytes, at + 12)?;
        if kind != expected || record_bytes != kind.record_bytes() {
            return Err(ProgramViewError::MalformedTable);
        }
        if count == 0 {
            if offset != 0 || byte_len != 0 {
                return Err(ProgramViewError::MalformedTable);
            }
        } else {
            let expected_len = count
                .checked_mul(u32::from(record_bytes))
                .ok_or(ProgramViewError::MalformedTable)?;
            let end = offset
                .checked_add(byte_len)
                .ok_or(ProgramViewError::MalformedTable)?;
            if byte_len != expected_len
                || usize::try_from(end).map_or(true, |end| end > self.bytes.len())
            {
                return Err(ProgramViewError::MalformedTable);
            }
        }
        Ok(TableView {
            count,
            offset,
            byte_len,
            record_bytes,
        })
    }
}

fn table_record_offset(table: TableView, id: u32) -> Result<usize, ProgramViewError> {
    let relative = id
        .checked_mul(u32::from(table.record_bytes))
        .ok_or(ProgramViewError::ProgramIndex)?;
    if relative
        .checked_add(u32::from(table.record_bytes))
        .is_none_or(|end| end > table.byte_len)
    {
        return Err(ProgramViewError::ProgramIndex);
    }
    usize::try_from(
        table
            .offset
            .checked_add(relative)
            .ok_or(ProgramViewError::ProgramIndex)?,
    )
    .map_err(|_| ProgramViewError::ProgramIndex)
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, ProgramViewError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(at..at.checked_add(2).ok_or(ProgramViewError::Truncated)?)
            .ok_or(ProgramViewError::Truncated)?
            .try_into()
            .map_err(|_| ProgramViewError::Truncated)?,
    ))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, ProgramViewError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at.checked_add(4).ok_or(ProgramViewError::Truncated)?)
            .ok_or(ProgramViewError::Truncated)?
            .try_into()
            .map_err(|_| ProgramViewError::Truncated)?,
    ))
}

fn read_u64(bytes: &[u8], at: usize) -> Result<u64, ProgramViewError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(at..at.checked_add(8).ok_or(ProgramViewError::Truncated)?)
            .ok_or(ProgramViewError::Truncated)?
            .try_into()
            .map_err(|_| ProgramViewError::Truncated)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Vec<u8>, SealedProgramContract) {
        let encoded = crate::pixels::encode::encode(
            &crate::pixels::program::minimal_verified_frame_program(),
        )
        .unwrap();
        let digest = encoded[FRAME_PROGRAM_DIGEST_OFFSET_V1
            ..FRAME_PROGRAM_DIGEST_OFFSET_V1 + FRAME_PROGRAM_DIGEST_BYTES_V1]
            .try_into()
            .unwrap();
        let flags = u32::from_le_bytes(encoded[12..16].try_into().unwrap());
        let tables = crate::pixels::binary_verify::verify_envelope(&encoded).unwrap();
        let mut table_counts = [0; FrameProgramTableKindV1::ALL.len()];
        let mut table_offsets = [0; FrameProgramTableKindV1::ALL.len()];
        let mut table_byte_lens = [0; FrameProgramTableKindV1::ALL.len()];
        for (index, table) in tables.into_iter().enumerate() {
            table_counts[index] = table.count;
            table_offsets[index] = table.offset;
            table_byte_lens[index] = table.byte_len;
        }
        (
            encoded,
            SealedProgramContract {
                renderer_index: 0,
                flags,
                digest,
                table_counts,
                table_offsets,
                table_byte_lens,
            },
        )
    }

    #[test]
    fn sealed_view_reads_records_and_operands_without_allocation() {
        let (encoded, contract) = fixture();
        let view = FrameProgramView::new(&encoded, contract).unwrap();
        let record = view
            .record(FrameProgramTableKindV1::FixedDomain, 0)
            .unwrap();
        assert_eq!(record.stable_id, 0);
        assert_ne!(record.tag, 0);
        assert_eq!(record.operand(0).unwrap(), 1);
        assert_eq!(view.header().total_bytes as usize, encoded.len());
        assert_eq!(
            std::mem::size_of_val(&view),
            std::mem::size_of::<&[u8]>()
                + std::mem::size_of::<ProgramHeader>()
                + std::mem::size_of::<TableView>()
        );
    }

    #[test]
    fn sealed_view_fails_closed_on_index_contract_and_reserved_bytes() {
        let (encoded, contract) = fixture();
        let view = FrameProgramView::new(&encoded, contract).unwrap();
        assert_eq!(
            view.record(FrameProgramTableKindV1::Object, u32::MAX),
            Err(ProgramViewError::ProgramIndex)
        );

        let mut corrupt = encoded;
        corrupt[34] = 1;
        assert_eq!(
            FrameProgramView::new(&corrupt, contract).unwrap_err(),
            ProgramViewError::ReservedNonzero
        );
    }

    #[test]
    fn sealed_view_compares_stored_digest_instead_of_rehashing_guest_bytes() {
        let (mut encoded, contract) = fixture();
        encoded[FRAME_PROGRAM_DIGEST_OFFSET_V1] ^= 1;
        assert_eq!(
            FrameProgramView::new(&encoded, contract).unwrap_err(),
            ProgramViewError::WrongDigest
        );
    }

    #[test]
    fn generated_table_layout_is_part_of_the_guest_contract() {
        let (mut encoded, contract) = fixture();
        let first_offset = usize::from(FRAME_PROGRAM_HEADER_BYTES_V1) + 8;
        let changed =
            u32::from_le_bytes(encoded[first_offset..first_offset + 4].try_into().unwrap()) + 16;
        encoded[first_offset..first_offset + 4].copy_from_slice(&changed.to_le_bytes());
        assert_eq!(
            FrameProgramView::new(&encoded, contract).unwrap_err(),
            ProgramViewError::MalformedTable
        );
    }
}
