//! Verified shading summaries and packet-equivalent candidate evaluation.

use super::interval::F64Interval;

pub const CROSS_GRID_V1: [f64; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryBasis {
    Constant,
    AffineX,
    QuadraticX,
    SeparableRank(u8),
    ExactPixel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShadingSummary {
    pub basis: SummaryBasis,
    pub coefficients: Vec<[f64; 3]>,
    pub residual: [F64Interval; 3],
    pub anchors: Vec<(u8, u8, [f64; 3])>,
}

impl ShadingSummary {
    pub fn validate(&self) -> Result<(), String> {
        let coefficient_count_valid = match self.basis {
            SummaryBasis::Constant => self.coefficients.len() == 1,
            SummaryBasis::AffineX => self.coefficients.len() == 2,
            SummaryBasis::QuadraticX => self.coefficients.len() == 3,
            SummaryBasis::SeparableRank(rank) => {
                (1..=4).contains(&rank) && self.coefficients.len() == usize::from(rank) * 3
            }
            SummaryBasis::ExactPixel => self.coefficients.is_empty(),
        };
        if self
            .coefficients
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
            || self.residual.iter().any(|interval| {
                interval.lo > interval.hi || !interval.lo.is_finite() || !interval.hi.is_finite()
            })
            || matches!(self.basis, SummaryBasis::SeparableRank(rank) if rank == 0 || rank > 4)
            || self.anchors.len() > 4
            || self.anchors.iter().any(|(x, y, value)| {
                *x >= 5 || *y >= 5 || value.iter().any(|channel| !channel.is_finite())
            })
            || self.anchors.iter().enumerate().any(|(index, (x, y, _))| {
                self.anchors[..index]
                    .iter()
                    .any(|(prior_x, prior_y, _)| prior_x == x && prior_y == y)
            })
            || !coefficient_count_valid
        {
            return Err("P024/P030: invalid shading summary certificate".to_string());
        }
        Ok(())
    }

    pub fn interval_at(&self, x: f64, y: f64) -> Result<([f64; 3], [F64Interval; 3]), String> {
        self.validate()?;
        if !x.is_finite() || !y.is_finite() || x.abs() > 1.0 || y.abs() > 1.0 {
            return Err("P030: shading coordinate outside certified tile".to_string());
        }
        let candidate = match self.basis {
            SummaryBasis::Constant => *self
                .coefficients
                .first()
                .ok_or_else(|| "P030: missing constant coefficient".to_string())?,
            SummaryBasis::AffineX => {
                if self.coefficients.len() != 2 {
                    return Err("P030: affine coefficient count".to_string());
                }
                std::array::from_fn(|channel| {
                    self.coefficients[0][channel] + self.coefficients[1][channel] * x
                })
            }
            SummaryBasis::QuadraticX => {
                if self.coefficients.len() != 3 {
                    return Err("P030: quadratic coefficient count".to_string());
                }
                std::array::from_fn(|channel| {
                    self.coefficients[0][channel]
                        + self.coefficients[1][channel] * x
                        + self.coefficients[2][channel] * x * x
                })
            }
            SummaryBasis::SeparableRank(rank) => {
                if self.coefficients.len() != usize::from(rank) * 3 {
                    return Err("P030: rank coefficient count".to_string());
                }
                let mut value = [0.0; 3];
                for term in 0..usize::from(rank) {
                    let u = self.coefficients[term * 3];
                    let v = self.coefficients[term * 3 + 1];
                    let scale = self.coefficients[term * 3 + 2];
                    for channel in 0..3 {
                        value[channel] += (u[channel] + x) * (v[channel] + y) * scale[channel];
                    }
                }
                value
            }
            SummaryBasis::ExactPixel => {
                return Err(
                    "P030: exact-pixel summary requires scalar material evaluation".to_string(),
                );
            }
        };
        let mut bounds = [F64Interval::point(0.0)?; 3];
        for channel in 0..3 {
            if !candidate[channel].is_finite() {
                return Err("P030: shading summary candidate is non-finite".to_string());
            }
            bounds[channel] = F64Interval::new(
                candidate[channel] + self.residual[channel].lo,
                candidate[channel] + self.residual[channel].hi,
            )
            .map_err(|_| "P030: shading summary residual application overflowed".to_string())?;
        }
        Ok((candidate, bounds))
    }
}

/// Greatest residual upper bound; ties are y then x as required by P9.4.
pub fn select_cross_pivot(
    residual_upper: [[f64; 5]; 5],
    used: &[(u8, u8)],
) -> Result<Option<(u8, u8)>, String> {
    if residual_upper
        .iter()
        .flatten()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err("P024: invalid rank residual grid".to_string());
    }
    let mut seen = std::collections::BTreeSet::new();
    if used
        .iter()
        .any(|&(x, y)| x >= 5 || y >= 5 || !seen.insert((x, y)))
    {
        return Err("P024: invalid used rank pivot set".to_string());
    }
    if used.len() >= 4 {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    for y in 0..5_u8 {
        for x in 0..5_u8 {
            if !used.contains(&(x, y)) {
                candidates.push((residual_upper[y as usize][x as usize], y, x));
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    Ok(candidates.first().map(|(_, y, x)| (*x, *y)))
}

pub fn packet_candidates(
    summary: &ShadingSummary,
    coordinates: &[(f64, f64); 4],
) -> Result<[[f64; 3]; 4], String> {
    let mut output = [[0.0; 3]; 4];
    for lane in 0..4 {
        output[lane] = summary
            .interval_at(coordinates[lane].0, coordinates[lane].1)?
            .0;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pivot_ties_use_y_then_x_and_rank_never_exceeds_four() {
        let grid = [[1.0; 5]; 5];
        assert_eq!(select_cross_pivot(grid, &[]).unwrap(), Some((0, 0)));
        assert_eq!(select_cross_pivot(grid, &[(0, 0)]).unwrap(), Some((1, 0)));
        assert_eq!(
            select_cross_pivot(grid, &[(0, 0), (1, 0), (2, 0), (3, 0)]).unwrap(),
            None,
        );
        assert!(select_cross_pivot(grid, &[(5, 0)]).is_err());
        assert!(
            ShadingSummary {
                basis: SummaryBasis::SeparableRank(5),
                coefficients: vec![],
                residual: [F64Interval::point(0.0).unwrap(); 3],
                anchors: vec![]
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn packet_and_scalar_candidates_are_identical() {
        let summary = ShadingSummary {
            basis: SummaryBasis::QuadraticX,
            coefficients: vec![[0.2, 0.3, 0.4], [0.1; 3], [0.05; 3]],
            residual: [F64Interval::new(-0.001, 0.001).unwrap(); 3],
            anchors: vec![],
        };
        let coordinates = [(-0.75, 0.0), (-0.25, 0.0), (0.25, 0.0), (0.75, 0.0)];
        let packet = packet_candidates(&summary, &coordinates).unwrap();
        for lane in 0..4 {
            assert_eq!(
                packet[lane],
                summary
                    .interval_at(coordinates[lane].0, coordinates[lane].1)
                    .unwrap()
                    .0
            );
        }
    }

    #[test]
    fn fixed_ladder_rungs_evaluate_with_explicit_residuals() {
        let zero = [F64Interval::point(0.0).unwrap(); 3];
        let cases = [
            ShadingSummary {
                basis: SummaryBasis::Constant,
                coefficients: vec![[0.25, 0.5, 0.75]],
                residual: zero,
                anchors: vec![(2, 2, [0.25, 0.5, 0.75])],
            },
            ShadingSummary {
                basis: SummaryBasis::AffineX,
                coefficients: vec![[0.25; 3], [0.125; 3]],
                residual: [F64Interval::new(-0.01, 0.01).unwrap(); 3],
                anchors: vec![(0, 2, [0.125; 3]), (4, 2, [0.375; 3])],
            },
            ShadingSummary {
                basis: SummaryBasis::QuadraticX,
                coefficients: vec![[0.25; 3], [0.125; 3], [0.0625; 3]],
                residual: [F64Interval::new(-0.02, 0.02).unwrap(); 3],
                anchors: vec![(0, 2, [0.1875; 3]), (2, 2, [0.25; 3]), (4, 2, [0.4375; 3])],
            },
            ShadingSummary {
                basis: SummaryBasis::SeparableRank(1),
                coefficients: vec![[0.5; 3], [0.25; 3], [0.125; 3]],
                residual: [F64Interval::new(-0.03, 0.03).unwrap(); 3],
                anchors: vec![(2, 2, [0.015625; 3])],
            },
        ];
        for summary in cases {
            let (candidate, bounds) = summary.interval_at(0.25, -0.5).unwrap();
            for channel in 0..3 {
                assert!(bounds[channel].contains(candidate[channel]));
            }
        }
        let exact = ShadingSummary {
            basis: SummaryBasis::ExactPixel,
            coefficients: vec![],
            residual: zero,
            anchors: vec![],
        };
        assert!(exact.validate().is_ok());
        assert!(exact.interval_at(0.0, 0.0).unwrap_err().contains("scalar"));
    }

    #[test]
    fn malformed_proposer_coefficients_fail_closed_before_evaluation() {
        let malformed = ShadingSummary {
            basis: SummaryBasis::AffineX,
            coefficients: vec![[0.0; 3]],
            residual: [F64Interval::point(0.0).unwrap(); 3],
            anchors: vec![],
        };
        assert!(malformed.validate().is_err());

        let overflowing = ShadingSummary {
            basis: SummaryBasis::SeparableRank(1),
            coefficients: vec![[f64::MAX; 3], [f64::MAX; 3], [f64::MAX; 3]],
            residual: [F64Interval::point(0.0).unwrap(); 3],
            anchors: vec![],
        };
        assert!(overflowing.interval_at(1.0, 1.0).is_err());
    }

    #[test]
    fn summary_residual_contains_an_independent_dense_scalar_reference() {
        let summary = ShadingSummary {
            basis: SummaryBasis::AffineX,
            coefficients: vec![[0.25, 0.5, 0.75], [0.1, -0.05, 0.025]],
            residual: [F64Interval::new(0.0, 0.02).unwrap(); 3],
            anchors: vec![(0, 0, [0.17, 0.57, 0.745]), (4, 4, [0.37, 0.47, 0.795])],
        };
        for y_step in 0..=32 {
            let y = -1.0 + 2.0 * y_step as f64 / 32.0;
            for x_step in 0..=32 {
                let x = -1.0 + 2.0 * x_step as f64 / 32.0;
                let (_, bounds) = summary.interval_at(x, y).unwrap();
                for channel in 0..3 {
                    let exact = summary.coefficients[0][channel]
                        + summary.coefficients[1][channel] * x
                        + 0.02 * y * y;
                    assert!(bounds[channel].contains(exact));
                }
            }
        }
    }
}
