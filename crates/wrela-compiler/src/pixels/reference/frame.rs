//! Tile coverage validation, deterministic debug output, and worker collection.

use super::sweep::{CertifiedRun, IdentitySetId, SweepError};
use super::telemetry::CertificateTelemetry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoverageRecord {
    pub x0: u16,
    pub x1: u16,
    pub kind: u8,
    pub record_index: u16,
}

pub fn validate_row_coverage(
    tile_x0: u16,
    tile_x1: u16,
    records: &[CoverageRecord],
) -> Result<(), SweepError> {
    if tile_x0 >= tile_x1 || records.is_empty() {
        return Err(SweepError::InternalInvariant);
    }
    let mut cursor = tile_x0;
    for record in records {
        if record.x0 != cursor || record.x0 >= record.x1 || record.x1 > tile_x1 || record.kind > 1 {
            return Err(SweepError::InternalInvariant);
        }
        cursor = record.x1;
    }
    if cursor != tile_x1 {
        return Err(SweepError::InternalInvariant);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct DebugPixel {
    pub object_material_code: u32,
    pub q_class: u16,
    pub coverage: u8,
    pub flags: u8,
}

pub fn debug_pixel(identity: IdentitySetId, q_lo: i32, q_hi: i32, coverage: u8) -> DebugPixel {
    let width = i64::from(q_hi) - i64::from(q_lo);
    let magnitude = i64::from(q_lo)
        .unsigned_abs()
        .max(i64::from(q_hi).unsigned_abs());
    let q_class = ((magnitude.leading_zeros() as u16) << 8)
        | u16::try_from(width.unsigned_abs().min(255)).unwrap_or(255);
    DebugPixel {
        object_material_code: identity.0.wrapping_mul(0x9e37_79b1),
        q_class,
        coverage,
        flags: u8::from(identity.0 != 0),
    }
}

pub fn raster_debug_run(run: CertifiedRun, pixels: &mut [DebugPixel]) -> Result<(), SweepError> {
    let width = usize::from(
        run.x1
            .checked_sub(run.x0)
            .ok_or(SweepError::InternalInvariant)?,
    );
    if pixels.len() != width {
        return Err(SweepError::CapacityExceeded);
    }
    let pixel = if run.visible.is_some() {
        debug_pixel(run.identity, run.q_model.q0.lo, run.q_model.q0.hi, 255)
    } else {
        DebugPixel::default()
    };
    pixels.fill(pixel);
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompletedTile {
    pub tile_id: u32,
    pub digest: [u8; 32],
}

pub fn render_tile(
    tile_id: u32,
    tile_x0: u16,
    tile_x1: u16,
    tile_y0: u16,
    tile_y1: u16,
    row_record_offsets: &[u32],
    coverage: &[CoverageRecord],
    runs: &[CertifiedRun],
    corridor_pixels: &[DebugPixel],
    output: &mut [DebugPixel],
) -> Result<CompletedTile, SweepError> {
    let width = usize::from(
        tile_x1
            .checked_sub(tile_x0)
            .ok_or(SweepError::InternalInvariant)?,
    );
    let height = usize::from(
        tile_y1
            .checked_sub(tile_y0)
            .ok_or(SweepError::InternalInvariant)?,
    );
    if width == 0
        || height == 0
        || row_record_offsets.len() != height + 1
        || output.len()
            != width
                .checked_mul(height)
                .ok_or(SweepError::CapacityExceeded)?
        || row_record_offsets.first().copied() != Some(0)
        || usize::try_from(row_record_offsets.last().copied().unwrap_or_default()).ok()
            != Some(coverage.len())
    {
        return Err(SweepError::InternalInvariant);
    }
    for row in 0..height {
        let start =
            usize::try_from(row_record_offsets[row]).map_err(|_| SweepError::CapacityExceeded)?;
        let end = usize::try_from(row_record_offsets[row + 1])
            .map_err(|_| SweepError::CapacityExceeded)?;
        let records = coverage
            .get(start..end)
            .ok_or(SweepError::InternalInvariant)?;
        validate_row_coverage(tile_x0, tile_x1, records)?;
        let row_output = &mut output[row * width..(row + 1) * width];
        for record in records {
            let first = usize::from(record.x0 - tile_x0);
            let last = usize::from(record.x1 - tile_x0);
            let destination = row_output
                .get_mut(first..last)
                .ok_or(SweepError::InternalInvariant)?;
            match record.kind {
                0 => {
                    let run = *runs
                        .get(usize::from(record.record_index))
                        .ok_or(SweepError::InternalInvariant)?;
                    if run.x0 != record.x0 || run.x1 != record.x1 {
                        return Err(SweepError::InternalInvariant);
                    }
                    raster_debug_run(run, destination)?;
                }
                1 => {
                    let pixel = *corridor_pixels
                        .get(usize::from(record.record_index))
                        .ok_or(SweepError::InternalInvariant)?;
                    destination.fill(pixel);
                }
                _ => return Err(SweepError::InternalInvariant),
            }
        }
    }
    Ok(CompletedTile {
        tile_id,
        digest: debug_tile_digest(output),
    })
}

fn debug_tile_digest(pixels: &[DebugPixel]) -> [u8; 32] {
    let mut state = [0_u8; 32];
    for pixel in pixels {
        let mut bytes = [0_u8; 40];
        bytes[..32].copy_from_slice(&state);
        bytes[32..36].copy_from_slice(&pixel.object_material_code.to_le_bytes());
        bytes[36..38].copy_from_slice(&pixel.q_class.to_le_bytes());
        bytes[38] = pixel.coverage;
        bytes[39] = pixel.flags;
        state = wrela_machine::sha256::sha256(&bytes);
    }
    state
}

#[derive(Clone, Copy, Debug)]
pub struct WorkerCompletion<'a> {
    pub worker_index: u16,
    pub tiles: &'a [CompletedTile],
    pub error: Option<SweepError>,
    pub telemetry: &'a CertificateTelemetry,
}

pub fn collect_worker_completions(
    completions: &[WorkerCompletion<'_>],
    worker_scratch: &mut [usize],
    tiles: &mut [CompletedTile],
    telemetry: &mut CertificateTelemetry,
) -> Result<usize, SweepError> {
    if completions.len() > worker_scratch.len() {
        return Err(SweepError::CapacityExceeded);
    }
    for (slot, index) in worker_scratch.iter_mut().zip(0..completions.len()) {
        *slot = index;
    }
    let order = &mut worker_scratch[..completions.len()];
    for index in 1..order.len() {
        let value = order[index];
        let mut destination = index;
        while destination != 0
            && completions[value].worker_index < completions[order[destination - 1]].worker_index
        {
            order[destination] = order[destination - 1];
            destination -= 1;
        }
        if destination != 0
            && completions[value].worker_index == completions[order[destination - 1]].worker_index
        {
            return Err(SweepError::InternalInvariant);
        }
        order[destination] = value;
    }
    let mut tile_count = 0_usize;
    for completion_index in order.iter().copied() {
        let completion = &completions[completion_index];
        if let Some(error) = completion.error {
            return Err(error);
        }
        telemetry
            .merge_in_tile_order(completion.telemetry)
            .map_err(|_| SweepError::InternalInvariant)?;
        for tile in completion.tiles {
            let Some(slot) = tiles.get_mut(tile_count) else {
                return Err(SweepError::CapacityExceeded);
            };
            *slot = *tile;
            tile_count += 1;
        }
    }
    for index in 1..tile_count {
        let value = tiles[index];
        let mut destination = index;
        while destination != 0 && value.tile_id < tiles[destination - 1].tile_id {
            tiles[destination] = tiles[destination - 1];
            destination -= 1;
        }
        if destination != 0 && value.tile_id == tiles[destination - 1].tile_id {
            return Err(SweepError::InternalInvariant);
        }
        tiles[destination] = value;
    }
    Ok(tile_count)
}

pub fn debug_frame_digest(tiles: &[CompletedTile]) -> [u8; 32] {
    let mut state = [0_u8; 32];
    for tile in tiles {
        let mut bytes = [0_u8; 68];
        bytes[..32].copy_from_slice(&state);
        bytes[32..36].copy_from_slice(&tile.tile_id.to_le_bytes());
        bytes[36..].copy_from_slice(&tile.digest);
        state = wrela_machine::sha256::sha256(&bytes);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_is_exactly_half_open_without_gaps_or_overlaps() {
        let valid = [
            CoverageRecord {
                x0: 0,
                x1: 8,
                kind: 0,
                record_index: 0,
            },
            CoverageRecord {
                x0: 8,
                x1: 9,
                kind: 1,
                record_index: 0,
            },
            CoverageRecord {
                x0: 9,
                x1: 32,
                kind: 0,
                record_index: 1,
            },
        ];
        assert_eq!(validate_row_coverage(0, 32, &valid), Ok(()));
        let mut gap = valid;
        gap[1].x0 = 9;
        assert_eq!(
            validate_row_coverage(0, 32, &gap),
            Err(SweepError::InternalInvariant)
        );
        let mut overlap = valid;
        overlap[1].x0 = 7;
        assert_eq!(
            validate_row_coverage(0, 32, &overlap),
            Err(SweepError::InternalInvariant)
        );
    }

    #[test]
    fn completion_order_does_not_change_tile_or_frame_order() {
        let worker0_tiles = [CompletedTile {
            tile_id: 0,
            digest: [1; 32],
        }];
        let worker1_tiles = [CompletedTile {
            tile_id: 1,
            digest: [2; 32],
        }];
        let first_telemetry = CertificateTelemetry::default();
        let second_telemetry = CertificateTelemetry::default();
        let completions = [
            WorkerCompletion {
                worker_index: 1,
                tiles: &worker1_tiles,
                error: None,
                telemetry: &second_telemetry,
            },
            WorkerCompletion {
                worker_index: 0,
                tiles: &worker0_tiles,
                error: None,
                telemetry: &first_telemetry,
            },
        ];
        let mut scratch = [0; 2];
        let mut tiles = [CompletedTile::default(); 2];
        let mut merged = CertificateTelemetry::default();
        let count = collect_worker_completions(&completions, &mut scratch, &mut tiles, &mut merged)
            .unwrap();
        assert_eq!(
            tiles[..count]
                .iter()
                .map(|tile| tile.tile_id)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_ne!(debug_frame_digest(&tiles[..count]), [0; 32]);
    }

    #[test]
    fn one_and_four_worker_partitions_have_identical_frame_digest() {
        let source = [
            CompletedTile {
                tile_id: 0,
                digest: [1; 32],
            },
            CompletedTile {
                tile_id: 1,
                digest: [2; 32],
            },
            CompletedTile {
                tile_id: 2,
                digest: [3; 32],
            },
            CompletedTile {
                tile_id: 3,
                digest: [4; 32],
            },
        ];
        let mut one_telemetry = CertificateTelemetry::default();
        one_telemetry.regular_pixels = 4;
        let mut four_telemetry = [CertificateTelemetry::default(); 4];
        for local in &mut four_telemetry {
            local.regular_pixels = 1;
        }
        let one = [WorkerCompletion {
            worker_index: 0,
            tiles: &source,
            error: None,
            telemetry: &one_telemetry,
        }];
        let four = [
            WorkerCompletion {
                worker_index: 3,
                tiles: &source[3..4],
                error: None,
                telemetry: &four_telemetry[3],
            },
            WorkerCompletion {
                worker_index: 1,
                tiles: &source[1..2],
                error: None,
                telemetry: &four_telemetry[1],
            },
            WorkerCompletion {
                worker_index: 0,
                tiles: &source[0..1],
                error: None,
                telemetry: &four_telemetry[0],
            },
            WorkerCompletion {
                worker_index: 2,
                tiles: &source[2..3],
                error: None,
                telemetry: &four_telemetry[2],
            },
        ];
        let collect = |completions: &[WorkerCompletion<'_>]| {
            let mut scratch = [0; 4];
            let mut tiles = [CompletedTile::default(); 4];
            let mut merged = CertificateTelemetry::default();
            let count =
                collect_worker_completions(completions, &mut scratch, &mut tiles, &mut merged)
                    .unwrap();
            (debug_frame_digest(&tiles[..count]), merged)
        };
        assert_eq!(collect(&one), collect(&four));
    }

    #[test]
    fn any_worker_error_prevents_global_success() {
        let telemetry = CertificateTelemetry::default();
        let completions = [WorkerCompletion {
            worker_index: 0,
            tiles: &[],
            error: Some(SweepError::CertificateExhausted),
            telemetry: &telemetry,
        }];
        assert_eq!(
            collect_worker_completions(
                &completions,
                &mut [0],
                &mut [],
                &mut CertificateTelemetry::default(),
            ),
            Err(SweepError::CertificateExhausted)
        );
    }

    #[test]
    fn render_tile_writes_every_regular_and_event_pixel_exactly_once() {
        let visible = CertifiedRun {
            x0: 0,
            x1: 2,
            visible: Some(0),
            identity: IdentitySetId(3),
            q_model: super::super::sweep::QModel {
                q0: super::super::iv32::Iv32::new(100, 101).unwrap(),
                ..super::super::sweep::QModel::default()
            },
            ..CertifiedRun::default()
        };
        let background = CertifiedRun {
            x0: 3,
            x1: 4,
            ..CertifiedRun::default()
        };
        let records = [
            CoverageRecord {
                x0: 0,
                x1: 2,
                kind: 0,
                record_index: 0,
            },
            CoverageRecord {
                x0: 2,
                x1: 3,
                kind: 1,
                record_index: 0,
            },
            CoverageRecord {
                x0: 3,
                x1: 4,
                kind: 0,
                record_index: 1,
            },
        ];
        let event = debug_pixel(IdentitySetId(9), 50, 52, 127);
        let mut output = [DebugPixel {
            object_material_code: u32::MAX,
            q_class: u16::MAX,
            coverage: 1,
            flags: u8::MAX,
        }; 4];
        let tile = render_tile(
            7,
            0,
            4,
            0,
            1,
            &[0, 3],
            &records,
            &[visible, background],
            &[event],
            &mut output,
        )
        .unwrap();
        assert_eq!(output[0], output[1]);
        assert_eq!(output[2], event);
        assert_eq!(output[3], DebugPixel::default());
        assert_ne!(tile.digest, [0; 32]);
    }
}
