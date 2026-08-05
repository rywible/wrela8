//! Exact, allocation-free mutable renderer-state layout.

use super::verify::{VerifiedProjectiveProgram, VerifiedStructuralProgram};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateRegion {
    pub offset: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RendererStateLayout {
    pub total_bytes: u64,
    pub instrumented_total_bytes: u64,
    pub header: StateRegion,
    pub coefficient_snapshots: StateRegion,
    pub frame_snapshots: StateRegion,
    pub frame_complexes: StateRegion,
    pub worker_scratch: StateRegion,
    pub framebuffers: StateRegion,
    pub probes: StateRegion,
    pub kinetic: StateRegion,
    pub tile_descriptors: StateRegion,
    pub tile_ownership: StateRegion,
    pub failure: StateRegion,
    pub telemetry: StateRegion,
}

fn take(cursor: &mut u64, bytes: u64) -> Result<StateRegion, String> {
    let offset = *cursor;
    *cursor = cursor
        .checked_add(bytes)
        .ok_or_else(|| "P025: renderer-state offset overflow".to_string())?;
    Ok(StateRegion { offset, bytes })
}

fn align(cursor: &mut u64, alignment: u64, what: &str) -> Result<(), String> {
    *cursor = cursor
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(|| format!("P025: {what} alignment overflow"))?;
    Ok(())
}

pub fn layout(
    structural: &VerifiedStructuralProgram,
    projective: &VerifiedProjectiveProgram,
) -> Result<RendererStateLayout, String> {
    let capacities = &structural.program().capacities;
    let final_capacities = &projective.program().capacities;
    let mut cursor = 0_u64;
    let header = take(&mut cursor, capacities.state_header_bytes)?;
    let coefficient_snapshots = take(&mut cursor, capacities.coefficient_snapshot_bytes)?;
    let frame_snapshots = take(&mut cursor, capacities.frame_dependency_snapshot_bytes)?;
    let frame_complexes = take(&mut cursor, capacities.frame_complex_double_buffer_bytes)?;
    let worker_scratch = take(&mut cursor, final_capacities.final_all_worker_scratch_bytes)?;
    align(
        &mut cursor,
        wrela_machine::layout::PIXELS_STATE_PAGE_ALIGNMENT,
        "framebuffer",
    )?;
    let framebuffers = take(&mut cursor, capacities.output_double_buffer_bytes)?;
    if capacities.probe_bytes != 0 {
        align(
            &mut cursor,
            wrela_machine::layout::PIXELS_STATE_PAGE_ALIGNMENT,
            "probe",
        )?;
    }
    let probes = take(&mut cursor, capacities.probe_bytes)?;
    let kinetic = take(&mut cursor, capacities.kinetic_certificate_bytes)?;
    let tile_descriptors = take(&mut cursor, capacities.tile_descriptor_bytes)?;
    let tile_ownership = take(&mut cursor, capacities.tile_ownership_bytes)?;
    let failure = take(&mut cursor, capacities.failure_record_bytes)?;
    if cursor != final_capacities.total_renderer_state_bytes {
        return Err(format!(
            "pixels::state: derived {} bytes but final P4 capacity sealed {}",
            cursor, final_capacities.total_renderer_state_bytes
        ));
    }
    let telemetry = StateRegion {
        offset: cursor,
        bytes: capacities.telemetry_bytes_instrumented,
    };
    let instrumented_total_bytes = cursor
        .checked_add(telemetry.bytes)
        .ok_or_else(|| "P025: instrumented renderer-state size overflow".to_string())?;
    if instrumented_total_bytes != final_capacities.total_renderer_state_bytes_instrumented {
        return Err(format!(
            "pixels::state: instrumented layout derives {instrumented_total_bytes} bytes but \
             final P4 capacity sealed {}",
            final_capacities.total_renderer_state_bytes_instrumented
        ));
    }
    Ok(RendererStateLayout {
        total_bytes: cursor,
        instrumented_total_bytes,
        header,
        coefficient_snapshots,
        frame_snapshots,
        frame_complexes,
        worker_scratch,
        framebuffers,
        probes,
        kinetic,
        tile_descriptors,
        tile_ownership,
        failure,
        telemetry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_regions_are_contiguous_and_nonoverlapping() {
        let mut cursor = 0;
        let first = take(&mut cursor, 17).unwrap();
        let empty = take(&mut cursor, 0).unwrap();
        let second = take(&mut cursor, 9).unwrap();
        assert_eq!(
            first,
            StateRegion {
                offset: 0,
                bytes: 17
            }
        );
        assert_eq!(
            empty,
            StateRegion {
                offset: 17,
                bytes: 0
            }
        );
        assert_eq!(
            second,
            StateRegion {
                offset: 17,
                bytes: 9
            }
        );
        assert_eq!(cursor, 26);
    }

    #[test]
    fn framebuffer_and_probe_alignment_is_page_exact_and_checked() {
        let page = wrela_machine::layout::PIXELS_STATE_PAGE_ALIGNMENT;
        let mut cursor = page + 1;
        align(&mut cursor, page, "test").unwrap();
        assert_eq!(cursor, page * 2);
        let mut exact = page * 3;
        align(&mut exact, page, "test").unwrap();
        assert_eq!(exact, page * 3);
        let mut overflow = u64::MAX;
        assert!(align(&mut overflow, page, "test").is_err());
    }
}
