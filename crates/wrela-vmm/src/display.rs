use wrela_machine::pixels::{
    CONTROL_BYTES, DisplayError, DisplayQueue, PresentControl, PresentedFrame, TILE_BYTES, Tile,
};

use crate::{VmmError, guest_dram_offset};

#[derive(Debug, Default)]
pub struct HeadlessDisplay {
    queue: DisplayQueue,
    frames: Vec<PresentedFrame>,
}

impl HeadlessDisplay {
    pub fn frames(&self) -> &[PresentedFrame] {
        &self.frames
    }

    pub fn consume(&mut self, dram: &[u8], control_addr: u64) -> Result<PresentedFrame, VmmError> {
        self.consume_with(control_addr, |addr, len, what| {
            let offset = display_dram_offset(addr, len as u64, what)?;
            let end = offset
                .checked_add(len)
                .filter(|end| *end <= dram.len())
                .ok_or_else(|| {
                    VmmError::GuestFault(format!(
                        "{what} address {addr:#x}+{len} exceeds the supplied DRAM snapshot"
                    ))
                })?;
            Ok(dram[offset..end].to_vec())
        })
    }

    /// Consume display-owned bytes using volatile reads from live guest DRAM.
    ///
    /// # Safety
    ///
    /// `host_ram` must point to `dram_len` mapped bytes for the duration of
    /// this call. The guest transfers ownership of the control, tile, and
    /// pixel records before ringing the synchronous display doorbell. Volatile
    /// byte reads avoid creating a Rust shared slice over RAM that other guest
    /// vCPUs can write.
    pub unsafe fn consume_volatile(
        &mut self,
        host_ram: *const u8,
        dram_len: usize,
        control_addr: u64,
    ) -> Result<PresentedFrame, VmmError> {
        self.consume_with(control_addr, |addr, len, what| {
            let offset = display_dram_offset(addr, len as u64, what)?;
            let end = offset
                .checked_add(len)
                .filter(|end| *end <= dram_len)
                .ok_or_else(|| {
                    VmmError::GuestFault(format!(
                        "{what} address {addr:#x}+{len} exceeds mapped guest DRAM"
                    ))
                })?;
            let mut bytes = Vec::with_capacity(len);
            for index in offset..end {
                // SAFETY: the caller guarantees the mapping; the checked
                // offset is within it, and volatile reads form no shared slice.
                bytes.push(unsafe { std::ptr::read_volatile(host_ram.add(index)) });
            }
            Ok(bytes)
        })
    }

    fn consume_with(
        &mut self,
        control_addr: u64,
        mut read: impl FnMut(u64, usize, &str) -> Result<Vec<u8>, VmmError>,
    ) -> Result<PresentedFrame, VmmError> {
        let control_bytes = read(control_addr, CONTROL_BYTES, "display control")?;
        let control = PresentControl::decode(&control_bytes).map_err(display_error)?;
        self.queue
            .validate_control(control)
            .map_err(display_error)?;
        let tile_bytes = control
            .tile_count
            .checked_mul(TILE_BYTES as u32)
            .ok_or_else(|| VmmError::GuestFault("display tile byte count overflow".to_string()))?;
        let tile_records = read(control.tiles_addr, tile_bytes as usize, "display tiles")?;
        let mut tiles = Vec::with_capacity(control.tile_count as usize);
        for bytes in tile_records.chunks_exact(TILE_BYTES) {
            tiles.push(Tile::decode(bytes).map_err(display_error)?);
        }
        let mut pixel_payloads = Vec::with_capacity(tiles.len());
        for tile in &tiles {
            let byte_len = tile.stride_bytes.checked_mul(tile.height).ok_or_else(|| {
                VmmError::GuestFault(format!(
                    "display tile {} pixel byte count overflows",
                    tile.id
                ))
            })? as usize;
            pixel_payloads.push(read(tile.pixels_addr, byte_len, "display pixels")?);
        }
        let mut payload_index = 0usize;
        let frame = self
            .queue
            .present(control, &tiles, |tile| {
                let index = payload_index;
                payload_index += 1;
                pixel_payloads.get(index).cloned().filter(|bytes| {
                    bytes.len() == tile.stride_bytes as usize * tile.height as usize
                })
            })
            .map_err(display_error)?;
        self.frames.push(PresentedFrame {
            sequence: frame.sequence,
            digest: frame.digest.clone(),
            bgra: Vec::new(),
        });
        Ok(frame)
    }
}

fn display_dram_offset(guest: u64, nbytes: u64, what: &str) -> Result<usize, VmmError> {
    guest_dram_offset(guest, nbytes, what)
        .map_err(|error| VmmError::GuestFault(format!("display present rejected: {error}")))
}

