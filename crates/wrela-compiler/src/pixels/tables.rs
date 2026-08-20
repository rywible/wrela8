//! Canonical P9 color-transfer tables and their fail-closed verifier.

pub const TRANSFER_TABLE_ENTRIES_V1: usize = 4097;
pub const TRANSFER_TABLE_BYTES_V1: usize = TRANSFER_TABLE_ENTRIES_V1 * 2;
pub const FILMIC_LOG2_MIN_V1: i32 = -16;
pub const FILMIC_LOG2_MAX_V1: i32 = 16;

const FILMIC_BYTES: &[u8] = include_bytes!("../../../../stdlib/data/pixels/filmic_v1_u16.bin");
const SRGB_BYTES: &[u8] = include_bytes!("../../../../stdlib/data/pixels/srgb_v1_u16.bin");

// These are the digests of the canonical little-endian u16 byte streams.
pub const FILMIC_V1_SHA256: &str =
    "834b92da2dc0efaa7ffeee438f95a9de53988abcfa0d122f55329ec01e1ebf6f";
pub const SRGB_V1_SHA256: &str = "28c6391387185672fd824973e342a185f7cc90d487be3d966821412509213201";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKind {
    FilmicV1,
    SrgbV1,
}

impl TableKind {
    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::FilmicV1 => "filmic-v1-log2[-16,+16]-u16le-4097",
            Self::SrgbV1 => "srgb-v1-linear[0,1]-u16le-4097",
        }
    }

    pub const fn expected_digest(self) -> &'static str {
        match self {
            Self::FilmicV1 => FILMIC_V1_SHA256,
            Self::SrgbV1 => SRGB_V1_SHA256,
        }
    }

    pub const fn bytes(self) -> &'static [u8] {
        match self {
            Self::FilmicV1 => FILMIC_BYTES,
            Self::SrgbV1 => SRGB_BYTES,
        }
    }
}

pub fn values(kind: TableKind) -> Result<Vec<u16>, String> {
    verify(kind)?;
    Ok(kind
        .bytes()
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect())
}

pub fn verify(kind: TableKind) -> Result<(), String> {
    let bytes = kind.bytes();
    if bytes.len() != TRANSFER_TABLE_BYTES_V1 {
        return Err(format!(
            "P020: renderer proof table failed internal verification: {} has {} bytes, expected {}",
            kind.stable_name(),
            bytes.len(),
            TRANSFER_TABLE_BYTES_V1,
        ));
    }
    let digest = wrela_machine::sha256::sha256_hex(bytes);
    if digest != kind.expected_digest() {
        return Err(format!(
            "P020: renderer proof table failed internal verification: {} digest {} differs from sealed {}",
            kind.stable_name(),
            digest,
            kind.expected_digest(),
        ));
    }
    let mut values = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let first = values.next().ok_or_else(|| {
        "P020: renderer proof table failed internal verification: empty transfer table".to_string()
    })?;
    let mut previous = first;
    for value in values {
        if value < previous {
            return Err(format!(
                "P018: tone or transfer table is not monotone: {}",
                kind.stable_name()
            ));
        }
        previous = value;
    }
    match kind {
        TableKind::FilmicV1 if first != 0 || previous != u16::MAX => Err(format!(
            "P020: renderer proof table failed internal verification: {} endpoints are [{first},{previous}], expected [0,65535]",
            kind.stable_name(),
        )),
        TableKind::SrgbV1 if first != 0 || previous != u16::MAX => Err(format!(
            "P020: renderer proof table failed internal verification: {} endpoints are [{first},{previous}], expected [0,65535]",
            kind.stable_name(),
        )),
        _ => Ok(()),
    }
}

pub fn verify_all() -> Result<(), String> {
    verify(TableKind::FilmicV1)?;
    verify(TableKind::SrgbV1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_tables_have_sealed_dimensions_endpoints_order_and_digest() {
        verify_all().unwrap();
        assert_eq!(values(TableKind::FilmicV1).unwrap().len(), 4097);
        assert_eq!(values(TableKind::SrgbV1).unwrap()[0], 0);
    }
}
