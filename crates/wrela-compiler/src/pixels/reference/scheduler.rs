//! Output-code refinement scheduler with exact integer priorities.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ErrorSource {
    Visibility,
    Coverage,
    Normal,
    Material,
    Texture,
    DirectLight,
    Shadow,
    Ao,
    Gi,
    Transparency,
    Post,
    Temporal,
    Quantization,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefinementOption {
    pub source: ErrorSource,
    pub code_span_reduction: u64,
    pub operation_count: u64,
    pub remaining_depth: u16,
    pub interval_width: u64,
    pub payload_id: u32,
}

impl RefinementOption {
    pub fn validate(self) -> bool {
        self.operation_count > 0 && self.remaining_depth > 0 && self.interval_width > 0
    }
}

pub fn select(options: &[RefinementOption]) -> Result<Option<usize>, String> {
    if options.iter().any(|option| !option.validate()) {
        return Err("P031: invalid refinement option".to_string());
    }
    let mut best = None;
    for (index, candidate) in options.iter().enumerate() {
        let replace = best.is_none_or(|best_index| {
            let current: &RefinementOption = &options[best_index];
            let left =
                u128::from(candidate.code_span_reduction) * u128::from(current.operation_count);
            let right =
                u128::from(current.code_span_reduction) * u128::from(candidate.operation_count);
            left > right
                || (left == right
                    && (candidate.source, candidate.payload_id)
                        < (current.source, current.payload_id))
        });
        if replace {
            best = Some(index);
        }
    }
    Ok(best)
}

