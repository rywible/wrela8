//! Certified deterministic rectangle/disk source integration.

use super::interval::F64Interval;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmitterShape {
    Rectangle,
    Disk,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceCell {
    pub s0: f64,
    pub s1: f64,
    pub t0: f64,
    pub t1: f64,
    pub depth: u8,
    pub morton: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellBound {
    /// Integral contribution bounds over the entire cell, already including
    /// source-domain area and the disk map's Jacobian when applicable.
    pub contribution: [F64Interval; 3],
    pub candidate: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaResult {
    pub candidate: [f64; 3],
    pub bounds: [F64Interval; 3],
    pub accepted_cells: u32,
    pub deepest_cell: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaError {
    InvalidInput,
    CertificateExhausted,
    CapacityExceeded,
}

fn uncertainty(bound: [F64Interval; 3]) -> f64 {
    bound
        .into_iter()
        .map(|channel| channel.hi - channel.lo)
        .fold(0.0, f64::max)
}

fn children(cell: SourceCell) -> [SourceCell; 4] {
    let sm = (cell.s0 + cell.s1) * 0.5;
    let tm = (cell.t0 + cell.t1) * 0.5;
    [
        SourceCell {
            s0: cell.s0,
            s1: sm,
            t0: cell.t0,
            t1: tm,
            depth: cell.depth + 1,
            morton: cell.morton << 2,
        },
        SourceCell {
            s0: sm,
            s1: cell.s1,
            t0: cell.t0,
            t1: tm,
            depth: cell.depth + 1,
            morton: (cell.morton << 2) | 1,
        },
        SourceCell {
            s0: cell.s0,
            s1: sm,
            t0: tm,
            t1: cell.t1,
            depth: cell.depth + 1,
            morton: (cell.morton << 2) | 2,
        },
        SourceCell {
            s0: sm,
            s1: cell.s1,
            t0: tm,
            t1: cell.t1,
            depth: cell.depth + 1,
            morton: (cell.morton << 2) | 3,
        },
    ]
}

/// Cells are consumed in depth-first Morton order. Candidate center queries
/// may propose a bound, but `bound_cell` must certify the complete cell.
pub fn integrate(
    shape: EmitterShape,
    channel_budget: f64,
    maximum_depth: u8,
    capacity: usize,
    mut bound_cell: impl FnMut(EmitterShape, SourceCell) -> Result<CellBound, AreaError>,
) -> Result<AreaResult, AreaError> {
    if !channel_budget.is_finite() || channel_budget < 0.0 || maximum_depth > 30 || capacity == 0 {
        return Err(AreaError::InvalidInput);
    }
    let root = SourceCell {
        s0: -1.0,
        s1: 1.0,
        t0: -1.0,
        t1: 1.0,
        depth: 0,
        morton: 0,
    };
    let mut stack = Vec::with_capacity(capacity.min(4096));
    stack.push(root);
    let mut candidate = [0.0; 3];
    let mut bounds = [F64Interval::point(0.0).map_err(|_| AreaError::InvalidInput)?; 3];
    let mut visited_cells = 0_usize;
    let mut accepted_cells = 0_u32;
    let mut deepest_cell = 0_u8;
    while let Some(cell) = stack.pop() {
        visited_cells = visited_cells
            .checked_add(1)
            .ok_or(AreaError::CapacityExceeded)?;
        if visited_cells > capacity {
            return Err(AreaError::CapacityExceeded);
        }
        let bound = bound_cell(shape, cell)?;
        if bound
            .contribution
            .into_iter()
            .any(|channel| channel.lo < 0.0 || !channel.lo.is_finite() || !channel.hi.is_finite())
            || bound.candidate.into_iter().any(|value| !value.is_finite())
            || (0..3).any(|channel| !bound.contribution[channel].contains(bound.candidate[channel]))
        {
            return Err(AreaError::InvalidInput);
        }
        // At depth d, a rectifiable blocker boundary can cross at most 2^d
        // cells. Reserving one 2^-maximum_depth share for every uncertain
        // terminal therefore lets subdivision reduce total uncertainty. A
        // cell-area-proportional share would never converge at an edge: both
        // its uncertainty and allowance shrink by four on every split.
        let cell_budget = channel_budget * 0.5_f64.powi(i32::from(maximum_depth));
        if uncertainty(bound.contribution) <= cell_budget {
            for channel in 0..3 {
                candidate[channel] += bound.candidate[channel];
                bounds[channel] = bounds[channel]
                    .add_outward(bound.contribution[channel])
                    .map_err(|_| AreaError::InvalidInput)?;
            }
            accepted_cells = accepted_cells
                .checked_add(1)
                .ok_or(AreaError::CapacityExceeded)?;
            if accepted_cells as usize > capacity {
                return Err(AreaError::CapacityExceeded);
            }
            deepest_cell = deepest_cell.max(cell.depth);
            continue;
        }
        if cell.depth >= maximum_depth {
            return Err(AreaError::CertificateExhausted);
        }
        if visited_cells
            .checked_add(stack.len())
            .and_then(|scheduled| scheduled.checked_add(4))
            .is_none_or(|scheduled| scheduled > capacity)
        {
            return Err(AreaError::CapacityExceeded);
        }
        let children = children(cell);
        // Reverse push causes child 0..3 Morton consumption.
        stack.extend(children.into_iter().rev());
    }
    if uncertainty(bounds) > channel_budget + 32.0 * f64::EPSILON {
        return Err(AreaError::CertificateExhausted);
    }
    Ok(AreaResult {
        candidate,
        bounds,
        accepted_cells,
        deepest_cell,
    })
}

/// Concentric square-to-disk map. The returned Jacobian is explicit so the
/// cell bound cannot silently treat a square integral as a disk integral.
pub fn concentric_disk(s: f64, t: f64) -> Result<([f64; 2], f64), AreaError> {
    if !s.is_finite() || !t.is_finite() || s.abs() > 1.0 || t.abs() > 1.0 {
        return Err(AreaError::InvalidInput);
    }
    if s == 0.0 && t == 0.0 {
        return Ok(([0.0, 0.0], std::f64::consts::PI / 4.0));
    }
    let (radius, angle) = if s.abs() > t.abs() {
        (s, std::f64::consts::FRAC_PI_4 * (t / s))
    } else {
        (
            t,
            std::f64::consts::FRAC_PI_2 - std::f64::consts::FRAC_PI_4 * (s / t),
        )
    };
    Ok((
        [radius * angle.cos(), radius * angle.sin()],
        std::f64::consts::PI / 4.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morton_subdivision_is_stable_and_exhaustion_is_explicit() {
        let mut seen = Vec::new();
        let result = integrate(EmitterShape::Rectangle, 0.01, 1, 8, |_, cell| {
            seen.push((cell.depth, cell.morton));
            let width = if cell.depth == 0 { 1.0 } else { 0.0 };
            Ok(CellBound {
                contribution: [F64Interval::new(0.0, width).unwrap(); 3],
                candidate: [0.0; 3],
            })
        })
        .unwrap();
        assert_eq!(seen, vec![(0, 0), (1, 0), (1, 1), (1, 2), (1, 3)]);
        assert_eq!(result.accepted_cells, 4);
        assert_eq!(
            integrate(EmitterShape::Rectangle, 0.0, 0, 1, |_, _| Ok(CellBound {
                contribution: [F64Interval::new(0.0, 1.0).unwrap(); 3],
                candidate: [0.5; 3]
            })),
            Err(AreaError::CertificateExhausted)
        );
    }

    #[test]
    fn capacity_counts_internal_and_terminal_cells() {
        let bound = |_: EmitterShape, cell: SourceCell| {
            Ok(CellBound {
                contribution: [F64Interval::new(0.0, if cell.depth == 0 { 1.0 } else { 0.0 })
                    .unwrap(); 3],
                candidate: [0.0; 3],
            })
        };
        assert_eq!(
            integrate(EmitterShape::Rectangle, 0.0, 1, 4, bound),
            Err(AreaError::CapacityExceeded),
        );
        assert_eq!(
            integrate(EmitterShape::Rectangle, 0.0, 1, 5, bound)
                .unwrap()
                .accepted_cells,
            4,
        );
    }

    #[test]
    fn disk_map_has_explicit_constant_jacobian() {
        let (_, jacobian) = concentric_disk(0.5, -0.25).unwrap();
        assert_eq!(jacobian, std::f64::consts::PI / 4.0);
    }

    #[test]
    fn accepted_leaf_uncertainties_sum_to_root_budget() {
        let result = integrate(EmitterShape::Rectangle, 1.0, 1, 8, |_, cell| {
            let width = if cell.depth == 0 { 2.0 } else { 0.125 };
            Ok(CellBound {
                contribution: [F64Interval::new(0.0, width).unwrap(); 3],
                candidate: [width * 0.5; 3],
            })
        })
        .unwrap();
        assert_eq!(result.accepted_cells, 4);
        assert!(uncertainty(result.bounds) <= 1.0 + 16.0 * f64::EPSILON);
    }

    #[test]
    fn a_center_proposal_outside_its_complete_cell_bound_fails_closed() {
        assert_eq!(
            integrate(EmitterShape::Disk, 1.0, 1, 8, |_, _| Ok(CellBound {
                contribution: [F64Interval::new(0.0, 0.25).unwrap(); 3],
                candidate: [0.5; 3],
            })),
            Err(AreaError::InvalidInput)
        );
    }
}
