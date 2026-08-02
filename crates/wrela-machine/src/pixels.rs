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

pub const FORMAT_BGRA8: u32 = 1;
pub const ABI_VERSION: u32 = 1;
pub const QUEUE_CAPACITY: u32 = 4;
pub const CONTROL_BYTES: usize = 64;
pub const TILE_BYTES: usize = 32;

pub const CONTROL_BASE: u64 = crate::layout::DRAM_BASE + 0x10_0000;
pub const TILES_BASE: u64 = CONTROL_BASE + 0x100;
pub const FRAMEBUFFER_BASE: u64 = CONTROL_BASE + 0x1000;
pub const FRAME_PROGRAM_BASE: u64 = FRAMEBUFFER_BASE + FRAME_BYTES as u64;
pub const DOORBELL_ADDR: u64 = crate::mmio::MMIO_BASE + 0x6000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentControl {
    pub abi_version: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub tile_count: u32,
    pub sequence: u64,
    pub tiles_addr: u64,
}

impl PresentControl {
    pub fn decode(bytes: &[u8]) -> Result<Self, DisplayError> {
        if bytes.len() < CONTROL_BYTES {
            return Err(DisplayError::TruncatedControl);
        }
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        let reserved = &bytes[40..CONTROL_BYTES];
        if reserved.iter().any(|b| *b != 0) {
            return Err(DisplayError::ReservedNonzero);
        }
        Ok(Self {
            abi_version: u32_at(0),
            format: u32_at(4),
            width: u32_at(8),
            height: u32_at(12),
            stride_bytes: u32_at(16),
            tile_count: u32_at(20),
            sequence: u64_at(24),
            tiles_addr: u64_at(32),
        })
    }

