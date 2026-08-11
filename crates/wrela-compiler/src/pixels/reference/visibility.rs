//! Integrated, bounded P7 visibility-tile orchestration.
//!
//! The compiler-specific program reader implements [`VisibilityProgram`].
//! Keeping the driver expressed in terms of fixed caller-owned slices makes
//! the completeness and failure boundaries testable without introducing a
//! second heap-backed renderer.

use super::csg::CsgInstruction;
use super::events::{
    EventCorridor, EventError, EventInterval, RegularDomain, partition_row_events,
};
use super::frame::{CompletedTile, DebugPixel};
use super::rebuild::{
    RebuildCell, RebuildError, RebuildLimits, RebuildTier, TierResult, resolve_bounded,
};
use super::sweep::{
    CandidateCompleteness, CertifiedRun, FeatureId, IndexedFeature, RootSheet, RowProposal,
    SweepError, certify_regular_run, enumerate_row_candidates, seed_next_row,
};
use super::telemetry::CertificateTelemetry;

pub const MAX_P7_TILE_PIXELS: usize = 16 * 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileDomain {
    pub tile_id: u32,
    pub x0: u16,
    pub x1: u16,
    pub y0: u16,
    pub y1: u16,
}

impl TileDomain {
    fn dimensions(self) -> Result<(usize, usize), SweepError> {
        let width = self
            .x1
            .checked_sub(self.x0)
            .filter(|width| *width != 0)
            .map(usize::from)
            .ok_or(SweepError::InternalInvariant)?;
        let height = self
            .y1
            .checked_sub(self.y0)
            .filter(|height| *height != 0)
            .map(usize::from)
            .ok_or(SweepError::InternalInvariant)?;
        if width
            .checked_mul(height)
            .filter(|pixels| *pixels <= MAX_P7_TILE_PIXELS)
            .is_none()
        {
            return Err(SweepError::CapacityExceeded);
        }
        Ok((width, height))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RootIsolationSummary {
    /// Number of complete roots written to the supplied output slice.
    pub root_count: u16,
    /// True only when the feature's complete support sublevel was searched.
    pub complete: bool,
}

/// Sealed-program operations needed by the generic tile driver.
///
/// Implementations must never return a successful, incomplete root list.
/// Numeric ambiguity is an error or a rebuild request, not an empty list.
pub trait VisibilityProgram {
    fn indexed_features(
        &self,
        tile: TileDomain,
        row: u16,
        output: &mut [IndexedFeature],
    ) -> Result<usize, SweepError>;

    fn isolate_feature_roots(
        &self,
        feature: FeatureId,
        row: u16,
        x_anchor: u16,
        output: &mut [RootSheet],
    ) -> Result<RootIsolationSummary, SweepError>;

    fn isolate_row_events(
        &self,
        tile: TileDomain,
        row: u16,
        output: &mut [EventInterval],
    ) -> Result<usize, SweepError>;

    fn csg_program(&self) -> &[CsgInstruction];

    fn initial_inside_bits(&self, row: u16, x_anchor: u16) -> Result<u64, SweepError>;

    fn row_proposal_mode(&self) -> RowProposal {
        RowProposal::Enabled
    }

    fn revalidate_row_proposal(&self, _feature: FeatureId, _row: u16) -> Result<bool, SweepError> {
        Ok(true)
    }

    /// Attempt one fixed rebuild tier for one pixel cell.
    ///
    /// The driver controls tier order and all depth/cell limits. The program
    /// may resolve at a tier or explicitly report that the next tier is
    /// required; it cannot reorder or repeat tiers.
    fn rebuild_pixel(
        &self,
        tier: RebuildTier,
        row: u16,
        cell: RebuildCell,
        events: &[EventInterval],
    ) -> TierResult<DebugPixel>;
}

#[derive(Debug)]
pub struct VisibilityWorkspace<
    const FEATURES: usize,
    const ROOTS: usize,
    const EVENTS: usize,
    const DOMAINS: usize,
    const REBUILDS: usize,
> {
    indexed: [IndexedFeature; FEATURES],
    candidates: [FeatureId; FEATURES],
    seeded_candidates: [FeatureId; FEATURES],
    previous_candidates: [FeatureId; FEATURES],
    previous_candidate_count: usize,
    sheets: [RootSheet; ROOTS],
    event_input: [EventInterval; EVENTS],
    event_sorted: [EventInterval; EVENTS],
    corridors: [EventCorridor; EVENTS],
    regular: [RegularDomain; DOMAINS],
    rebuild_stack: [RebuildCell; REBUILDS],
    rebuild_output: [DebugPixel; REBUILDS],
    runs: [CertifiedRun; DOMAINS],
    run_count: usize,
    written: [bool; MAX_P7_TILE_PIXELS],
    pub telemetry: CertificateTelemetry,
}

impl<
    const FEATURES: usize,
    const ROOTS: usize,
    const EVENTS: usize,
    const DOMAINS: usize,
    const REBUILDS: usize,
> Default for VisibilityWorkspace<FEATURES, ROOTS, EVENTS, DOMAINS, REBUILDS>
{
    fn default() -> Self {
        Self {
            indexed: [IndexedFeature::default(); FEATURES],
            candidates: [FeatureId::default(); FEATURES],
            seeded_candidates: [FeatureId::default(); FEATURES],
            previous_candidates: [FeatureId::default(); FEATURES],
            previous_candidate_count: 0,
            sheets: [RootSheet::default(); ROOTS],
            event_input: [EventInterval::default(); EVENTS],
            event_sorted: [EventInterval::default(); EVENTS],
            corridors: [EventCorridor::default(); EVENTS],
            regular: [RegularDomain::default(); DOMAINS],
            rebuild_stack: [RebuildCell::default(); REBUILDS],
            rebuild_output: [DebugPixel::default(); REBUILDS],
            runs: [CertifiedRun::default(); DOMAINS],
            run_count: 0,
            written: [false; MAX_P7_TILE_PIXELS],
            telemetry: CertificateTelemetry::default(),
        }
    }
}

impl<
    const FEATURES: usize,
    const ROOTS: usize,
    const EVENTS: usize,
    const DOMAINS: usize,
    const REBUILDS: usize,
> VisibilityWorkspace<FEATURES, ROOTS, EVENTS, DOMAINS, REBUILDS>
{
    fn reset_for_tile(&mut self) {
        self.written.fill(false);
        self.previous_candidate_count = 0;
        self.run_count = 0;
        self.telemetry = CertificateTelemetry::default();
    }

    pub fn runs(&self) -> &[CertifiedRun] {
        &self.runs[..self.run_count]
    }
}

fn map_event_error(error: EventError) -> SweepError {
    match error {
        EventError::CapacityExceeded => SweepError::CapacityExceeded,
        EventError::InvalidDomain | EventError::InvalidEvent => SweepError::InternalInvariant,
    }
}

fn map_rebuild_error(error: RebuildError) -> SweepError {
    match error {
        RebuildError::CapacityExceeded => SweepError::CapacityExceeded,
        RebuildError::CertificateExhausted => SweepError::CertificateExhausted,
        RebuildError::NumericFailure => SweepError::NumericFailure,
        RebuildError::InvalidDomain => SweepError::InternalInvariant,
    }
}

fn order_sheets(sheets: &mut [RootSheet]) -> Result<(), SweepError> {
    for index in 1..sheets.len() {
        let value = sheets[index];
        let mut destination = index;
        while destination != 0
            && (value.q_domain.lo, value.q_domain.hi, value.root.feature)
                > (
                    sheets[destination - 1].q_domain.lo,
                    sheets[destination - 1].q_domain.hi,
                    sheets[destination - 1].root.feature,
                )
        {
            sheets[destination] = sheets[destination - 1];
            destination -= 1;
        }
        sheets[destination] = value;
    }
    if sheets
        .windows(2)
        .any(|pair| pair[0].q_domain.lo <= pair[1].q_domain.hi)
    {
        return Err(SweepError::CertificateExhausted);
    }
    Ok(())
}

fn raster_run(
    run: CertifiedRun,
    tile: TileDomain,
    row: u16,
    width: usize,
    output: &mut [DebugPixel],
    written: &mut [bool; MAX_P7_TILE_PIXELS],
) -> Result<(), SweepError> {
    let row_offset = usize::from(row - tile.y0)
        .checked_mul(width)
        .ok_or(SweepError::CapacityExceeded)?;
    let pixel = if run.visible.is_some() {
        super::frame::debug_pixel(run.identity, run.q_model.q0.lo, run.q_model.q0.hi, 255)
    } else {
        DebugPixel::default()
    };
    for x in run.x0..run.x1 {
        let index = row_offset
            .checked_add(usize::from(x - tile.x0))
            .ok_or(SweepError::CapacityExceeded)?;
        let destination = output.get_mut(index).ok_or(SweepError::InternalInvariant)?;
        let marker = written
            .get_mut(index)
            .ok_or(SweepError::InternalInvariant)?;
        if *marker {
            return Err(SweepError::InternalInvariant);
        }
        *destination = pixel;
        *marker = true;
    }
    Ok(())
}

fn write_rebuild_pixel(
    pixel: DebugPixel,
    tile: TileDomain,
    row: u16,
    x: u16,
    width: usize,
    output: &mut [DebugPixel],
    written: &mut [bool; MAX_P7_TILE_PIXELS],
) -> Result<(), SweepError> {
    let index = usize::from(row - tile.y0)
        .checked_mul(width)
        .and_then(|offset| offset.checked_add(usize::from(x - tile.x0)))
        .ok_or(SweepError::CapacityExceeded)?;
    let destination = output.get_mut(index).ok_or(SweepError::InternalInvariant)?;
    let marker = written
        .get_mut(index)
        .ok_or(SweepError::InternalInvariant)?;
    if *marker {
        return Err(SweepError::InternalInvariant);
    }
    *destination = pixel;
    *marker = true;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rebuild_domain<P: VisibilityProgram>(
    program: &P,
    tile: TileDomain,
    row: u16,
    x0: u16,
    x1: u16,
    events: &[EventInterval],
    limits: RebuildLimits,
    width: usize,
    output: &mut [DebugPixel],
    written: &mut [bool; MAX_P7_TILE_PIXELS],
    stack: &mut [RebuildCell],
    rebuild_output: &mut [DebugPixel],
    telemetry: &mut CertificateTelemetry,
) -> Result<(), SweepError> {
    for x in x0..x1 {
        let count = resolve_bounded(
            RebuildCell {
                x0: x,
                x1: x + 1,
                q_lo: i32::MIN,
                q_hi: i32::MAX,
                x_depth: 0,
                q_depth: 0,
            },
            limits,
            stack,
            rebuild_output,
            |tier, cell| program.rebuild_pixel(tier, row, cell, events),
            Some(telemetry),
        )
        .map_err(map_rebuild_error)?;
        if count == 0 {
            return Err(SweepError::InternalInvariant);
        }
        // A q split searches disjoint depth subdomains for the same output
        // pixel. It is safe to collapse those leaves only when every
        // independently certified subdomain selects identical visibility.
        if rebuild_output[1..count]
            .iter()
            .any(|candidate| *candidate != rebuild_output[0])
        {
            return Err(SweepError::CertificateExhausted);
        }
        write_rebuild_pixel(rebuild_output[0], tile, row, x, width, output, written)?;
        telemetry.corridor_pixels = telemetry
            .corridor_pixels
            .checked_add(1)
            .ok_or(SweepError::CapacityExceeded)?;
    }
    Ok(())
}

/// Construct one complete debug-visibility tile from sealed structural data.
///
/// Success means every pixel was written exactly once by a certified regular
/// run or a bounded, explicitly resolved event/rebuild cell.
pub fn render_visibility_tile<
    P: VisibilityProgram,
    const FEATURES: usize,
    const ROOTS: usize,
    const EVENTS: usize,
    const DOMAINS: usize,
    const REBUILDS: usize,
>(
    program: &P,
    tile: TileDomain,
    rebuild_limits: RebuildLimits,
    workspace: &mut VisibilityWorkspace<FEATURES, ROOTS, EVENTS, DOMAINS, REBUILDS>,
    output: &mut [DebugPixel],
) -> Result<CompletedTile, SweepError> {
    let (width, height) = tile.dimensions()?;
    if output.len() != width * height {
        return Err(SweepError::InternalInvariant);
    }
    workspace.reset_for_tile();

    for row in tile.y0..tile.y1 {
        let indexed_count = program.indexed_features(tile, row, &mut workspace.indexed)?;
        let indexed = workspace
            .indexed
            .get(..indexed_count)
            .ok_or(SweepError::CapacityExceeded)?;
        let (candidate_count, base_completeness) =
            enumerate_row_candidates(indexed, row, &mut workspace.candidates)?;
        let previous = &workspace.previous_candidates[..workspace.previous_candidate_count];
        let (seeded_count, proposal_counts) = seed_next_row(
            program.row_proposal_mode(),
            &workspace.candidates[..candidate_count],
            previous,
            &mut workspace.seeded_candidates,
            |feature| program.revalidate_row_proposal(feature, row),
        )?;
        workspace.telemetry.proposed_records = workspace
            .telemetry
            .proposed_records
            .checked_add(u64::from(proposal_counts.proposed))
            .ok_or(SweepError::CapacityExceeded)?;
        workspace.telemetry.revalidated_records = workspace
            .telemetry
            .revalidated_records
            .checked_add(u64::from(proposal_counts.revalidated))
            .ok_or(SweepError::CapacityExceeded)?;
        workspace.telemetry.new_records = workspace
            .telemetry
            .new_records
            .checked_add(u64::from(proposal_counts.new))
            .ok_or(SweepError::CapacityExceeded)?;
        workspace.previous_candidates[..seeded_count]
            .copy_from_slice(&workspace.seeded_candidates[..seeded_count]);
        workspace.previous_candidate_count = seeded_count;
        let candidates = &workspace.seeded_candidates[..seeded_count];

        let event_count = program.isolate_row_events(tile, row, &mut workspace.event_input)?;
        let event_input = workspace
            .event_input
            .get(..event_count)
            .ok_or(SweepError::CapacityExceeded)?;
        let partition = partition_row_events(
            tile.x0,
            tile.x1,
            event_input,
            &mut workspace.event_sorted,
            &mut workspace.corridors,
            &mut workspace.regular,
        )
        .map_err(map_event_error)?;

        for domain in partition.regular {
            let mut cursor = domain.x0;
            while cursor < domain.x1 {
                let mut sheet_count = 0_usize;
                let mut accounted = 0_u16;
                for feature in candidates {
                    let available = workspace
                        .sheets
                        .get_mut(sheet_count..)
                        .ok_or(SweepError::CapacityExceeded)?;
                    let isolated =
                        program.isolate_feature_roots(*feature, row, cursor, available)?;
                    if !isolated.complete {
                        return Err(SweepError::CertificateExhausted);
                    }
                    let roots = usize::from(isolated.root_count);
                    if roots > available.len()
                        || available[..roots]
                            .iter()
                            .any(|sheet| sheet.root.feature != *feature)
                    {
                        return Err(SweepError::InternalInvariant);
                    }
                    sheet_count = sheet_count
                        .checked_add(roots)
                        .ok_or(SweepError::CapacityExceeded)?;
                    accounted = accounted
                        .checked_add(1)
                        .ok_or(SweepError::CapacityExceeded)?;
                }
                let sheets = &mut workspace.sheets[..sheet_count];
                if let Err(SweepError::CertificateExhausted) = order_sheets(sheets) {
                    rebuild_domain(
                        program,
                        tile,
                        row,
                        cursor,
                        domain.x1,
                        &[],
                        rebuild_limits,
                        width,
                        output,
                        &mut workspace.written,
                        &mut workspace.rebuild_stack,
                        &mut workspace.rebuild_output,
                        &mut workspace.telemetry,
                    )?;
                    cursor = domain.x1;
                    continue;
                }
                order_sheets(sheets)?;
                let completeness = CandidateCompleteness {
                    roots_accounted: accounted,
                    ..base_completeness
                };
                let run = certify_regular_run(
                    cursor,
                    domain.x1,
                    sheets,
                    program.csg_program(),
                    program.initial_inside_bits(row, cursor)?,
                    completeness,
                    domain.left_corridor,
                    domain.right_corridor,
                    Some(&mut workspace.telemetry),
                )?;
                let run_slot = workspace
                    .runs
                    .get_mut(workspace.run_count)
                    .ok_or(SweepError::CapacityExceeded)?;
                *run_slot = run;
                workspace.run_count += 1;
                raster_run(run, tile, row, width, output, &mut workspace.written)?;
                cursor = run.x1;
            }
        }

        for corridor in partition.corridors {
            let first = usize::from(corridor.first_event);
            let end = first
                .checked_add(usize::from(corridor.event_count))
                .ok_or(SweepError::CapacityExceeded)?;
            let events = partition
                .sorted_events
                .get(first..end)
                .ok_or(SweepError::InternalInvariant)?;
            rebuild_domain(
                program,
                tile,
                row,
                corridor.x0,
                corridor.x1,
                events,
                rebuild_limits,
                width,
                output,
                &mut workspace.written,
                &mut workspace.rebuild_stack,
                &mut workspace.rebuild_output,
                &mut workspace.telemetry,
            )?;
        }
    }

    if workspace.written[..width * height]
        .iter()
        .any(|written| !written)
    {
        return Err(SweepError::InternalInvariant);
    }
    Ok(CompletedTile {
        tile_id: tile.tile_id,
        digest: debug_output_digest(output),
    })
}

fn debug_output_digest(pixels: &[DebugPixel]) -> [u8; 32] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::reference::iv32::Iv32;
    use crate::pixels::reference::sweep::{
        ExclusionResult, IdentitySetId, NormalModel, ObjectId, QModel, RootRecord,
    };

    struct PlaneProgram {
        event: bool,
        fail_rebuild: bool,
        proposal: RowProposal,
    }

    impl VisibilityProgram for PlaneProgram {
        fn indexed_features(
            &self,
            _tile: TileDomain,
            _row: u16,
            output: &mut [IndexedFeature],
        ) -> Result<usize, SweepError> {
            output[0] = IndexedFeature {
                id: FeatureId(3),
                row_start: 0,
                row_end: 16,
                exclusion: ExclusionResult::Retain,
            };
            Ok(1)
        }

        fn isolate_feature_roots(
            &self,
            feature: FeatureId,
            _row: u16,
            x_anchor: u16,
            output: &mut [RootSheet],
        ) -> Result<RootIsolationSummary, SweepError> {
            let q = Iv32::new(1024 + i32::from(x_anchor), 1025 + i32::from(x_anchor))
                .map_err(|_| SweepError::NumericFailure)?;
            output[0] = RootSheet {
                root: RootRecord {
                    feature,
                    object: ObjectId(0),
                    identity_set: IdentitySetId(9),
                    q,
                    orientation: 1,
                    validity_margin: 8,
                    root_slack: 8,
                    dedup_owner: 0,
                    support_sublevel_proof: true,
                },
                q_model: QModel {
                    q0: q,
                    qx: Iv32::point(1),
                    qxx: Iv32::point(0),
                },
                q_domain: q,
                q_error: Iv32::point(1),
                normal_model: NormalModel {
                    nx: Iv32::point(0),
                    ny: Iv32::point(0),
                    nz: Iv32::point(1),
                },
                q_order_slack: 8,
                root_slack: 8,
                feature_slack: 8,
                branch_slack: 8,
                fixed_q_slack: 8,
                expires_at: 16,
                method: 0,
                composition_shape: 1,
            };
            Ok(RootIsolationSummary {
                root_count: 1,
                complete: true,
            })
        }

        fn isolate_row_events(
            &self,
            _tile: TileDomain,
            _row: u16,
            output: &mut [EventInterval],
        ) -> Result<usize, SweepError> {
            if self.event {
                output[0] = EventInterval {
                    lo: 3,
                    hi: 4,
                    generator_id: 17,
                    subdivision_depth: 1,
                };
                Ok(1)
            } else {
                Ok(0)
            }
        }

        fn csg_program(&self) -> &[CsgInstruction] {
            const CSG: &[CsgInstruction] = &[CsgInstruction::Object(0)];
            CSG
        }

        fn initial_inside_bits(&self, _row: u16, _x_anchor: u16) -> Result<u64, SweepError> {
            Ok(0)
        }

        fn row_proposal_mode(&self) -> RowProposal {
            self.proposal
        }

        fn rebuild_pixel(
            &self,
            tier: RebuildTier,
            _row: u16,
            _cell: RebuildCell,
            events: &[EventInterval],
        ) -> TierResult<DebugPixel> {
            if self.fail_rebuild {
                return TierResult::Inconclusive;
            }
            if tier == RebuildTier::SubpixelIntegration && !events.is_empty() {
                TierResult::Resolved(DebugPixel {
                    object_material_code: 0x1234,
                    q_class: 7,
                    coverage: 128,
                    flags: 3,
                })
            } else {
                TierResult::Inconclusive
            }
        }
    }

    type Workspace = VisibilityWorkspace<4, 8, 4, 5, 16>;

    fn limits() -> RebuildLimits {
        RebuildLimits {
            max_x_depth: 0,
            max_q_depth: 0,
            max_cells: 1,
        }
    }

    #[test]
    fn integrated_plane_tile_has_exact_regular_and_event_coverage() {
        let mut workspace = Workspace::default();
        let mut output = [DebugPixel::default(); 8];
        let tile = render_visibility_tile(
            &PlaneProgram {
                event: true,
                fail_rebuild: false,
                proposal: RowProposal::Enabled,
            },
            TileDomain {
                tile_id: 2,
                x0: 0,
                x1: 8,
                y0: 0,
                y1: 1,
            },
            limits(),
            &mut workspace,
            &mut output,
        )
        .unwrap();
        assert_eq!(tile.tile_id, 2);
        assert!(output.iter().all(|pixel| pixel.flags != 0));
        assert_eq!(output[3].coverage, 128);
        assert!(output[..3].iter().all(|pixel| pixel.coverage == 255));
        assert!(output[4..].iter().all(|pixel| pixel.coverage == 255));
        assert_eq!(workspace.runs().len(), 2);
        assert_eq!(workspace.telemetry.regular_pixels, 7);
        assert_eq!(workspace.telemetry.corridor_pixels, 1);
    }

    #[test]
    fn unresolved_event_pixel_fails_without_leaving_a_successful_tile() {
        let mut workspace = Workspace::default();
        let mut output = [DebugPixel::default(); 8];
        assert_eq!(
            render_visibility_tile(
                &PlaneProgram {
                    event: true,
                    fail_rebuild: true,
                    proposal: RowProposal::Enabled,
                },
                TileDomain {
                    tile_id: 2,
                    x0: 0,
                    x1: 8,
                    y0: 0,
                    y1: 1,
                },
                limits(),
                &mut workspace,
                &mut output,
            ),
            Err(SweepError::CertificateExhausted)
        );
    }

    #[test]
    fn structural_candidate_without_complete_root_search_fails_closed() {
        struct Incomplete(PlaneProgram);
        impl VisibilityProgram for Incomplete {
            fn indexed_features(
                &self,
                tile: TileDomain,
                row: u16,
                output: &mut [IndexedFeature],
            ) -> Result<usize, SweepError> {
                self.0.indexed_features(tile, row, output)
            }
            fn isolate_feature_roots(
                &self,
                _feature: FeatureId,
                _row: u16,
                _x_anchor: u16,
                _output: &mut [RootSheet],
            ) -> Result<RootIsolationSummary, SweepError> {
                Ok(RootIsolationSummary {
                    root_count: 0,
                    complete: false,
                })
            }
            fn isolate_row_events(
                &self,
                tile: TileDomain,
                row: u16,
                output: &mut [EventInterval],
            ) -> Result<usize, SweepError> {
                self.0.isolate_row_events(tile, row, output)
            }
            fn csg_program(&self) -> &[CsgInstruction] {
                self.0.csg_program()
            }
            fn initial_inside_bits(&self, row: u16, x_anchor: u16) -> Result<u64, SweepError> {
                self.0.initial_inside_bits(row, x_anchor)
            }
            fn rebuild_pixel(
                &self,
                tier: RebuildTier,
                row: u16,
                cell: RebuildCell,
                events: &[EventInterval],
            ) -> TierResult<DebugPixel> {
                self.0.rebuild_pixel(tier, row, cell, events)
            }
        }
        let mut workspace = Workspace::default();
        let mut output = [DebugPixel::default(); 8];
        assert_eq!(
            render_visibility_tile(
                &Incomplete(PlaneProgram {
                    event: false,
                    fail_rebuild: false,
                    proposal: RowProposal::Enabled,
                }),
                TileDomain {
                    tile_id: 0,
                    x0: 0,
                    x1: 8,
                    y0: 0,
                    y1: 1,
                },
                limits(),
                &mut workspace,
                &mut output,
            ),
            Err(SweepError::CertificateExhausted)
        );
    }

    #[test]
    fn row_proposals_change_only_diagnostic_counts() {
        let domain = TileDomain {
            tile_id: 4,
            x0: 0,
            x1: 4,
            y0: 0,
            y1: 2,
        };
        let mut enabled_workspace = Workspace::default();
        let mut disabled_workspace = Workspace::default();
        let mut enabled = [DebugPixel::default(); 8];
        let mut disabled = [DebugPixel::default(); 8];
        let enabled_tile = render_visibility_tile(
            &PlaneProgram {
                event: false,
                fail_rebuild: false,
                proposal: RowProposal::Enabled,
            },
            domain,
            limits(),
            &mut enabled_workspace,
            &mut enabled,
        )
        .unwrap();
        let disabled_tile = render_visibility_tile(
            &PlaneProgram {
                event: false,
                fail_rebuild: false,
                proposal: RowProposal::Disabled,
            },
            domain,
            limits(),
            &mut disabled_workspace,
            &mut disabled,
        )
        .unwrap();
        assert_eq!(enabled, disabled);
        assert_eq!(enabled_tile, disabled_tile);
        assert_eq!(enabled_workspace.telemetry.proposed_records, 1);
        assert_eq!(enabled_workspace.telemetry.revalidated_records, 1);
        assert_eq!(enabled_workspace.telemetry.new_records, 1);
        assert_eq!(disabled_workspace.telemetry.proposed_records, 0);
        assert_eq!(disabled_workspace.telemetry.revalidated_records, 0);
        assert_eq!(disabled_workspace.telemetry.new_records, 2);
    }
}