fn display_error(error: DisplayError) -> VmmError {
    VmmError::GuestFault(format!("display present rejected: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wrela_machine::{layout, pixels};

    #[test]
    fn consumes_guest_owned_bytes_without_modification() {
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let mut dram = vec![0u8; at(pixels::FRAMEBUFFER_BASE) + pixels::FRAME_BYTES];
        let control = PresentControl {
            abi_version: pixels::ABI_VERSION,
            format: pixels::FORMAT_BGRA8,
            width: pixels::WIDTH,
            height: pixels::HEIGHT,
            stride_bytes: pixels::STRIDE_BYTES,
            tile_count: 1,
            sequence: 0,
            tiles_addr: pixels::TILES_BASE,
        };
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        let tile = Tile {
            id: 0,
            x: 0,
            y: 0,
            width: pixels::WIDTH,
            height: pixels::HEIGHT,
            stride_bytes: pixels::STRIDE_BYTES,
            pixels_addr: pixels::FRAMEBUFFER_BASE,
        };
        dram[at(pixels::TILES_BASE)..at(pixels::TILES_BASE) + pixels::TILE_BYTES]
            .copy_from_slice(&tile.encode());
        let original: Vec<u8> = (0..pixels::FRAME_BYTES)
            .map(|index| (index % 251) as u8)
            .collect();
        dram[at(pixels::FRAMEBUFFER_BASE)..at(pixels::FRAMEBUFFER_BASE) + original.len()]
            .copy_from_slice(&original);

        let before = dram.clone();
        let mut sink = HeadlessDisplay::default();
        let frame = sink.consume(&dram, pixels::CONTROL_BASE).unwrap();
        assert_eq!(frame.bgra, original);
        assert!(sink.frames()[0].bgra.is_empty());
        assert_eq!(dram, before, "headless display must not modify guest bytes");
    }

    #[test]
    fn rejects_hostile_control_before_allocating_or_reading_tiles() {
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let mut dram = vec![0u8; at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES];
        let mut control = PresentControl {
            abi_version: pixels::ABI_VERSION,
            format: pixels::FORMAT_BGRA8,
            width: pixels::WIDTH,
            height: pixels::HEIGHT,
            stride_bytes: pixels::STRIDE_BYTES,
            tile_count: u32::MAX,
            sequence: 0,
            tiles_addr: u64::MAX,
        };
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        let error = HeadlessDisplay::default()
            .consume(&dram, pixels::CONTROL_BASE)
            .unwrap_err();
        assert!(error.to_string().contains("TileCount"));

        control.tile_count = 0;
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        let error = HeadlessDisplay::default()
            .consume(&dram, pixels::CONTROL_BASE)
            .unwrap_err();
        assert!(error.to_string().contains("TileCount"));
    }

    #[test]
    fn guest_supplied_addresses_are_guest_faults_not_bad_images() {
        let dram = vec![0u8; layout::DRAM_SIZE as usize];
        let error = HeadlessDisplay::default()
            .consume(&dram, u64::MAX)
            .unwrap_err();
        assert!(matches!(error, VmmError::GuestFault(_)));

        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let mut dram = vec![0u8; at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES];
        let control = PresentControl {
            abi_version: pixels::ABI_VERSION,
            format: pixels::FORMAT_BGRA8,
            width: pixels::WIDTH,
            height: pixels::HEIGHT,
            stride_bytes: pixels::STRIDE_BYTES,
            tile_count: 1,
            sequence: 0,
            tiles_addr: u64::MAX,
        };
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        let error = HeadlessDisplay::default()
            .consume(&dram, pixels::CONTROL_BASE)
            .unwrap_err();
        assert!(matches!(error, VmmError::GuestFault(_)));
    }

    #[test]
    fn rejects_truncated_and_malformed_control_records() {
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let truncated = vec![0u8; at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES - 1];
        assert!(matches!(
            HeadlessDisplay::default().consume(&truncated, pixels::CONTROL_BASE),
            Err(VmmError::GuestFault(_))
        ));

        let mut dram = vec![0u8; at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES];
        for (offset, value, expected) in [
            (0usize, 99u32, "WrongVersion"),
            (4, 99, "WrongFormat"),
            (8, 99, "WrongExtent"),
            (16, 99, "WrongStride"),
        ] {
            let mut control = PresentControl {
                abi_version: pixels::ABI_VERSION,
                format: pixels::FORMAT_BGRA8,
                width: pixels::WIDTH,
                height: pixels::HEIGHT,
                stride_bytes: pixels::STRIDE_BYTES,
                tile_count: 1,
                sequence: 0,
                tiles_addr: pixels::TILES_BASE,
            }
            .encode();
            control[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
                .copy_from_slice(&control);
            let error = HeadlessDisplay::default()
                .consume(&dram, pixels::CONTROL_BASE)
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
