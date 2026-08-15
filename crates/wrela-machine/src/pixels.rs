//! Machine-v1 display ABI.
//!
//! The guest owns and writes every pixel. A doorbell transfers a complete,
//! ordered tile list to the host. The host validates and hashes the assembled
//! BGRA8 framebuffer, then returns ownership synchronously.

pub const WIDTH: u32 = 64;
pub const HEIGHT: u32 = 32;
pub const REFRESH_HZ: u32 = 60;
pub const BYTES_PER_PIXEL: u32 = 4;
pub const STRIDE_BYTES: u32 = WIDTH * BYTES_PER_PIXEL;
pub const FRAME_BYTES: usize = STRIDE_BYTES as usize * HEIGHT as usize;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra8Srgb = 1,
}

pub const FORMAT_BGRA8_SRGB: u8 = PixelFormat::Bgra8Srgb as u8;
/// P-1 source compatibility; new code uses the typed byte-width constant.
pub const FORMAT_BGRA8: u32 = FORMAT_BGRA8_SRGB as u32;
pub const ABI_VERSION: u32 = 2;
pub const CONTROL_BYTES: usize = 160;
pub const GUEST_VISIBLE_DIGEST_OFFSET: usize = 56;
pub const GUEST_RAW_TILE_DIGEST_OFFSET: usize = 88;
pub const GUEST_TILE_DESCRIPTOR_DIGEST_OFFSET: usize = 120;
pub const COMPLETION_STATUS_OFFSET: u64 = 152;
pub const COMPLETION_PENDING: u32 = 0;
pub const COMPLETION_PRESENTED: u32 = 1;
pub const COMPLETION_REJECTED: u32 = 2;
pub const DISPLAY_TILE_DESC_BYTES_V1: usize = 24;
pub const TILE_WIDTH: u32 = 64;
pub const TILE_HEIGHT: u32 = 32;
pub const TILE_STRIDE_BYTES: u32 = TILE_WIDTH * BYTES_PER_PIXEL;
pub const TILE_ALLOCATION_BYTES: usize = TILE_STRIDE_BYTES as usize * TILE_HEIGHT as usize;
pub const MAX_MODE_WIDTH: u32 = 8192;
pub const MAX_MODE_HEIGHT: u32 = 8192;
pub const MAX_TILE_COUNT: u32 =
    MAX_MODE_WIDTH.div_ceil(TILE_WIDTH) * MAX_MODE_HEIGHT.div_ceil(TILE_HEIGHT);

pub const CONTROL_BASE: u64 = crate::layout::DRAM_BASE + 0x10_0000;
pub const TILES_BASE: u64 = CONTROL_BASE + 0x100;
pub const FRAMEBUFFER_BASE: u64 = CONTROL_BASE + 0x1000;
/// Fixed P-1 compatibility metadata used only by the walking-skeleton renderer.
///
/// Production FrameProgram v1 blobs use compiler-assigned renderer placements.
pub const PLANE_SEED_METADATA_BASE: u64 = FRAMEBUFFER_BASE + FRAME_BYTES as u64;
pub const DOORBELL_ADDR: u64 = crate::mmio::MMIO_BASE + 0x6000;
pub const DISPLAY_DOORBELL_LAST_ADDR: u64 = crate::mmio::MMIO_BASE + 0xf000;

pub const fn is_display_doorbell_addr(address: u64) -> bool {
    address >= DOORBELL_ADDR
        && address <= DISPLAY_DOORBELL_LAST_ADDR
        && (address - crate::mmio::MMIO_BASE) % 0x1000 == 0
}

// FrameProgram v1 is shared format data. The compiler writes every field
// explicitly; these structs document and size-check the wire envelope only.
pub const FRAME_PROGRAM_MAGIC_V1: [u8; 8] = *b"WRELAPX\0";
pub const FRAME_PROGRAM_VERSION_V1: u16 = 1;
pub const FRAME_PROGRAM_HEADER_BYTES_V1: u16 = 80;
pub const FRAME_PROGRAM_TABLE_BYTES_V1: u16 = 16;
pub const FRAME_PROGRAM_RECORD_BYTES_V1: u16 = 16;
pub const FRAME_PROGRAM_IMMEDIATE_BYTES_V1: u16 = 24;
pub const FRAME_PROGRAM_MAX_OPERANDS_V1: usize = u16::MAX as usize;
pub const FRAME_PROGRAM_TRANSFORM_DEPTH_MAX_V1: usize = 64;
pub const FRAME_PROGRAM_DIGEST_OFFSET_V1: usize = 48;
pub const FRAME_PROGRAM_DIGEST_BYTES_V1: usize = 32;
pub const FRAME_PROGRAM_TABLE_ALIGNMENT_V1: u64 = 16;
pub const FRAME_PROGRAM_HOT_ALIGNMENT_V1: u64 = 64;
pub const FRAME_PROGRAM_MAX_BYTES_V1: u64 = 64 * 1024 * 1024;
pub const FRAME_PROGRAM_NUMERIC_REVISION_V1: u32 = 1;
pub const FRAME_PROGRAM_FORMAL_REVISION_V1: u32 = 1;
pub const FRAME_PROGRAM_PROFILE_REVISION_V1: u32 = 1;
pub const FRAME_PROGRAM_FORMAL_REVISION_STR_V1: &str = "pixels-v1";

