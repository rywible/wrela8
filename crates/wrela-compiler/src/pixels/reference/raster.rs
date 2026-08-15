//! P8 compact raster records and deterministic BGRA scanout reference.
//!
//! Sweep certificates remain proof-rich.  This module is the deliberately
//! small boundary between those certificates and the hot raster loop: after
//! lowering, neither a feature list nor a root-isolation stack is reachable.

use super::fixed_q::{self, QRunScalar};
use super::iv32::{FixedDomain, Iv32, NumericError};
use super::sweep::{CertifiedRun, IdentitySetId, SweepError};

pub const TILE_WIDTH: usize = 64;
pub const TILE_HEIGHT: usize = 32;
pub const TILE_STRIDE: usize = TILE_WIDTH * 4;
pub const TILE_BYTES: usize = TILE_STRIDE * TILE_HEIGHT;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AffineRunSetup {
    pub value: i32,
    pub delta: i32,
    pub error_radius: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputProofCode(pub u8);

impl OutputProofCode {
    pub const GEOMETRY: u8 = 1 << 0;
    pub const IDENTITY: u8 = 1 << 1;
    pub const DEBUG_RGB: u8 = 1 << 2;
    pub const BACKGROUND: u8 = 1 << 3;

    pub const fn contains(self, flag: u8) -> bool {
        self.0 & flag == flag
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterRun {
    pub x0: u16,
    pub x1: u16,
    pub q: QRunScalar,
    pub q_u: AffineRunSetup,
    pub q_v: AffineRunSetup,
    pub identity: IdentitySetId,
    pub material_summary: u32,
    pub light_summary: u32,
    pub proof_code: OutputProofCode,
    /// Guest debug-blue code derived before compact symmetric recentering.
    pub debug_q_width: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventPixel {
    pub x: u16,
    pub coverage: Iv32,
    pub front_run: u16,
    pub back_run: u16,
    pub first_event: u16,
    pub event_count: u16,
}

pub fn lower_certified_run(
    run: CertifiedRun,
    domain: FixedDomain,
    material_summary: u32,
    light_summary: u32,
) -> Result<RasterRun, SweepError> {
    if run.x0 >= run.x1 || run.event_left.is_some() || run.event_right.is_some() {
        return Err(SweepError::InternalInvariant);
    }
    let width = u8::try_from(run.x1 - run.x0).map_err(|_| SweepError::CapacityExceeded)?;
    let error_radius = interval_abs_max(run.q_error)?;
    let q = fixed_q::setup(
        run.q_model.q0,
        run.q_model.qx,
        run.q_model.qxx,
        domain,
        width,
        error_radius,
    )
    .map_err(map_numeric)?;
    if q.microtile_width == 0
        || q.microtile_width > 64
        || !q.microtile_width.is_power_of_two()
        || u16::from(q.microtile_width) < run.x1 - run.x0
    {
        // The caller must split and re-anchor before producing the compact
        // record; a hot record never carries an implicit reset table.
        return Err(SweepError::CertificateExhausted);
    }
    let mut proof = OutputProofCode::GEOMETRY | OutputProofCode::IDENTITY;
    if run.visible.is_some() {
        proof |= OutputProofCode::DEBUG_RGB;
    } else {
        proof |= OutputProofCode::BACKGROUND | OutputProofCode::DEBUG_RGB;
    }
    // P8 debug identity does not consume a material or light program.  The
    // IDs are nevertheless carried now so P9 can populate the predeclared
    // summaries without changing this record boundary.
    Ok(RasterRun {
        x0: run.x0,
        x1: run.x1,
        q,
        q_u: affine_from_interval(run.q_u)?,
        q_v: affine_from_interval(run.q_v)?,
        identity: run.identity,
        material_summary,
        light_summary,
        proof_code: OutputProofCode(proof | ((run.proof_owner as u8) << 4)),
        debug_q_width: source_q_width_class(run.q_model.q0, run.q_error)?,
    })
}

pub fn lower_certified_runs(
    run: CertifiedRun,
    domain: FixedDomain,
    material_summary: u32,
    light_summary: u32,
    output: &mut [RasterRun],
) -> Result<usize, SweepError> {
    if run.x0 >= run.x1 || run.event_left.is_some() || run.event_right.is_some() {
        return Err(SweepError::InternalInvariant);
    }
    let mut count = 0_usize;
    let mut x0 = run.x0;
    while x0 < run.x1 {
        let remaining = (run.x1 - x0).min(32);
        let width = 1_u16 << (15 - remaining.leading_zeros());
        let offset = i64::from(x0 - run.x0);
        let triangular = offset * offset.saturating_sub(1) / 2;
        let advance = |q0: Iv32, qx: Iv32, qxx: Iv32| -> Result<Iv32, SweepError> {
            let endpoint = |a: i32, b: i32, c: i32| -> Result<i32, SweepError> {
                i32::try_from(i64::from(a) + offset * i64::from(b) + triangular * i64::from(c))
                    .map_err(|_| SweepError::NumericFailure)
            };
            let candidates = [
                endpoint(q0.lo, qx.lo, qxx.lo)?,
                endpoint(q0.lo, qx.lo, qxx.hi)?,
                endpoint(q0.lo, qx.hi, qxx.lo)?,
                endpoint(q0.lo, qx.hi, qxx.hi)?,
                endpoint(q0.hi, qx.lo, qxx.lo)?,
                endpoint(q0.hi, qx.lo, qxx.hi)?,
                endpoint(q0.hi, qx.hi, qxx.lo)?,
                endpoint(q0.hi, qx.hi, qxx.hi)?,
            ];
            Iv32::new(
                *candidates.iter().min().unwrap(),
                *candidates.iter().max().unwrap(),
            )
            .map_err(map_numeric)
        };
        let advance_delta = |qx: Iv32, qxx: Iv32| -> Result<Iv32, SweepError> {
            let values = [
                i64::from(qx.lo) + offset * i64::from(qxx.lo),
                i64::from(qx.lo) + offset * i64::from(qxx.hi),
                i64::from(qx.hi) + offset * i64::from(qxx.lo),
                i64::from(qx.hi) + offset * i64::from(qxx.hi),
            ];
            Iv32::new(
                i32::try_from(*values.iter().min().unwrap())
                    .map_err(|_| SweepError::NumericFailure)?,
                i32::try_from(*values.iter().max().unwrap())
                    .map_err(|_| SweepError::NumericFailure)?,
            )
            .map_err(map_numeric)
        };
        let mut piece = run;
        piece.x0 = x0;
        piece.x1 = x0 + width;
        piece.q_model.q0 = advance(run.q_model.q0, run.q_model.qx, run.q_model.qxx)?;
        piece.q_model.qx = advance_delta(run.q_model.qx, run.q_model.qxx)?;
        let mut lowered = lower_certified_run(piece, domain, material_summary, light_summary)?;
        lowered.debug_q_width = source_q_width_class(run.q_model.q0, run.q_error)?;
        *output.get_mut(count).ok_or(SweepError::CapacityExceeded)? = lowered;
        count += 1;
        x0 += width;
    }
    Ok(count)
}

fn affine_from_interval(value: Iv32) -> Result<AffineRunSetup, SweepError> {
    let center = i32::try_from(
        i64::from(value.lo) + (i64::from(value.hi) - i64::from(value.lo)).div_euclid(2),
    )
    .map_err(|_| SweepError::NumericFailure)?;
    Ok(AffineRunSetup {
        value: center,
        delta: 0,
        error_radius: i32::try_from(
            (i64::from(value.lo) - i64::from(center))
                .unsigned_abs()
                .max((i64::from(value.hi) - i64::from(center)).unsigned_abs()),
        )
        .map_err(|_| SweepError::NumericFailure)?,
    })
}

fn interval_abs_max(value: Iv32) -> Result<i32, SweepError> {
    i32::try_from(
        i64::from(value.lo)
            .unsigned_abs()
            .max(i64::from(value.hi).unsigned_abs()),
    )
    .map_err(|_| SweepError::NumericFailure)
}

fn map_numeric(error: NumericError) -> SweepError {
    match error {
        NumericError::CapacityExceeded => SweepError::CapacityExceeded,
        NumericError::Overflow | NumericError::NonFinite => SweepError::NumericFailure,
        _ => SweepError::CertificateExhausted,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct I32x4([i32; 4]);

impl I32x4 {
    fn add(self, other: Self) -> Option<Self> {
        Some(Self([
            self.0[0].checked_add(other.0[0])?,
            self.0[1].checked_add(other.0[1])?,
            self.0[2].checked_add(other.0[2])?,
            self.0[3].checked_add(other.0[3])?,
        ]))
    }
}

pub fn raster_run_scalar(
    run: &RasterRun,
    tile_x: u16,
    tile_y: u16,
    row: u16,
    visible_width: u16,
    visible_height: u16,
    bytes: &mut [u8; TILE_BYTES],
) -> Result<(), SweepError> {
    validate_destination(run, tile_x, tile_y, row, visible_width, visible_height)?;
    let color = debug_identity_bgra(run.identity, run.proof_code, 255, run.debug_q_width);
    let mut x = run.x0;
    while x < run.x1 {
        let anchor = x;
        let reset = (run.x1 - x).min(u16::from(run.q.microtile_width));
        let mut recurrence = recurrence_at(run.q, u32::from(anchor - run.x0))?;
        for lane in 0..reset {
            let _q_with_radius = (recurrence.q, recurrence.error_radius);
            write_pixel(bytes, tile_x, tile_y, x + lane, row, color)?;
            if lane + 1 < reset {
                recurrence.advance().map_err(map_numeric)?;
            }
        }
        x = x.checked_add(reset).ok_or(SweepError::CapacityExceeded)?;
    }
    if x != run.x1 {
        return Err(SweepError::InternalInvariant);
    }
    Ok(())
}

pub fn raster_run4(
    run: &RasterRun,
    tile_x: u16,
    tile_y: u16,
    row: u16,
    visible_width: u16,
    visible_height: u16,
    bytes: &mut [u8; TILE_BYTES],
) -> Result<(), SweepError> {
    validate_destination(run, tile_x, tile_y, row, visible_width, visible_height)?;
    let color = debug_identity_bgra(run.identity, run.proof_code, 255, run.debug_q_width);
    let mut x = run.x0;
    while x < run.x1 && (x - run.x0) % 4 != 0 {
        write_pixel(bytes, tile_x, tile_y, x, row, color)?;
        x += 1;
    }
    while run.x1 - x >= 4 {
        let n = u32::from(x - run.x0);
        let scalar = recurrence_at(run.q, n)?;
        let q0 = i64::from(scalar.q);
        let d1 = i64::from(scalar.dq);
        let d2 = i64::from(scalar.ddq);
        let lanes = I32x4([
            i32::try_from(q0).map_err(|_| SweepError::NumericFailure)?,
            i32::try_from(q0 + d1).map_err(|_| SweepError::NumericFailure)?,
            i32::try_from(q0 + 2 * d1 + d2).map_err(|_| SweepError::NumericFailure)?,
            i32::try_from(q0 + 3 * d1 + 3 * d2).map_err(|_| SweepError::NumericFailure)?,
        ]);
        // Express the packet carry as the sealed vector recurrence.  The
        // ordinary adds are valid only because lowering proved all lanes fit.
        let next = lanes
            .add(I32x4(
                [i32::try_from(4 * d1 + 6 * d2).map_err(|_| SweepError::NumericFailure)?; 4],
            ))
            .ok_or(SweepError::NumericFailure)?;
        let _carry_witness = next.0[0];
        for lane in 0..4_u16 {
            let _q_with_radius = (lanes.0[usize::from(lane)], scalar.error_radius);
            write_pixel(bytes, tile_x, tile_y, x + lane, row, color)?;
        }
        x += 4;
    }
    while x < run.x1 {
        write_pixel(bytes, tile_x, tile_y, x, row, color)?;
        x += 1;
    }
    Ok(())
}

fn recurrence_at(run: QRunScalar, steps: u32) -> Result<QRunScalar, SweepError> {
    // Reset anchors are compiler data.  Reconstructing one in the reference
    // is intentionally cold and gives the scalar/packet differential oracle
    // an independent final-index check.
    if steps > 65_535 {
        return Err(SweepError::CapacityExceeded);
    }
    run.reset_at(steps).map_err(map_numeric)
}

fn validate_destination(
    run: &RasterRun,
    tile_x: u16,
    tile_y: u16,
    row: u16,
    visible_width: u16,
    visible_height: u16,
) -> Result<(), SweepError> {
    if run.x0 >= run.x1
        || visible_width == 0
        || visible_height == 0
        || usize::from(visible_width) > TILE_WIDTH
        || usize::from(visible_height) > TILE_HEIGHT
        || row < tile_y
        || row >= tile_y + visible_height
        || run.x0 < tile_x
        || run.x1 > tile_x + visible_width
    {
        return Err(SweepError::InternalInvariant);
    }
    Ok(())
}

pub fn raster_event_pixel(
    event: EventPixel,
    event_arena: &[u16],
    tile_x: u16,
    tile_y: u16,
    row: u16,
    visible_width: u16,
    visible_height: u16,
    front: [u8; 4],
    back: [u8; 4],
    bytes: &mut [u8; TILE_BYTES],
) -> Result<(), SweepError> {
    if event.event_count == 0 || event.x < tile_x || event.x >= tile_x + visible_width {
        return Err(SweepError::InternalInvariant);
    }
    let first = usize::from(event.first_event);
    let end = first
        .checked_add(usize::from(event.event_count))
        .filter(|end| *end <= event_arena.len())
        .ok_or(SweepError::InternalInvariant)?;
    let referenced = &event_arena[first..end];
    if referenced.is_empty() || referenced.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(SweepError::InternalInvariant);
    }
    let coverage = singleton_coverage_byte(event.coverage)?;
    let mut color = [0_u8; 4];
    for channel in 0..3 {
        let numerator = u32::from(front[channel]) * u32::from(coverage)
            + u32::from(back[channel]) * (255 - u32::from(coverage));
        color[channel] =
            u8::try_from((numerator + 127) / 255).map_err(|_| SweepError::InternalInvariant)?;
    }
    color[3] = 255;
    if row < tile_y || row >= tile_y + visible_height {
        return Err(SweepError::InternalInvariant);
    }
    write_pixel(bytes, tile_x, tile_y, event.x, row, color)
}

fn singleton_coverage_byte(coverage: Iv32) -> Result<u8, SweepError> {
    if coverage.lo < 0 || coverage.hi > 256 || coverage.lo > coverage.hi {
        return Err(SweepError::InternalInvariant);
    }
    let encode = |raw: i32| ((u32::try_from(raw).unwrap_or(0) * 255 + 128) / 256) as u8;
    let lo = encode(coverage.lo);
    let hi = encode(coverage.hi);
    if lo != hi {
        return Err(SweepError::CertificateExhausted);
    }
    Ok(lo)
}

/// Host mirror of `core.render_raster.raster_rsqrt`.
///
/// The explicit seed-then-two-Newton-steps ordering is part of the sealed
/// numeric contract rather than an optimisation detail, so it is written out
/// here the same way instead of folded into a single `powf(-0.5)`: a host that
/// reassociated the refinement would stop being an oracle for the guest.
pub fn raster_rsqrt(value: f32) -> f32 {
    if value <= 0.0 {
        return 0.0;
    }
    let mut estimate = 1.0 / value.sqrt();
    estimate *= 1.5 - 0.5 * value * estimate * estimate;
    estimate *= 1.5 - 0.5 * value * estimate * estimate;
    estimate
}

pub fn debug_identity_bgra(
    identity: IdentitySetId,
    proof: OutputProofCode,
    coverage: u8,
    q_width: u8,
) -> [u8; 4] {
    if proof.contains(OutputProofCode::BACKGROUND) {
        return [0, 0, 0, 255];
    }
    let scale = |channel: u8| -> u8 {
        u8::try_from((u16::from(channel) * u16::from(coverage) + 127) / 255).unwrap_or(255)
    };
    [
        scale(129_u8.saturating_add(q_width.min(126))),
        scale(identity.0 as u8),
        scale((identity.0 >> 8) as u8),
        255,
    ]
}

fn source_q_width_class(q: Iv32, error: Iv32) -> Result<u8, SweepError> {
    let lo = i64::from(q.lo)
        .checked_add(i64::from(error.lo))
        .ok_or(SweepError::NumericFailure)?;
    let hi = i64::from(q.hi)
        .checked_add(i64::from(error.hi))
        .ok_or(SweepError::NumericFailure)?;
    if hi < lo {
        return Err(SweepError::InternalInvariant);
    }
    Ok(u8::try_from((hi - lo).min(126)).expect("bounded q-width class fits u8"))
}

fn write_pixel(
    bytes: &mut [u8; TILE_BYTES],
    tile_x: u16,
    tile_y: u16,
    x: u16,
    y: u16,
    color: [u8; 4],
) -> Result<(), SweepError> {
    let local_x = usize::from(x.checked_sub(tile_x).ok_or(SweepError::InternalInvariant)?);
    let local_y = usize::from(y.checked_sub(tile_y).ok_or(SweepError::InternalInvariant)?);
    let offset = local_y
        .checked_mul(TILE_STRIDE)
        .and_then(|row| row.checked_add(local_x * 4))
        .filter(|offset| *offset + 4 <= bytes.len())
        .ok_or(SweepError::InternalInvariant)?;
    bytes[offset..offset + 4].copy_from_slice(&color);
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanoutTile {
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub bytes: [u8; TILE_BYTES],
}

impl ScanoutTile {
    pub fn new(id: u32, x: u16, y: u16, width: u16, height: u16) -> Result<Self, SweepError> {
        if width == 0
            || height == 0
            || usize::from(width) > TILE_WIDTH
            || usize::from(height) > TILE_HEIGHT
        {
            return Err(SweepError::InternalInvariant);
        }
        Ok(Self {
            id,
            x,
            y,
            width,
            height,
            bytes: [0; TILE_BYTES],
        })
    }

    pub fn padding_is_zero(&self) -> bool {
        (0..TILE_HEIGHT).all(|row| {
            (0..TILE_WIDTH).all(|column| {
                if row < usize::from(self.height) && column < usize::from(self.width) {
                    true
                } else {
                    let at = row * TILE_STRIDE + column * 4;
                    self.bytes[at..at + 4] == [0; 4]
                }
            })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationOwner {
    Available,
    Workers,
    Display,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TilePool {
    owners: [GenerationOwner; 2],
    front: Option<u8>,
    back: u8,
    next_sequence: u64,
}

impl Default for TilePool {
    fn default() -> Self {
        Self {
            owners: [GenerationOwner::Available, GenerationOwner::Workers],
            front: None,
            back: 1,
            next_sequence: 0,
        }
    }
}

impl TilePool {
    pub fn front(&self) -> Option<u8> {
        self.front
    }

    pub fn back(&self) -> u8 {
        self.back
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn fail_frame(&mut self) -> Result<(), SweepError> {
        if self.owners[usize::from(self.back)] != GenerationOwner::Workers {
            return Err(SweepError::InternalInvariant);
        }
        // Partial bytes are harmless: every visible byte is overwritten on
        // the next attempt and padding is immutable zero.
        Ok(())
    }

    pub fn present_succeeded(&mut self, generation: u8, sequence: u64) -> Result<(), SweepError> {
        if generation != self.back
            || sequence != self.next_sequence
            || self.owners[usize::from(generation)] != GenerationOwner::Workers
        {
            return Err(SweepError::InternalInvariant);
        }
        if let Some(front) = self.front {
            self.owners[usize::from(front)] = GenerationOwner::Workers;
            self.back = front;
        } else {
            self.back = 1 - generation;
            self.owners[usize::from(self.back)] = GenerationOwner::Workers;
        }
        self.owners[usize::from(generation)] = GenerationOwner::Display;
        self.front = Some(generation);
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(SweepError::CapacityExceeded)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::reference::sweep::{NormalModel, ProofMarginKind, QModel};

    fn certified(x0: u16, x1: u16, visible: bool) -> CertifiedRun {
        CertifiedRun {
            x0,
            x1,
            visible: visible.then_some(0),
            sheet_count: u16::from(visible),
            q_model: QModel {
                q0: Iv32::point(100),
                qx: Iv32::point(3),
                qxx: Iv32::point(1),
            },
            q_error: Iv32::point(0),
            q_u: Iv32::new(11, 15).unwrap(),
            q_v: Iv32::new(-9, -3).unwrap(),
            root_slack: Iv32::point(1),
            q_order_slack: Iv32::point(1),
            identity: IdentitySetId(7),
            normal_model: NormalModel {
                nx: Iv32::point(1000),
                ny: Iv32::point(-1000),
                nz: Iv32::point(1),
            },
            proof_owner: ProofMarginKind::Root,
            ..CertifiedRun::default()
        }
    }

    #[test]
    fn lowering_preserves_half_open_domain_and_discards_proof_arrays() {
        let lowered =
            lower_certified_run(certified(3, 19, true), FixedDomain::full(0), 2, 4).unwrap();
        assert_eq!((lowered.x0, lowered.x1), (3, 19));
        assert_eq!(lowered.material_summary, 2);
        assert_eq!(lowered.light_summary, 4);
        assert!(lowered.proof_code.contains(OutputProofCode::GEOMETRY));
        assert_eq!(lowered.q_u.value, 13);
        assert_eq!(lowered.q_u.error_radius, 2);
        assert_eq!(lowered.q_v.value, -6);
        assert_eq!(lowered.q_v.error_radius, 3);
    }

    #[test]
    fn scalar_and_packet_paths_match_with_prefix_suffix_and_reset() {
        for (x0, x1) in [(0, 1), (1, 3), (3, 7), (0, 16), (7, 15)] {
            let run =
                lower_certified_run(certified(x0, x1, true), FixedDomain::full(0), 0, 0).unwrap();
            let mut scalar = [0_u8; TILE_BYTES];
            let mut packet = [0_u8; TILE_BYTES];
            raster_run_scalar(&run, 0, 0, 0, 64, 1, &mut scalar).unwrap();
            raster_run4(&run, 0, 0, 0, 64, 1, &mut packet).unwrap();
            assert_eq!(packet, scalar, "domain [{x0},{x1})");
        }
    }

    #[test]
    fn scalar_and_packet_paths_encode_nonzero_q_width_in_blue() {
        let mut source = certified(1, 9, true);
        source.q_model.q0 = Iv32::new(100, 103).unwrap();
        let run = lower_certified_run(source, FixedDomain::full(0), 0, 0).unwrap();
        let mut scalar = [0_u8; TILE_BYTES];
        let mut packet = [0_u8; TILE_BYTES];
        raster_run_scalar(&run, 0, 0, 0, 64, 1, &mut scalar).unwrap();
        raster_run4(&run, 0, 0, 0, 64, 1, &mut packet).unwrap();
        assert_eq!(packet, scalar);
        assert_eq!(&scalar[4..8], &[132, 7, 0, 255]);
    }

    #[test]
    fn long_runs_split_into_independently_anchored_fixed_q_records() {
        let mut output =
            [lower_certified_run(certified(0, 1, true), FixedDomain::full(0), 0, 0).unwrap(); 4];
        let count = lower_certified_runs(
            certified(0, 64, true),
            FixedDomain::full(0),
            0,
            0,
            &mut output,
        )
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!((output[0].x0, output[0].x1), (0, 32));
        assert_eq!((output[1].x0, output[1].x1), (32, 64));
        assert_eq!(output[1].q.q, 100 + 32 * 3 + 32 * 31 / 2);
    }

    #[test]
    fn event_requires_a_singleton_encoded_byte_and_writes_alpha_255() {
        let mut bytes = [0_u8; TILE_BYTES];
        let event = EventPixel {
            x: 4,
            coverage: Iv32::point(128),
            front_run: 0,
            back_run: 1,
            first_event: 3,
            event_count: 2,
        };
        raster_event_pixel(
            event,
            &[1, 4, 9, 12, 20],
            0,
            0,
            0,
            64,
            1,
            [20, 40, 60, 255],
            [0, 0, 0, 255],
            &mut bytes,
        )
        .unwrap();
        assert_eq!(&bytes[16..20], &[10, 20, 30, 255]);
        let ambiguous = EventPixel {
            coverage: Iv32::new(127, 129).unwrap(),
            ..event
        };
        assert_eq!(
            raster_event_pixel(
                ambiguous,
                &[1, 4, 9, 12, 20],
                0,
                0,
                0,
                64,
                1,
                [20, 40, 60, 255],
                [0, 0, 0, 255],
                &mut bytes,
            ),
            Err(SweepError::CertificateExhausted)
        );

        let byte_207 = EventPixel {
            coverage: Iv32::point((207 * 256 + 127) / 255),
            ..event
        };
        raster_event_pixel(
            byte_207,
            &[1, 4, 9, 12, 20],
            0,
            0,
            0,
            64,
            1,
            [255, 0, 0, 255],
            [0, 0, 0, 255],
            &mut bytes,
        )
        .unwrap();
        assert_eq!(bytes[16], 207);
        assert_eq!(
            raster_event_pixel(
                EventPixel {
                    first_event: 4,
                    event_count: 2,
                    ..event
                },
                &[1, 4, 9, 12, 20],
                0,
                0,
                0,
                64,
                1,
                [255, 0, 0, 255],
                [0, 0, 0, 255],
                &mut bytes,
            ),
            Err(SweepError::InternalInvariant)
        );
    }

    #[test]
    fn high_contrast_diagonal_geometry_silhouette_matches_exact_host_area() {
        use crate::pixels::reference::coverage::{BoundaryOwner, HalfPlane, half_plane_byte};

        // A true foreground/background geometry edge, not a material seam.
        // The selected half-plane crosses both pixel axes at non-dyadic
        // positions, so this locks analytic area rather than point sampling.
        let coverage = half_plane_byte(
            HalfPlane {
                a: 32,
                b: -32,
                c: -11,
            },
            BoundaryOwner::LowerOrLeft,
        )
        .expect("exact diagonal area");
        assert_eq!(coverage, 55);
        let mut bytes = [0_u8; TILE_BYTES];
        raster_event_pixel(
            EventPixel {
                x: 7,
                coverage: Iv32::point(i32::from((u16::from(coverage) * 256 + 127) / 255)),
                front_run: 0,
                back_run: 1,
                first_event: 0,
                event_count: 1,
            },
            &[41],
            0,
            0,
            0,
            64,
            1,
            [255, 0, 0, 255],
            [0, 0, 0, 255],
            &mut bytes,
        )
        .expect("certified silhouette raster");
        assert_eq!(&bytes[28..32], &[coverage, 0, 0, 255]);
    }

    #[test]
    fn guest_coverage_codes_round_trip_every_byte_including_regressions() {
        for byte in 0_u16..=255 {
            let raw = i32::from((byte * 256 + 127) / 255);
            assert_eq!(
                singleton_coverage_byte(Iv32::point(raw)).unwrap(),
                byte as u8
            );
        }
        for byte in [207_u16, 225] {
            let raw = i32::from((byte * 256 + 127) / 255);
            assert_eq!(
                singleton_coverage_byte(Iv32::point(raw)).unwrap(),
                byte as u8
            );
        }
    }

    #[test]
    fn partial_tile_padding_stays_zero_and_addresses_are_bounded() {
        let mut tile = ScanoutTile::new(0, 64, 32, 3, 2).unwrap();
        let mut runs =
            [lower_certified_run(certified(64, 65, true), FixedDomain::full(0), 0, 0).unwrap(); 2];
        let count = lower_certified_runs(
            certified(64, 67, true),
            FixedDomain::full(0),
            0,
            0,
            &mut runs,
        )
        .unwrap();
        for run in &runs[..count] {
            raster_run4(run, 64, 32, 32, 3, 2, &mut tile.bytes).unwrap();
        }
        assert!(tile.padding_is_zero());
        assert_eq!(tile.bytes[3], 255);
        assert_eq!(tile.bytes[3 * 4..TILE_STRIDE], [0; TILE_STRIDE - 12]);
    }

    #[test]
    fn rsqrt_holds_its_exact_fixed_points_and_bounds_every_other_residual() {
        // These inputs are powers of four, so the sequence sits on its own
        // fixed point and must return the representable power of two exactly.
        // The guest asserts the identical pairs in `stdlib/tests/pixels_raster.wr`,
        // which is what makes this a correspondence oracle rather than a
        // second opinion.
        for (value, expected) in [
            (0.015_625_f32, 8.0_f32),
            (0.25, 2.0),
            (1.0, 1.0),
            (4.0, 0.5),
            (16.0, 0.25),
            (64.0, 0.125),
        ] {
            assert_eq!(
                raster_rsqrt(value).to_bits(),
                expected.to_bits(),
                "rsqrt({value}) must be bit-exact"
            );
        }
        for value in [0.1_f32, 0.5, 2.0, 3.0, 5.0, 7.0, 123.0, 1_000_000.0] {
            let estimate = raster_rsqrt(value);
            let residual = estimate * estimate * value - 1.0;
            assert!(
                residual.abs() < 1e-4,
                "rsqrt({value}) residual {residual} escapes the packet-normal budget"
            );
        }
        // Total at and below zero: a normal cone that cannot prove a strictly
        // positive squared norm must not reach an infinity through this path.
        for value in [0.0_f32, -0.0, -1.0, -1_000_000.0] {
            assert_eq!(raster_rsqrt(value), 0.0);
        }
        assert!(raster_rsqrt(f32::MAX).is_finite());
    }

    #[test]
    fn failed_frame_keeps_front_and_success_rotates_only_after_present() {
        let mut pool = TilePool::default();
        pool.present_succeeded(1, 0).unwrap();
        assert_eq!(pool.front(), Some(1));
        assert_eq!(pool.back(), 0);
        pool.fail_frame().unwrap();
        assert_eq!(pool.front(), Some(1));
        assert_eq!(pool.next_sequence(), 1);
        pool.present_succeeded(0, 1).unwrap();
        assert_eq!(pool.front(), Some(0));
        assert_eq!(pool.back(), 1);
    }
}