    pub fn encode(self) -> [u8; CONTROL_BYTES] {
        let mut out = [0u8; CONTROL_BYTES];
        out[0..4].copy_from_slice(&self.abi_version.to_le_bytes());
        out[4..8].copy_from_slice(&self.format.to_le_bytes());
        out[8..12].copy_from_slice(&self.width.to_le_bytes());
        out[12..16].copy_from_slice(&self.height.to_le_bytes());
        out[16..20].copy_from_slice(&self.stride_bytes.to_le_bytes());
        out[20..24].copy_from_slice(&self.tile_count.to_le_bytes());
        out[24..32].copy_from_slice(&self.sequence.to_le_bytes());
        out[32..40].copy_from_slice(&self.tiles_addr.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    pub id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub pixels_addr: u64,
}

impl Tile {
    pub fn decode(bytes: &[u8]) -> Result<Self, DisplayError> {
        if bytes.len() < TILE_BYTES {
            return Err(DisplayError::TruncatedTile);
        }
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        Ok(Self {
            id: u32_at(0),
            x: u32_at(4),
            y: u32_at(8),
            width: u32_at(12),
            height: u32_at(16),
            stride_bytes: u32_at(20),
            pixels_addr: u64_at(24),
        })
    }

    pub fn encode(self) -> [u8; TILE_BYTES] {
        let mut out = [0u8; TILE_BYTES];
        out[0..4].copy_from_slice(&self.id.to_le_bytes());
        out[4..8].copy_from_slice(&self.x.to_le_bytes());
        out[8..12].copy_from_slice(&self.y.to_le_bytes());
        out[12..16].copy_from_slice(&self.width.to_le_bytes());
        out[16..20].copy_from_slice(&self.height.to_le_bytes());
        out[20..24].copy_from_slice(&self.stride_bytes.to_le_bytes());
        out[24..32].copy_from_slice(&self.pixels_addr.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayError {
    TruncatedControl,
    TruncatedTile,
    ReservedNonzero,
    WrongVersion(u32),
    WrongFormat(u32),
    WrongExtent,
    WrongStride,
    WrongSequence { expected: u64, found: u64 },
    TileCount(u32),
    TileOrder { expected: u32, found: u32 },
    TileBounds(u32),
    TileStride(u32),
    DuplicateCoverage { x: u32, y: u32 },
    MissingCoverage { x: u32, y: u32 },
    PixelBytes(u32),
}

impl std::fmt::Display for DisplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedFrame {
    pub sequence: u64,
    pub digest: String,
    pub bgra: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct DisplayQueue {
    next_sequence: u64,
}

impl DisplayQueue {
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn validate_control(&self, control: PresentControl) -> Result<(), DisplayError> {
        if control.abi_version != ABI_VERSION {
            return Err(DisplayError::WrongVersion(control.abi_version));
        }
        if control.format != FORMAT_BGRA8 {
            return Err(DisplayError::WrongFormat(control.format));
        }
        if control.width != WIDTH || control.height != HEIGHT {
            return Err(DisplayError::WrongExtent);
        }
        if control.stride_bytes != STRIDE_BYTES {
            return Err(DisplayError::WrongStride);
        }
        if control.sequence != self.next_sequence {
            return Err(DisplayError::WrongSequence {
                expected: self.next_sequence,
                found: control.sequence,
            });
        }
        if control.tile_count == 0 || control.tile_count > QUEUE_CAPACITY {
            return Err(DisplayError::TileCount(control.tile_count));
        }
        Ok(())
    }

    pub fn present<F>(
        &mut self,
        control: PresentControl,
        tiles: &[Tile],
        mut read_pixels: F,
    ) -> Result<PresentedFrame, DisplayError>
    where
        F: FnMut(&Tile) -> Option<Vec<u8>>,
    {
        self.validate_control(control)?;
        if control.tile_count as usize != tiles.len() {
            return Err(DisplayError::TileCount(control.tile_count));
        }

        let mut frame = vec![0u8; FRAME_BYTES];
        let mut covered = vec![false; WIDTH as usize * HEIGHT as usize];
        for (index, tile) in tiles.iter().enumerate() {
            if tile.id != index as u32 {
                return Err(DisplayError::TileOrder {
                    expected: index as u32,
                    found: tile.id,
                });
            }
            let x_end = tile.x.checked_add(tile.width);
            let y_end = tile.y.checked_add(tile.height);
            if tile.width == 0
                || tile.height == 0
                || x_end.is_none_or(|end| end > WIDTH)
                || y_end.is_none_or(|end| end > HEIGHT)
            {
                return Err(DisplayError::TileBounds(tile.id));
            }
            let row_bytes = tile.width * BYTES_PER_PIXEL;
            if tile.stride_bytes != row_bytes {
                return Err(DisplayError::TileStride(tile.id));
            }
            let expected = row_bytes as usize * tile.height as usize;
            let Some(pixels) = read_pixels(tile) else {
                return Err(DisplayError::PixelBytes(tile.id));
            };
            if pixels.len() != expected {
                return Err(DisplayError::PixelBytes(tile.id));
            }
            for row in 0..tile.height {
                for col in 0..tile.width {
                    let x = tile.x + col;
                    let y = tile.y + row;
                    let coverage = y as usize * WIDTH as usize + x as usize;
                    if covered[coverage] {
                        return Err(DisplayError::DuplicateCoverage { x, y });
                    }
                    covered[coverage] = true;
                    let source = (row * tile.stride_bytes + col * BYTES_PER_PIXEL) as usize;
                    let target = (y * STRIDE_BYTES + x * BYTES_PER_PIXEL) as usize;
                    frame[target..target + 4].copy_from_slice(&pixels[source..source + 4]);
                }
            }
        }
        if let Some(index) = covered.iter().position(|value| !value) {
            return Err(DisplayError::MissingCoverage {
                x: index as u32 % WIDTH,
                y: index as u32 / WIDTH,
            });
        }
        let digest = crate::sha256::sha256_hex(&frame);
        self.next_sequence += 1;
        Ok(PresentedFrame {
            sequence: control.sequence,
            digest,
            bgra: frame,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(tile_count: u32) -> PresentControl {
        PresentControl {
            abi_version: ABI_VERSION,
            format: FORMAT_BGRA8,
            width: WIDTH,
            height: HEIGHT,
            stride_bytes: STRIDE_BYTES,
            tile_count,
            sequence: 0,
            tiles_addr: TILES_BASE,
        }
    }

    fn full_tile() -> Tile {
        Tile {
            id: 0,
            x: 0,
            y: 0,
            width: WIDTH,
            height: HEIGHT,
            stride_bytes: STRIDE_BYTES,
            pixels_addr: FRAMEBUFFER_BASE,
        }
    }

    #[test]
    fn records_round_trip_without_host_layout() {
        let c = control(1);
        assert_eq!(PresentControl::decode(&c.encode()).unwrap(), c);
        let t = full_tile();
        assert_eq!(Tile::decode(&t.encode()).unwrap(), t);
    }

    #[test]
    fn full_bgra_frame_digest_is_stable_and_byte_sensitive() {
        let mut queue = DisplayQueue::default();
        let pixels: Vec<u8> = (0..FRAME_BYTES).map(|i| (i % 251) as u8).collect();
        let frame = queue
            .present(control(1), &[full_tile()], |_| Some(pixels.clone()))
            .unwrap();
        assert_eq!(
            frame.digest,
            "25df2449b2e5a35fea14e02a7158e283801a1069c9f84631b9a9dacb2f809a7f"
        );
        assert_eq!(frame.bgra, pixels);
        assert_eq!(queue.next_sequence(), 1);
    }

    #[test]
    fn rejects_sequence_without_consuming_ownership() {
        let mut queue = DisplayQueue::default();
        let mut c = control(1);
        c.sequence = 1;
        assert!(matches!(
            queue.present(c, &[full_tile()], |_| Some(vec![0; FRAME_BYTES])),
            Err(DisplayError::WrongSequence {
                expected: 0,
                found: 1
            })
        ));
        assert_eq!(queue.next_sequence(), 0);
    }

    #[test]
    fn rejects_duplicate_missing_and_out_of_order_tiles() {
        let left = Tile {
            id: 0,
            width: WIDTH / 2,
            stride_bytes: WIDTH / 2 * 4,
            ..full_tile()
        };
        let mut duplicate = left;
        duplicate.id = 1;
        assert!(matches!(
            DisplayQueue::default().present(control(2), &[left, duplicate], |tile| {
                Some(vec![0; tile.stride_bytes as usize * tile.height as usize])
            }),
            Err(DisplayError::DuplicateCoverage { .. })
        ));

        assert!(matches!(
            DisplayQueue::default().present(control(1), &[left], |tile| {
                Some(vec![0; tile.stride_bytes as usize * tile.height as usize])
            }),
            Err(DisplayError::MissingCoverage { .. })
        ));

        let mut wrong_id = full_tile();
        wrong_id.id = 1;
        assert!(matches!(
            DisplayQueue::default()
                .present(control(1), &[wrong_id], |_| { Some(vec![0; FRAME_BYTES]) }),
            Err(DisplayError::TileOrder {
                expected: 0,
                found: 1
            })
        ));
    }

    #[test]
    fn ordered_tiles_assemble_row_major_bgra_bytes() {
        let left = Tile {
            id: 0,
            width: WIDTH / 2,
            stride_bytes: WIDTH / 2 * BYTES_PER_PIXEL,
            ..full_tile()
        };
        let right = Tile {
            id: 1,
            x: WIDTH / 2,
            width: WIDTH / 2,
            stride_bytes: WIDTH / 2 * BYTES_PER_PIXEL,
            pixels_addr: FRAMEBUFFER_BASE + FRAME_BYTES as u64 / 2,
            ..full_tile()
        };
        let frame = DisplayQueue::default()
            .present(control(2), &[left, right], |tile| {
                let bgra = if tile.id == 0 {
                    [1, 2, 3, 4]
                } else {
                    [5, 6, 7, 8]
                };
                Some(
                    bgra.into_iter()
                        .cycle()
                        .take(tile.stride_bytes as usize * tile.height as usize)
                        .collect(),
                )
            })
            .unwrap();
        assert_eq!(&frame.bgra[0..4], &[1, 2, 3, 4]);
        let right_first = (WIDTH / 2 * BYTES_PER_PIXEL) as usize;
        assert_eq!(&frame.bgra[right_first..right_first + 4], &[5, 6, 7, 8]);
        assert_eq!(frame.digest, crate::sha256::sha256_hex(&frame.bgra));
    }

    #[test]
    fn rejects_bounds_stride_and_reserved_bytes() {
        let mut tile = full_tile();
        tile.x = WIDTH;
        assert!(matches!(
            DisplayQueue::default().present(control(1), &[tile], |_| None),
            Err(DisplayError::TileBounds(0))
        ));
        tile = full_tile();
        tile.stride_bytes -= 1;
        assert!(matches!(
            DisplayQueue::default().present(control(1), &[tile], |_| None),
            Err(DisplayError::TileStride(0))
        ));
        let mut bytes = control(1).encode();
        bytes[63] = 1;
        assert_eq!(
            PresentControl::decode(&bytes),
            Err(DisplayError::ReservedNonzero)
        );
    }
}
