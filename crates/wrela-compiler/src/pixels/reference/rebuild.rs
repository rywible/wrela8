//! Fixed-order bounded local rebuild ladder.

use super::telemetry::{CertificateTelemetry, RebuildReason, SUBDIVISION_BINS};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RebuildCell {
    pub x0: u16,
    pub x1: u16,
    pub q_lo: i32,
    pub q_hi: i32,
    pub x_depth: u8,
    pub q_depth: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RebuildLimits {
    pub max_x_depth: u8,
    pub max_q_depth: u8,
    pub max_cells: u16,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebuildTier {
    QSplit = 0,
    FeatureSplit = 1,
    BranchSplit = 2,
    EventArrangement = 3,
    PixelCell = 4,
    SubpixelIntegration = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierResult<T> {
    Resolved(T),
    Inconclusive,
    NumericFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebuildError {
    InvalidDomain,
    CapacityExceeded,
    CertificateExhausted,
    NumericFailure,
}

pub fn resolve_bounded<T: Copy + PartialEq>(
    initial: RebuildCell,
    limits: RebuildLimits,
    stack: &mut [RebuildCell],
    output: &mut [T],
    mut attempt: impl FnMut(RebuildTier, RebuildCell) -> TierResult<T>,
    mut telemetry: Option<&mut CertificateTelemetry>,
) -> Result<usize, RebuildError> {
    if initial.x0 >= initial.x1 || limits.max_cells == 0 || stack.is_empty() {
        return Err(RebuildError::InvalidDomain);
    }
    let mut stack_len = 1_usize;
    stack[0] = initial;
    let mut output_len = 0_usize;
    let mut entered = 0_u16;
    while stack_len != 0 {
        stack_len -= 1;
        let cell = stack[stack_len];
        entered = entered
            .checked_add(1)
            .ok_or(RebuildError::CapacityExceeded)?;
        if entered > limits.max_cells {
            charge_terminal(&mut telemetry, RebuildReason::Exhausted);
            return Err(RebuildError::CertificateExhausted);
        }
        if cell.x1 - cell.x0 > 1 && cell.x_depth < limits.max_x_depth {
            charge_entry(&mut telemetry, RebuildReason::XSplit);
            let midpoint = cell.x0 + (cell.x1 - cell.x0) / 2;
            if midpoint <= cell.x0 || midpoint >= cell.x1 || stack_len + 2 > stack.len() {
                charge_terminal(&mut telemetry, RebuildReason::Exhausted);
                return Err(RebuildError::CapacityExceeded);
            }
            stack[stack_len] = RebuildCell {
                x0: midpoint,
                x1: cell.x1,
                q_lo: cell.q_lo,
                q_hi: cell.q_hi,
                x_depth: cell.x_depth + 1,
                q_depth: cell.q_depth,
            };
            stack[stack_len + 1] = RebuildCell {
                x0: cell.x0,
                x1: midpoint,
                q_lo: cell.q_lo,
                q_hi: cell.q_hi,
                x_depth: cell.x_depth + 1,
                q_depth: cell.q_depth,
            };
            stack_len += 2;
            continue;
        }

        let mut resolved = None;
        let mut q_split = false;
        for tier in [
            RebuildTier::QSplit,
            RebuildTier::FeatureSplit,
            RebuildTier::BranchSplit,
            RebuildTier::EventArrangement,
            RebuildTier::PixelCell,
            RebuildTier::SubpixelIntegration,
        ] {
            let reason = tier_reason(tier);
            charge_entry(&mut telemetry, reason);
            match attempt(tier, cell) {
                TierResult::Resolved(value) => {
                    resolved = Some((value, reason));
                    break;
                }
                TierResult::Inconclusive => {
                    if tier == RebuildTier::QSplit
                        && cell.q_depth < limits.max_q_depth
                        && cell.q_lo < cell.q_hi
                    {
                        let midpoint = i64::from(cell.q_lo)
                            + (i64::from(cell.q_hi) - i64::from(cell.q_lo)) / 2;
                        let midpoint =
                            i32::try_from(midpoint).map_err(|_| RebuildError::NumericFailure)?;
                        if midpoint > cell.q_lo
                            && midpoint < cell.q_hi
                            && stack_len + 2 <= stack.len()
                        {
                            // The stack is LIFO. Store the rear (smaller-q)
                            // half first so the front (larger-q) half is
                            // resolved first, matching the sealed sweep order.
                            stack[stack_len] = RebuildCell {
                                q_lo: cell.q_lo,
                                q_hi: midpoint,
                                q_depth: cell.q_depth + 1,
                                ..cell
                            };
                            stack[stack_len + 1] = RebuildCell {
                                q_lo: midpoint,
                                q_hi: cell.q_hi,
                                q_depth: cell.q_depth + 1,
                                ..cell
                            };
                            stack_len += 2;
                            q_split = true;
                            break;
                        }
                    }
                }
                TierResult::NumericFailure => {
                    charge_terminal(&mut telemetry, RebuildReason::Exhausted);
                    return Err(RebuildError::NumericFailure);
                }
            }
        }
        if q_split {
            continue;
        }
        let Some((value, reason)) = resolved else {
            charge_terminal(&mut telemetry, RebuildReason::Exhausted);
            return Err(RebuildError::CertificateExhausted);
        };
        let Some(slot) = output.get_mut(output_len) else {
            charge_terminal(&mut telemetry, RebuildReason::Exhausted);
            return Err(RebuildError::CapacityExceeded);
        };
        *slot = value;
        output_len += 1;
        charge_terminal(&mut telemetry, reason);
        if let Some(telemetry) = telemetry.as_deref_mut() {
            telemetry.root_subdivision_depth
                [usize::from(cell.q_depth).min(SUBDIVISION_BINS - 1)] += 1;
            telemetry.event_subdivision_depth
                [usize::from(cell.x_depth).min(SUBDIVISION_BINS - 1)] += 1;
        }
    }
    Ok(output_len)
}

fn tier_reason(tier: RebuildTier) -> RebuildReason {
    match tier {
        RebuildTier::QSplit => RebuildReason::QSplit,
        RebuildTier::FeatureSplit => RebuildReason::FeatureSplit,
        RebuildTier::BranchSplit => RebuildReason::BranchSplit,
        RebuildTier::EventArrangement => RebuildReason::EventArrangement,
        RebuildTier::PixelCell => RebuildReason::PixelCell,
        RebuildTier::SubpixelIntegration => RebuildReason::SubpixelIntegration,
    }
}

fn charge_entry(telemetry: &mut Option<&mut CertificateTelemetry>, reason: RebuildReason) {
    if let Some(telemetry) = telemetry.as_deref_mut() {
        telemetry.charge_rebuild_entry(reason);
    }
}

fn charge_terminal(telemetry: &mut Option<&mut CertificateTelemetry>, reason: RebuildReason) {
    if let Some(telemetry) = telemetry.as_deref_mut() {
        telemetry.charge_rebuild_terminal(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_is_fixed_bounded_and_each_leaf_has_one_terminal() {
        let mut stack = [RebuildCell::default(); 8];
        let mut output = [0_u16; 8];
        let mut telemetry = CertificateTelemetry::default();
        let count = resolve_bounded(
            RebuildCell {
                x0: 0,
                x1: 4,
                q_lo: 0,
                q_hi: 0,
                x_depth: 0,
                q_depth: 0,
            },
            RebuildLimits {
                max_x_depth: 2,
                max_q_depth: 3,
                max_cells: 7,
            },
            &mut stack,
            &mut output,
            |tier, cell| {
                if tier == RebuildTier::PixelCell {
                    TierResult::Resolved(cell.x0)
                } else {
                    TierResult::Inconclusive
                }
            },
            Some(&mut telemetry),
        )
        .unwrap();
        assert_eq!(count, 4);
        assert_eq!(&output[..count], &[0, 1, 2, 3]);
        assert_eq!(
            telemetry.rebuild_terminals.iter().sum::<u64>(),
            u64::try_from(count).unwrap()
        );
    }

    #[test]
    fn unresolved_pixel_fails_explicitly() {
        let mut stack = [RebuildCell::default(); 2];
        let mut output = [0_u8; 2];
        assert_eq!(
            resolve_bounded(
                RebuildCell {
                    x0: 0,
                    x1: 1,
                    q_lo: 0,
                    q_hi: 0,
                    x_depth: 0,
                    q_depth: 0,
                },
                RebuildLimits {
                    max_x_depth: 0,
                    max_q_depth: 0,
                    max_cells: 1,
                },
                &mut stack,
                &mut output,
                |_, _| TierResult::Inconclusive,
                None,
            ),
            Err(RebuildError::CertificateExhausted)
        );
    }

    #[test]
    fn q_split_returns_all_certified_subdomains_front_to_back() {
        let mut stack = [RebuildCell::default(); 8];
        let mut output = [0_i32; 8];
        let count = resolve_bounded(
            RebuildCell {
                x0: 0,
                x1: 1,
                q_lo: 0,
                q_hi: 8,
                x_depth: 0,
                q_depth: 0,
            },
            RebuildLimits {
                max_x_depth: 0,
                max_q_depth: 2,
                max_cells: 7,
            },
            &mut stack,
            &mut output,
            |tier, cell| {
                if tier == RebuildTier::QSplit && cell.q_depth < 2 {
                    TierResult::Inconclusive
                } else if tier == RebuildTier::PixelCell {
                    TierResult::Resolved(cell.q_lo)
                } else {
                    TierResult::Inconclusive
                }
            },
            None,
        )
        .unwrap();
        assert_eq!(count, 4);
        assert_eq!(&output[..count], &[6, 4, 2, 0]);
    }
}