// Canonical output-referred tables are part of the machine-v1 wire contract,
// not merely compiler defaults. Keeping them here lets both the compiler and
// the independent decoder validate the same sealed bytes.
pub const LINEAR_TONE_LUT_V1: [u16; 17] = [
    0, 4096, 8192, 12288, 16384, 20480, 24576, 28672, 32768, 36863, 40959, 45055, 49151, 53247,
    57343, 61439, 65535,
];
pub const FILMIC_TONE_LUT_V1: [u16; 17] = [
    0, 6975, 12799, 17731, 21958, 25620, 28827, 31656, 34170, 36420, 38444, 40272, 41929, 43437,
    44812, 46070, 47222,
];
pub const SRGB_TRANSFER_LUT_V1: [u16; 17] = [
    0, 18173, 25465, 30815, 35199, 38980, 42341, 45388, 48192, 50797, 53238, 55541, 57725, 59805,
    61793, 63701, 65535,
];

#[repr(C)]
pub struct FrameProgramHeaderV1 {
    pub magic: [u8; 8],
    pub version: u16,
    pub header_bytes: u16,
    pub flags: u32,
    pub total_bytes: u32,
    pub renderer_index: u16,
    pub reserved0: u16,
    pub numeric_revision: u32,
    pub formal_revision: u32,
    pub table_count: u16,
    pub reserved1: [u8; 14],
    pub digest: [u8; 32],
}

#[repr(C)]
pub struct FrameProgramTableV1 {
    pub kind: u16,
    pub record_bytes: u16,
    pub count: u32,
    pub offset: u32,
    pub byte_len: u32,
}

#[repr(C)]
pub struct FrameProgramRecordV1 {
    pub stable_id: u32,
    pub tag: u16,
    pub flags: u16,
    pub operand_offset: u32,
    pub operand_count: u16,
    pub reserved: u16,
}

#[repr(C)]
pub struct FrameProgramImmediateV1 {
    pub owner_kind: u16,
    pub reserved0: u16,
    pub owner_id: u32,
    pub ordinal: u32,
    pub reserved1: u32,
    pub value: u64,
}

const _: [(); FRAME_PROGRAM_HEADER_BYTES_V1 as usize] =
    [(); std::mem::size_of::<FrameProgramHeaderV1>()];
const _: [(); FRAME_PROGRAM_TABLE_BYTES_V1 as usize] =
    [(); std::mem::size_of::<FrameProgramTableV1>()];
const _: [(); FRAME_PROGRAM_RECORD_BYTES_V1 as usize] =
    [(); std::mem::size_of::<FrameProgramRecordV1>()];
const _: [(); FRAME_PROGRAM_IMMEDIATE_BYTES_V1 as usize] =
    [(); std::mem::size_of::<FrameProgramImmediateV1>()];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FrameProgramTableKindV1 {
    Scalar,
    Field,
    Object,
    Feature,
    Material,
    Parameter,
    Event,
    Csg,
    FixedDomain,
    Immediate,
    CameraLightPost,
    Texture,
    ShadingSummary,
    Transparency,
    Probe,
    Kinetic,
    DebugName,
}

impl FrameProgramTableKindV1 {
    pub const ALL: [Self; 17] = [
        Self::Scalar,
        Self::Field,
        Self::Object,
        Self::Feature,
        Self::Material,
        Self::Parameter,
        Self::Event,
        Self::Csg,
        Self::FixedDomain,
        Self::Immediate,
        Self::CameraLightPost,
        Self::Texture,
        Self::ShadingSummary,
        Self::Transparency,
        Self::Probe,
        Self::Kinetic,
        Self::DebugName,
    ];

    pub const REQUIRED_COUNT: u16 = Self::ALL.len() as u16;

    pub const fn code(self) -> u16 {
        match self {
            Self::Scalar => 1,
            Self::Field => 2,
            Self::Object => 3,
            Self::Feature => 4,
            Self::Material => 5,
            Self::Parameter => 6,
            Self::Event => 7,
            Self::Csg => 8,
            Self::FixedDomain => 9,
            Self::Immediate => 10,
            Self::CameraLightPost => 11,
            Self::Texture => 12,
            Self::ShadingSummary => 13,
            Self::Transparency => 14,
            Self::Probe => 15,
            Self::Kinetic => 16,
            Self::DebugName => 17,
        }
    }

    pub const fn record_bytes(self) -> u16 {
        match self {
            Self::Immediate => FRAME_PROGRAM_IMMEDIATE_BYTES_V1,
            _ => FRAME_PROGRAM_RECORD_BYTES_V1,
        }
    }

