//! Generic compressed-slack predicate used by later kinetic maintenance.

use super::iv32::NumericError;

pub fn slack_preserves(
    minimum_slack_raw: i32,
    perturbation_raw: i32,
) -> Result<bool, NumericError> {
    if minimum_slack_raw < 0 || perturbation_raw < 0 {
        return Err(NumericError::InvalidDomain);
    }
    Ok(perturbation_raw < minimum_slack_raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_does_not_preserve_strict_slack() {
        assert_eq!(slack_preserves(4, 3), Ok(true));
        assert_eq!(slack_preserves(4, 4), Ok(false));
        assert_eq!(slack_preserves(4, -1), Err(NumericError::InvalidDomain));
    }
}