pub fn apply_measure(
    before: RefinementOption,
    after: Option<RefinementOption>,
) -> Result<(), String> {
    if !before.validate() {
        return Err("P031: invalid refinement measure".to_string());
    }
    if let Some(after) = after {
        if !after.validate()
            || (after.remaining_depth, after.interval_width)
                >= (before.remaining_depth, before.interval_width)
        {
            return Err(
                "P031: refinement did not strictly decrease its discrete measure".to_string(),
            );
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefinementResult {
    Singleton([u8; 3]),
    CertificateExhausted,
}

pub fn run(
    mut endpoint_codes: impl FnMut() -> ([u8; 3], [u8; 3]),
    mut options: impl FnMut() -> Vec<RefinementOption>,
    mut refine: impl FnMut(u32) -> Result<Option<RefinementOption>, String>,
    maximum_steps: usize,
) -> Result<RefinementResult, String> {
    for _ in 0..maximum_steps {
        let (lo, hi) = endpoint_codes();
        if lo == hi {
            return Ok(RefinementResult::Singleton(lo));
        }
        let available = options();
        let Some(index) = select(&available)? else {
            return Ok(RefinementResult::CertificateExhausted);
        };
        let chosen = available[index];
        let after = refine(chosen.payload_id)?;
        apply_measure(chosen, after)?;
    }
    let (lo, hi) = endpoint_codes();
    Ok(if lo == hi {
        RefinementResult::Singleton(lo)
    } else {
        RefinementResult::CertificateExhausted
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_uses_cross_multiplication_and_stable_ties() {
        let options = [
            RefinementOption {
                source: ErrorSource::Shadow,
                code_span_reduction: 4,
                operation_count: 2,
                remaining_depth: 2,
                interval_width: 4,
                payload_id: 9,
            },
            RefinementOption {
                source: ErrorSource::Material,
                code_span_reduction: 2,
                operation_count: 1,
                remaining_depth: 2,
                interval_width: 4,
                payload_id: 7,
            },
        ];
        assert_eq!(select(&options).unwrap(), Some(1));
    }

    #[test]
    fn every_error_source_participates_in_the_one_stable_order() {
        let sources = [
            ErrorSource::Visibility,
            ErrorSource::Coverage,
            ErrorSource::Normal,
            ErrorSource::Material,
            ErrorSource::Texture,
            ErrorSource::DirectLight,
            ErrorSource::Shadow,
            ErrorSource::Ao,
            ErrorSource::Gi,
            ErrorSource::Transparency,
            ErrorSource::Post,
            ErrorSource::Temporal,
            ErrorSource::Quantization,
        ];
        let options = sources.map(|source| RefinementOption {
            source,
            code_span_reduction: 1,
            operation_count: 1,
            remaining_depth: 1,
            interval_width: 1,
            payload_id: 0,
        });
        assert_eq!(select(&options).unwrap(), Some(0));
        for pair in sources.windows(2) {
            assert!(pair[0] < pair[1]);
        }
    }

    #[test]
    fn zero_guaranteed_reductions_remain_valid_and_use_stable_ties() {
        let options = [
            RefinementOption {
                source: ErrorSource::Texture,
                code_span_reduction: 0,
                operation_count: 1,
                remaining_depth: 1,
                interval_width: 3,
                payload_id: 9,
            },
            RefinementOption {
                source: ErrorSource::Material,
                code_span_reduction: 0,
                operation_count: 64,
                remaining_depth: 1,
                interval_width: 3,
                payload_id: 7,
            },
        ];
        assert_eq!(select(&options).unwrap(), Some(1));
    }

    #[test]
    fn unresolved_output_never_chooses_nearest_byte() {
        let result = run(|| ([1, 2, 3], [1, 2, 4]), Vec::new, |_| Ok(None), 4).unwrap();
        assert_eq!(result, RefinementResult::CertificateExhausted);
    }

    #[test]
    fn run_rejects_a_refinement_that_does_not_make_progress() {
        let option = RefinementOption {
            source: ErrorSource::Texture,
            code_span_reduction: 1,
            operation_count: 1,
            remaining_depth: 2,
            interval_width: 4,
            payload_id: 3,
        };
        assert_eq!(
            run(
                || ([0; 3], [1; 3]),
                || vec![option],
                |_| Ok(Some(option)),
                2,
            ),
            Err("P031: refinement did not strictly decrease its discrete measure".to_string())
        );
    }

    #[test]
    fn maximum_steps_is_an_exact_refinement_limit() {
        use std::cell::Cell;

        let option = RefinementOption {
            source: ErrorSource::Texture,
            code_span_reduction: 1,
            operation_count: 1,
            remaining_depth: 1,
            interval_width: 1,
            payload_id: 3,
        };
        let refinements = Cell::new(0);
        let result = run(
            || ([0; 3], [1; 3]),
            || vec![option],
            |_| {
                refinements.set(refinements.get() + 1);
                Ok(None)
            },
            0,
        )
        .unwrap();
        assert_eq!(result, RefinementResult::CertificateExhausted);
        assert_eq!(refinements.get(), 0);

        let resolved = Cell::new(false);
        let result = run(
            || {
                if resolved.get() {
                    ([7, 8, 9], [7, 8, 9])
                } else {
                    ([7, 8, 9], [7, 8, 10])
                }
            },
            || vec![option],
            |_| {
                resolved.set(true);
                Ok(None)
            },
            1,
        )
        .unwrap();
        assert_eq!(result, RefinementResult::Singleton([7, 8, 9]));
    }

    #[test]
    fn exact_small_fixture_matches_exhaustive_refinement() {
        use std::cell::Cell;

        fn endpoints(mask: u8) -> ([u8; 3], [u8; 3]) {
            let lo = [17, 23, 31];
            let span = u8::try_from(mask.count_ones()).unwrap();
            (lo, [lo[0], lo[1], lo[2] + span])
        }

        fn exhaust(mask: u8, results: &mut Vec<[u8; 3]>) {
            let (lo, hi) = endpoints(mask);
            if lo == hi {
                results.push(lo);
                return;
            }
            for payload_id in 0..2 {
                let bit = 1 << payload_id;
                if mask & bit != 0 {
                    exhaust(mask & !bit, results);
                }
            }
        }

        let mask = Cell::new(0b11_u8);
        let result = run(
            || endpoints(mask.get()),
            || {
                (0..2)
                    .filter(|payload_id| mask.get() & (1 << payload_id) != 0)
                    .map(|payload_id| RefinementOption {
                        source: ErrorSource::Material,
                        code_span_reduction: if payload_id == 0 { 1 } else { 4 },
                        operation_count: if payload_id == 0 { 1 } else { 2 },
                        remaining_depth: 1,
                        interval_width: 1,
                        payload_id,
                    })
                    .collect()
            },
            |payload_id| {
                mask.set(mask.get() & !(1 << payload_id));
                Ok(None)
            },
            2,
        )
        .unwrap();

        let mut exhaustive_results = Vec::new();
        exhaust(0b11, &mut exhaustive_results);
        assert!(!exhaustive_results.is_empty());
        assert!(
            exhaustive_results
                .iter()
                .all(|bytes| *bytes == [17, 23, 31])
        );
        assert_eq!(result, RefinementResult::Singleton(exhaustive_results[0]));
    }
}