    pub const fn required_at_runtime(self) -> bool {
        !matches!(self, Self::DebugName)
    }

    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }

    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Field => "field",
            Self::Object => "object",
            Self::Feature => "feature",
            Self::Material => "material",
            Self::Parameter => "parameter",
            Self::Event => "event",
            Self::Csg => "csg",
            Self::FixedDomain => "fixed-domain",
            Self::Immediate => "immediate",
            Self::CameraLightPost => "camera-light-post",
            Self::Texture => "texture",
            Self::ShadingSummary => "shading-summary",
            Self::Transparency => "transparency",
            Self::Probe => "probe",
            Self::Kinetic => "kinetic",
            Self::DebugName => "debug-name",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameRecordV1 {
    pub stable_id: u32,
    pub tag: u16,
    pub flags: u16,
    pub operands: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameTableModelV1 {
    pub kind: FrameProgramTableKindV1,
    pub records: Vec<FrameRecordV1>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameProgramModelV1 {
    pub renderer_index: u16,
    pub flags: u32,
    pub numeric_revision: u32,
    pub formal_revision: u32,
    pub tables: Vec<FrameTableModelV1>,
}

impl FrameProgramModelV1 {
    pub fn table(&self, kind: FrameProgramTableKindV1) -> Option<&FrameTableModelV1> {
        self.tables.iter().find(|table| table.kind == kind)
    }

    pub fn record_count(&self, kind: FrameProgramTableKindV1) -> usize {
        self.table(kind).map_or(0, |table| table.records.len())
    }
}

#[path = "pixels_contract.rs"]
mod contract;
pub use contract::{
    FixedQPolicyV1, derive_fixed_q_envelope_v1, derive_fixed_q_policy_v1,
    verify_frame_program_model_v1, verify_frame_record_shape_v1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayModeV1 {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

impl DisplayModeV1 {
    pub const SEED: Self = Self {
        width: WIDTH,
        height: HEIGHT,
        refresh_hz: REFRESH_HZ,
    };

    pub fn validate(self) -> Result<(), DisplayError> {
        if self.width == 0
            || self.height == 0
            || self.refresh_hz == 0
            || self.width > MAX_MODE_WIDTH
            || self.height > MAX_MODE_HEIGHT
        {
            return Err(DisplayError::WrongMode);
        }
        Ok(())
    }

    pub fn tile_count(self) -> Result<u32, DisplayError> {
        self.validate()?;
        self.width
            .div_ceil(TILE_WIDTH)
            .checked_mul(self.height.div_ceil(TILE_HEIGHT))
            .filter(|count| *count != 0 && *count <= MAX_TILE_COUNT)
            .ok_or(DisplayError::TileCountOverflow)
    }

    pub fn visible_bytes(self) -> Result<usize, DisplayError> {
        usize::try_from(self.width)
            .ok()
            .and_then(|width| width.checked_mul(self.height as usize))
            .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL as usize))
            .ok_or(DisplayError::ModeByteOverflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentControl {
    pub abi_version: u32,
    pub format: u8,
    pub generation: u8,
    pub renderer_index: u16,
    pub mode: DisplayModeV1,
    pub tile_count: u32,
    pub sequence: u64,
    pub tiles_addr: u64,
    pub vsync_id: u64,
    pub checkpoint: u64,
    /// Guest-computed bounded digests. The host recomputes each digest from
    /// the descriptor/pixel bytes it actually consumed before accepting the
    /// frame, so stale padding or list bytes cannot hide behind host-only
    /// evidence.
    pub guest_visible_digest: [u64; 4],
    pub guest_raw_tile_digest: [u64; 4],
    pub guest_tile_descriptor_digest: [u64; 4],
    /// Guest writes `COMPLETION_PENDING`; the device replaces it with a
    /// terminal completion code before returning from the synchronous
    /// doorbell. It is not part of the presented-frame identity.
    pub completion_status: u32,
}

impl PresentControl {
    pub fn decode(bytes: &[u8]) -> Result<Self, DisplayError> {
        if bytes.len() < CONTROL_BYTES {
            return Err(DisplayError::TruncatedControl);
        }
        let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        if bytes[156..160].iter().any(|byte| *byte != 0) {
            return Err(DisplayError::ReservedNonzero);
        }
        let digest_at = |at: usize| std::array::from_fn(|word| u64_at(at + word * 8));
        Ok(Self {
            abi_version: u32_at(0),
            format: bytes[4],
            generation: bytes[5],
            renderer_index: u16_at(6),
            mode: DisplayModeV1 {
                width: u32_at(8),
                height: u32_at(12),
                refresh_hz: u32_at(16),
            },
            tile_count: u32_at(20),
            sequence: u64_at(24),
            tiles_addr: u64_at(32),
            vsync_id: u64_at(40),
            checkpoint: u64_at(48),
            guest_visible_digest: digest_at(GUEST_VISIBLE_DIGEST_OFFSET),
            guest_raw_tile_digest: digest_at(GUEST_RAW_TILE_DIGEST_OFFSET),
            guest_tile_descriptor_digest: digest_at(GUEST_TILE_DESCRIPTOR_DIGEST_OFFSET),
            completion_status: u32_at(COMPLETION_STATUS_OFFSET as usize),
        })
    }

    pub fn encode(self) -> [u8; CONTROL_BYTES] {
        let mut out = [0_u8; CONTROL_BYTES];
        out[0..4].copy_from_slice(&self.abi_version.to_le_bytes());
        out[4] = self.format;
        out[5] = self.generation;
        out[6..8].copy_from_slice(&self.renderer_index.to_le_bytes());
        out[8..12].copy_from_slice(&self.mode.width.to_le_bytes());
        out[12..16].copy_from_slice(&self.mode.height.to_le_bytes());
        out[16..20].copy_from_slice(&self.mode.refresh_hz.to_le_bytes());
        out[20..24].copy_from_slice(&self.tile_count.to_le_bytes());
        out[24..32].copy_from_slice(&self.sequence.to_le_bytes());
        out[32..40].copy_from_slice(&self.tiles_addr.to_le_bytes());
        out[40..48].copy_from_slice(&self.vsync_id.to_le_bytes());
        out[48..56].copy_from_slice(&self.checkpoint.to_le_bytes());
        let mut encode_digest = |at: usize, digest: [u64; 4]| {
            for (word, value) in digest.into_iter().enumerate() {
                out[at + word * 8..at + word * 8 + 8].copy_from_slice(&value.to_le_bytes());
            }
        };
        encode_digest(GUEST_VISIBLE_DIGEST_OFFSET, self.guest_visible_digest);
        encode_digest(GUEST_RAW_TILE_DIGEST_OFFSET, self.guest_raw_tile_digest);
        encode_digest(
            GUEST_TILE_DESCRIPTOR_DIGEST_OFFSET,
            self.guest_tile_descriptor_digest,
        );
        out[COMPLETION_STATUS_OFFSET as usize..COMPLETION_STATUS_OFFSET as usize + 4]
            .copy_from_slice(&self.completion_status.to_le_bytes());
        out
    }
}

/// Stable bounded digest used by generated guest code and independently
/// recomputed by the host. This supplements the host SHA-256 replay digests;
/// it is not a replacement for them.
pub fn guest_bounded_digest(bytes: &[u8]) -> [u64; 4] {
    let mut state = [
        1_469_598_103_934_665_603_u64,
        1_099_511_628_211,
        7_809_847_782_465_536_322,
        1_609_587_929_392_839_161,
    ];
    for (index, value) in bytes.iter().copied().enumerate() {
        let value = u64::from(value);
        state[0] = (state[0] ^ value).wrapping_mul(1_099_511_628_211);
        state[1] =
            (state[1] ^ value.wrapping_add(index as u64)).wrapping_mul(14_029_467_366_897_019_727);
        state[2] = state[2]
            .wrapping_add(value)
            .wrapping_mul(11_400_714_785_074_694_791);
        state[3] = (state[3] ^ (value << (index % 8))).wrapping_mul(9_650_029_242_287_828_579);
    }
    state
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayTileDescV1 {
    pub guest_addr: u64,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub stride_bytes: u16,
    pub format: u8,
    pub reserved: [u8; 5],
}

const _: [(); DISPLAY_TILE_DESC_BYTES_V1] = [(); std::mem::size_of::<DisplayTileDescV1>()];

impl DisplayTileDescV1 {
    pub fn decode(bytes: &[u8]) -> Result<Self, DisplayError> {
        if bytes.len() < DISPLAY_TILE_DESC_BYTES_V1 {
            return Err(DisplayError::TruncatedTile);
        }
        let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let descriptor = Self {
            guest_addr: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            x: u16_at(8),
            y: u16_at(10),
            width: u16_at(12),
            height: u16_at(14),
            stride_bytes: u16_at(16),
            format: bytes[18],
            reserved: bytes[19..24].try_into().unwrap(),
        };
        if descriptor.reserved != [0; 5] {
            return Err(DisplayError::ReservedNonzero);
        }
        Ok(descriptor)
    }

    pub fn encode(self) -> [u8; DISPLAY_TILE_DESC_BYTES_V1] {
        let mut out = [0_u8; DISPLAY_TILE_DESC_BYTES_V1];
        out[0..8].copy_from_slice(&self.guest_addr.to_le_bytes());
        out[8..10].copy_from_slice(&self.x.to_le_bytes());
        out[10..12].copy_from_slice(&self.y.to_le_bytes());
        out[12..14].copy_from_slice(&self.width.to_le_bytes());
        out[14..16].copy_from_slice(&self.height.to_le_bytes());
        out[16..18].copy_from_slice(&self.stride_bytes.to_le_bytes());
        out[18] = self.format;
        out[19..24].copy_from_slice(&self.reserved);
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayError {
    TruncatedControl,
    TruncatedTile,
    ReservedNonzero,
    WrongVersion(u32),
    WrongFormat(u8),
    WrongMode,
    ModeChanged,
    ModeByteOverflow,
    TileCountOverflow,
    WrongSequence { expected: u64, found: u64 },
    TileCount { expected: u32, found: u32 },
    Generation(u8),
    FrontGenerationReused(u8),
    TileGeometry(u32),
    TileStride(u32),
    TileFormat { tile: u32, format: u8 },
    TileAddress { tile: u32, address: u64 },
    OverlappingTileAddress { first: u32, second: u32 },
    PixelBytes(u32),
    PaddingNonzero { tile: u32, offset: u32 },
    AlphaNotOpaque { x: u32, y: u32, alpha: u8 },
    GuestVisibleDigest,
    GuestRawTileDigest,
    GuestTileDescriptorDigest,
    SequenceOverflow,
}

impl std::fmt::Display for DisplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedFrame {
    pub renderer_index: u16,
    pub sequence: u64,
    pub generation: u8,
    pub released_generation: Option<u8>,
    pub mode: DisplayModeV1,
    pub format: u8,
    pub tile_descriptor_digest: [u8; 32],
    pub visible_digest: [u8; 32],
    pub raw_tile_digest: [u8; 32],
    pub digest: String,
    pub vsync_id: u64,
    pub checkpoint: u64,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct DisplayQueue {
    /// `None` is the sealed-first-submit negotiation state used by the VMM.
    /// A successfully validated first frame fixes the mode for the lifetime
    /// of the queue; failed submissions never mutate it.
    mode: Option<DisplayModeV1>,
    next_sequence: u64,
    front_generation: Option<u8>,
}

impl Default for DisplayQueue {
    fn default() -> Self {
        Self::new(DisplayModeV1::SEED).expect("seed display mode is valid")
    }
}

impl DisplayQueue {
    pub fn new(mode: DisplayModeV1) -> Result<Self, DisplayError> {
        mode.validate()?;
        Ok(Self {
            mode: Some(mode),
            next_sequence: 0,
            front_generation: None,
        })
    }

    pub fn negotiate_first_mode() -> Self {
        Self {
            mode: None,
            next_sequence: 0,
            front_generation: None,
        }
    }

    pub fn mode(&self) -> Option<DisplayModeV1> {
        self.mode
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn front_generation(&self) -> Option<u8> {
        self.front_generation
    }

    pub fn validate_control(&self, control: PresentControl) -> Result<(), DisplayError> {
        if control.abi_version != ABI_VERSION {
            return Err(DisplayError::WrongVersion(control.abi_version));
        }
        if control.format != FORMAT_BGRA8_SRGB {
            return Err(DisplayError::WrongFormat(control.format));
        }
        control.mode.validate()?;
        if self.mode.is_some_and(|mode| control.mode != mode) {
            return Err(DisplayError::ModeChanged);
        }
        if control.sequence != self.next_sequence {
            return Err(DisplayError::WrongSequence {
                expected: self.next_sequence,
                found: control.sequence,
            });
        }
        if control.generation > 1 {
            return Err(DisplayError::Generation(control.generation));
        }
        if self.front_generation == Some(control.generation) {
            return Err(DisplayError::FrontGenerationReused(control.generation));
        }
        let expected = control.mode.tile_count()?;
        if control.tile_count != expected {
            return Err(DisplayError::TileCount {
                expected,
                found: control.tile_count,
            });
        }
        if control.completion_status != COMPLETION_PENDING {
            return Err(DisplayError::ReservedNonzero);
        }
        Ok(())
    }

    pub fn present<F>(
        &mut self,
        control: PresentControl,
        tiles: &[DisplayTileDescV1],
        mut read_pixels: F,
    ) -> Result<PresentedFrame, DisplayError>
    where
        F: FnMut(&DisplayTileDescV1) -> Option<Vec<u8>>,
    {
        self.validate_control(control)?;
        if control.tile_count as usize != tiles.len() {
            return Err(DisplayError::TileCount {
                expected: control.tile_count,
                found: u32::try_from(tiles.len()).unwrap_or(u32::MAX),
            });
        }
        validate_descriptors(control.mode, tiles)?;

        let mut frame = vec![0_u8; control.mode.visible_bytes()?];
        let mut raw = Vec::with_capacity(
            tiles
                .len()
                .checked_mul(TILE_ALLOCATION_BYTES)
                .ok_or(DisplayError::ModeByteOverflow)?,
        );
        let mut descriptor_bytes = Vec::with_capacity(
            tiles
                .len()
                .checked_mul(DISPLAY_TILE_DESC_BYTES_V1)
                .ok_or(DisplayError::ModeByteOverflow)?,
        );
        for (index, tile) in tiles.iter().enumerate() {
            descriptor_bytes.extend_from_slice(&tile.encode());
            let pixels = read_pixels(tile).ok_or(DisplayError::PixelBytes(index as u32))?;
            if pixels.len() != TILE_ALLOCATION_BYTES {
                return Err(DisplayError::PixelBytes(index as u32));
            }
            validate_padding(index as u32, tile, &pixels)?;
            raw.extend_from_slice(&pixels);
            for row in 0..u32::from(tile.height) {
                for column in 0..u32::from(tile.width) {
                    let source = (row * TILE_STRIDE_BYTES + column * BYTES_PER_PIXEL) as usize;
                    let x = u32::from(tile.x) + column;
                    let y = u32::from(tile.y) + row;
                    let target = ((y * control.mode.width + x) * BYTES_PER_PIXEL) as usize;
                    let alpha = pixels[source + 3];
                    if alpha != 255 {
                        return Err(DisplayError::AlphaNotOpaque { x, y, alpha });
                    }
                    frame[target..target + 4].copy_from_slice(&pixels[source..source + 4]);
                }
            }
        }
        let released_generation = self.front_generation;
        let next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(DisplayError::SequenceOverflow)?;
        let tile_descriptor_digest = crate::sha256::sha256(&descriptor_bytes);
        let visible_digest = crate::sha256::sha256(&frame);
        let raw_tile_digest = crate::sha256::sha256(&raw);
        if guest_bounded_digest(&frame) != control.guest_visible_digest {
            return Err(DisplayError::GuestVisibleDigest);
        }
        if guest_bounded_digest(&raw) != control.guest_raw_tile_digest {
            return Err(DisplayError::GuestRawTileDigest);
        }
        if guest_bounded_digest(&descriptor_bytes) != control.guest_tile_descriptor_digest {
            return Err(DisplayError::GuestTileDescriptorDigest);
        }
        self.mode = Some(control.mode);
        self.next_sequence = next_sequence;
        self.front_generation = Some(control.generation);
        Ok(PresentedFrame {
            renderer_index: control.renderer_index,
            sequence: control.sequence,
            generation: control.generation,
            released_generation,
            mode: control.mode,
            format: control.format,
            tile_descriptor_digest,
            visible_digest,
            raw_tile_digest,
            digest: crate::sha256::sha256_hex(&frame),
            vsync_id: control.vsync_id,
            checkpoint: control.checkpoint,
            bgra: frame,
        })
    }
}

fn validate_descriptors(
    mode: DisplayModeV1,
    tiles: &[DisplayTileDescV1],
) -> Result<(), DisplayError> {
    let tiles_x = mode.width.div_ceil(TILE_WIDTH);
    // Earliest tile id seen at each guest address, so the overlap test below
    // stays a lookup rather than a scan over every earlier descriptor: the mode
    // cap admits 32k tiles, where the quadratic form is half a billion
    // comparisons on a hostile-but-legal list.
    let mut earliest_at: std::collections::BTreeMap<u64, u32> = std::collections::BTreeMap::new();
    for (index, tile) in tiles.iter().enumerate() {
        let id = index as u32;
        let expected_x = (id % tiles_x) * TILE_WIDTH;
        let expected_y = (id / tiles_x) * TILE_HEIGHT;
        let expected_width = (mode.width - expected_x).min(TILE_WIDTH);
        let expected_height = (mode.height - expected_y).min(TILE_HEIGHT);
        if u32::from(tile.x) != expected_x
            || u32::from(tile.y) != expected_y
            || u32::from(tile.width) != expected_width
            || u32::from(tile.height) != expected_height
        {
            return Err(DisplayError::TileGeometry(id));
        }
        if u32::from(tile.stride_bytes) != TILE_STRIDE_BYTES {
            return Err(DisplayError::TileStride(id));
        }
        if tile.format != FORMAT_BGRA8_SRGB {
            return Err(DisplayError::TileFormat {
                tile: id,
                format: tile.format,
            });
        }
        if tile.guest_addr % 4096 != 0
            || tile
                .guest_addr
                .checked_add(TILE_ALLOCATION_BYTES as u64)
                .is_none()
        {
            return Err(DisplayError::TileAddress {
                tile: id,
                address: tile.guest_addr,
            });
        }
        // Every descriptor reaching this point is 4096-aligned and spans 8192
        // bytes, so two tiles overlap exactly when their addresses are equal or
        // one page apart. Probing those three keys reproduces the earlier
        // "lowest overlapping tile id" answer without the pairwise scan.
        let overlap = [
            tile.guest_addr.checked_sub(4096),
            Some(tile.guest_addr),
            tile.guest_addr.checked_add(4096),
        ]
        .into_iter()
        .flatten()
        .filter_map(|address| earliest_at.get(&address).copied())
        .min();
        if let Some(first) = overlap {
            return Err(DisplayError::OverlappingTileAddress { first, second: id });
        }
        earliest_at.entry(tile.guest_addr).or_insert(id);
    }
    Ok(())
}

fn validate_padding(
    tile_id: u32,
    tile: &DisplayTileDescV1,
    pixels: &[u8],
) -> Result<(), DisplayError> {
    for row in 0..TILE_HEIGHT {
        for column in 0..TILE_WIDTH {
            if row < u32::from(tile.height) && column < u32::from(tile.width) {
                continue;
            }
            let offset = (row * TILE_STRIDE_BYTES + column * BYTES_PER_PIXEL) as usize;
            if pixels[offset..offset + 4] != [0; 4] {
                return Err(DisplayError::PaddingNonzero {
                    tile: tile_id,
                    offset: offset as u32,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(mode: DisplayModeV1, generation: u8, sequence: u64) -> PresentControl {
        PresentControl {
            abi_version: ABI_VERSION,
            format: FORMAT_BGRA8_SRGB,
            generation,
            renderer_index: 3,
            mode,
            tile_count: mode.tile_count().unwrap(),
            sequence,
            tiles_addr: TILES_BASE,
            vsync_id: 11,
            checkpoint: 17,
            guest_visible_digest: [0; 4],
            guest_raw_tile_digest: [0; 4],
            guest_tile_descriptor_digest: [0; 4],
            completion_status: COMPLETION_PENDING,
        }
    }

    fn descriptors(mode: DisplayModeV1, generation: u8) -> Vec<DisplayTileDescV1> {
        let tiles_x = mode.width.div_ceil(TILE_WIDTH);
        (0..mode.tile_count().unwrap())
            .map(|id| {
                let x = (id % tiles_x) * TILE_WIDTH;
                let y = (id / tiles_x) * TILE_HEIGHT;
                DisplayTileDescV1 {
                    guest_addr: FRAMEBUFFER_BASE
                        + u64::from(generation)
                            * u64::from(mode.tile_count().unwrap())
                            * TILE_ALLOCATION_BYTES as u64
                        + u64::from(id) * TILE_ALLOCATION_BYTES as u64,
                    x: x as u16,
                    y: y as u16,
                    width: (mode.width - x).min(TILE_WIDTH) as u16,
                    height: (mode.height - y).min(TILE_HEIGHT) as u16,
                    stride_bytes: TILE_STRIDE_BYTES as u16,
                    format: FORMAT_BGRA8_SRGB,
                    reserved: [0; 5],
                }
            })
            .collect()
    }

    fn tile_bytes(tile: &DisplayTileDescV1, code: [u8; 4]) -> Vec<u8> {
        let mut bytes = vec![0_u8; TILE_ALLOCATION_BYTES];
        for row in 0..u32::from(tile.height) {
            for column in 0..u32::from(tile.width) {
                let offset = (row * TILE_STRIDE_BYTES + column * 4) as usize;
                bytes[offset..offset + 4].copy_from_slice(&code);
            }
        }
        bytes
    }

    fn seal_control(
        mut control: PresentControl,
        descriptors: &[DisplayTileDescV1],
        payloads: &[Vec<u8>],
    ) -> PresentControl {
        let mut visible = vec![0_u8; control.mode.visible_bytes().unwrap()];
        let mut raw = Vec::new();
        let mut descriptor_bytes = Vec::new();
        for (descriptor, payload) in descriptors.iter().zip(payloads) {
            descriptor_bytes.extend_from_slice(&descriptor.encode());
            raw.extend_from_slice(payload);
            for row in 0..u32::from(descriptor.height) {
                let source = row as usize * TILE_STRIDE_BYTES as usize;
                let target = ((u32::from(descriptor.y) + row) * control.mode.width
                    + u32::from(descriptor.x)) as usize
                    * BYTES_PER_PIXEL as usize;
                let count = descriptor.width as usize * BYTES_PER_PIXEL as usize;
                visible[target..target + count].copy_from_slice(&payload[source..source + count]);
            }
        }
        control.guest_visible_digest = guest_bounded_digest(&visible);
        control.guest_raw_tile_digest = guest_bounded_digest(&raw);
        control.guest_tile_descriptor_digest = guest_bounded_digest(&descriptor_bytes);
        control
    }

    #[test]
    fn records_round_trip_little_endian_without_host_layout() {
        let c = control(DisplayModeV1::SEED, 1, 0);
        assert_eq!(PresentControl::decode(&c.encode()).unwrap(), c);
        let descriptor = descriptors(DisplayModeV1::SEED, 1)[0];
        assert_eq!(
            DisplayTileDescV1::decode(&descriptor.encode()).unwrap(),
            descriptor
        );
        assert_eq!(
            &descriptor.encode()[0..8],
            &descriptor.guest_addr.to_le_bytes()
        );
        assert_eq!(DISPLAY_TILE_DESC_BYTES_V1, 24);
    }

    #[test]
    fn arbitrary_positive_modes_derive_exact_tiles_and_full_allocation_bytes() {
        for (width, height, count) in [(1, 1, 1), (64, 32, 1), (65, 32, 2), (65, 33, 4)] {
            let mode = DisplayModeV1 {
                width,
                height,
                refresh_hz: 60,
            };
            assert_eq!(mode.tile_count(), Ok(count));
            assert_eq!(
                descriptors(mode, 0).len() * TILE_ALLOCATION_BYTES,
                count as usize * 8192
            );
        }
        assert_eq!(
            DisplayModeV1 {
                width: 0,
                height: 1,
                refresh_hz: 60
            }
            .tile_count(),
            Err(DisplayError::WrongMode)
        );
    }

    #[test]
    fn full_bgra_frame_digest_is_stable_and_alpha_is_opaque() {
        let mode = DisplayModeV1::SEED;
        let descriptors = descriptors(mode, 1);
        let pixels = tile_bytes(&descriptors[0], [1, 2, 3, 255]);
        let sealed = seal_control(control(mode, 1, 0), &descriptors, &[pixels.clone()]);
        let frame = DisplayQueue::new(mode)
            .unwrap()
            .present(sealed, &descriptors, |_| Some(pixels.clone()))
            .unwrap();
        assert_eq!(frame.bgra.len(), FRAME_BYTES);
        assert_eq!(&frame.bgra[..4], &[1, 2, 3, 255]);
        assert_eq!(frame.digest, crate::sha256::sha256_hex(&frame.bgra));
        assert_eq!(frame.visible_digest, frame.raw_tile_digest);
    }

    #[test]
    fn guest_evidence_rejects_visible_raw_and_descriptor_corruption_distinctly() {
        let mode = DisplayModeV1::SEED;
        let descriptors = descriptors(mode, 1);
        let pixels = tile_bytes(&descriptors[0], [1, 2, 3, 255]);
        let sealed = seal_control(
            control(mode, 1, 0),
            &descriptors,
            std::slice::from_ref(&pixels),
        );

        for (submitted, expected) in [
            (sealed, DisplayError::GuestVisibleDigest),
            (sealed, DisplayError::GuestRawTileDigest),
            (sealed, DisplayError::GuestTileDescriptorDigest),
        ]
        .into_iter()
        .enumerate()
        .map(|(class, pair)| {
            let (mut submitted, expected) = pair;
            match class {
                0 => submitted.guest_visible_digest[0] ^= 1,
                1 => submitted.guest_raw_tile_digest[0] ^= 1,
                2 => submitted.guest_tile_descriptor_digest[0] ^= 1,
                _ => unreachable!(),
            }
            (submitted, expected)
        }) {
            assert_eq!(
                DisplayQueue::new(mode)
                    .unwrap()
                    .present(submitted, &descriptors, |_| Some(pixels.clone()),),
                Err(expected),
            );
        }
    }

    #[test]
    fn partial_padding_is_raw_digest_input_but_never_visible() {
        let mode = DisplayModeV1 {
            width: 65,
            height: 33,
            refresh_hz: 60,
        };
        let descriptors = descriptors(mode, 1);
        let payloads: Vec<Vec<u8>> = descriptors
            .iter()
            .enumerate()
            .map(|(index, tile)| tile_bytes(tile, [index as u8, 2, 3, 255]))
            .collect();
        let sealed = seal_control(control(mode, 1, 0), &descriptors, &payloads);
        let mut index = 0;
        let frame = DisplayQueue::new(mode)
            .unwrap()
            .present(sealed, &descriptors, |_| {
                let result = payloads[index].clone();
                index += 1;
                Some(result)
            })
            .unwrap();
        assert_eq!(frame.bgra.len(), 65 * 33 * 4);
        let mut corrupt = payloads.clone();
        corrupt[1][4] = 1;
        let mut index = 0;
        assert!(matches!(
            DisplayQueue::new(mode)
                .unwrap()
                .present(control(mode, 1, 0), &descriptors, |_| {
                    let result = corrupt[index].clone();
                    index += 1;
                    Some(result)
                }),
            Err(DisplayError::PaddingNonzero { tile: 1, offset: 4 })
        ));
    }

    #[test]
    fn overlapping_guest_tile_allocations_are_rejected_before_pixel_reads() {
        let mode = DisplayModeV1 {
            width: 65,
            height: 32,
            refresh_hz: 60,
        };
        let mut tiles = descriptors(mode, 0);
        tiles[1].guest_addr = tiles[0].guest_addr + 4096;
        let mut reads = 0;
        let error = DisplayQueue::new(mode)
            .unwrap()
            .present(control(mode, 0, 0), &tiles, |_| {
                reads += 1;
                None
            })
            .unwrap_err();
        assert_eq!(
            error,
            DisplayError::OverlappingTileAddress {
                first: 0,
                second: 1
            }
        );
        assert_eq!(reads, 0);
    }

    #[test]
    fn overlap_detection_reports_the_lowest_partner_at_every_page_offset() {
        // The detector indexes addresses instead of comparing every pair, so
        // pin the answers it must keep: a page-below, exact, and page-above
        // collision are all overlaps, a two-page gap is not, and the reported
        // `first` stays the lowest colliding tile id rather than the nearest.
        let mode = DisplayModeV1 {
            width: 256,
            height: 32,
            refresh_hz: 60,
        };
        let base = descriptors(mode, 0)[0].guest_addr;
        for (offset, expected) in [
            (0_i64, Some((0_u32, 3_u32))),
            (4096, Some((0, 3))),
            (-4096, Some((0, 3))),
            // One whole tile up is tile 1's own address, so the collision is
            // real and its partner is 1 — not the lower-addressed tile 0.
            (8192, Some((1, 3))),
            (-8192, None),
        ] {
            let mut tiles = descriptors(mode, 0);
            tiles[3].guest_addr = base.wrapping_add_signed(offset);
            let result = validate_descriptors(mode, &tiles);
            match expected {
                Some((first, second)) => assert_eq!(
                    result,
                    Err(DisplayError::OverlappingTileAddress { first, second }),
                    "tile 3 at {offset:+} from tile 0 must collide"
                ),
                None => assert_eq!(result, Ok(()), "tile 3 at {offset:+} is disjoint"),
            }
        }
    }

    #[test]
    fn first_submission_seals_an_arbitrary_valid_mode_and_later_changes_fail() {
        let mode = DisplayModeV1 {
            width: 65,
            height: 33,
            refresh_hz: 75,
        };
        let descriptors = descriptors(mode, 1);
        let payloads: Vec<_> = descriptors
            .iter()
            .map(|tile| tile_bytes(tile, [1, 2, 3, 255]))
            .collect();
        let mut queue = DisplayQueue::negotiate_first_mode();
        let mut index = 0;
        let sealed = seal_control(control(mode, 1, 0), &descriptors, &payloads);
        let frame = queue
            .present(sealed, &descriptors, |_| {
                let bytes = payloads[index].clone();
                index += 1;
                Some(bytes)
            })
            .unwrap();
        assert_eq!(frame.mode, mode);
        assert_eq!(queue.mode(), Some(mode));

        let changed = DisplayModeV1 {
            width: 64,
            height: 32,
            refresh_hz: 60,
        };
        assert_eq!(
            queue.validate_control(control(changed, 0, 1)),
            Err(DisplayError::ModeChanged)
        );
    }

    #[test]
    fn failed_submission_does_not_advance_or_release_front_generation() {
        let mode = DisplayModeV1::SEED;
        let generation1 = descriptors(mode, 1);
        let pixels = tile_bytes(&generation1[0], [0, 0, 0, 255]);
        let mut queue = DisplayQueue::new(mode).unwrap();
        let first_control = seal_control(
            control(mode, 1, 0),
            &generation1,
            std::slice::from_ref(&pixels),
        );
        let first = queue
            .present(first_control, &generation1, |_| Some(pixels.clone()))
            .unwrap();
        assert_eq!(first.released_generation, None);
        let mut wrong = control(mode, 0, 2);
        assert!(matches!(
            queue.present(wrong, &descriptors(mode, 0), |_| Some(pixels.clone())),
            Err(DisplayError::WrongSequence {
                expected: 1,
                found: 2
            })
        ));
        assert_eq!(queue.next_sequence(), 1);
        assert_eq!(queue.front_generation(), Some(1));
        wrong.sequence = 1;
        wrong = seal_control(wrong, &descriptors(mode, 0), std::slice::from_ref(&pixels));
        let second = queue
            .present(wrong, &descriptors(mode, 0), |_| Some(pixels.clone()))
            .unwrap();
        assert_eq!(second.released_generation, Some(1));
        assert_eq!(queue.front_generation(), Some(0));
    }

    #[test]
    fn hostile_descriptors_fail_before_pixel_read() {
        let mode = DisplayModeV1::SEED;
        let mut descriptors = descriptors(mode, 1);
        descriptors[0].stride_bytes = 4;
        let mut reads = 0;
        assert_eq!(
            DisplayQueue::new(mode)
                .unwrap()
                .present(control(mode, 1, 0), &descriptors, |_| {
                    reads += 1;
                    None
                }),
            Err(DisplayError::TileStride(0))
        );
        assert_eq!(reads, 0);
    }

    #[test]
    fn display_doorbell_slots_are_page_aligned_and_bounded() {
        assert!(is_display_doorbell_addr(DOORBELL_ADDR));
        assert!(is_display_doorbell_addr(DOORBELL_ADDR + 0x1000));
        assert!(is_display_doorbell_addr(DISPLAY_DOORBELL_LAST_ADDR));
        assert!(!is_display_doorbell_addr(DOORBELL_ADDR + 8));
        assert!(!is_display_doorbell_addr(DOORBELL_ADDR - 0x1000));
        assert!(!is_display_doorbell_addr(
            DISPLAY_DOORBELL_LAST_ADDR + 0x1000
        ));
    }
}
