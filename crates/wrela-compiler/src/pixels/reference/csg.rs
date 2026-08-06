//! Fixed-stack CSG occupancy and boundary influence kernels.

use super::iv32::NumericError;

pub const MAX_CSG_STACK: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsgInstruction {
    Object(u8),
    Union,
    Intersection,
    Subtract,
    Negate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Enter,
    Exit,
}

pub fn evaluate(program: &[CsgInstruction], inside_bits: u64) -> Result<bool, NumericError> {
    let mut stack = [false; MAX_CSG_STACK];
    let mut len = 0_usize;
    for instruction in program {
        match *instruction {
            CsgInstruction::Object(index) => {
                if len == MAX_CSG_STACK || index >= 64 {
                    return Err(NumericError::CapacityExceeded);
                }
                stack[len] = inside_bits & (1_u64 << index) != 0;
                len += 1;
            }
            CsgInstruction::Negate => {
                let value = stack
                    .get_mut(len.checked_sub(1).ok_or(NumericError::UnsupportedShape)?)
                    .ok_or(NumericError::UnsupportedShape)?;
                *value = !*value;
            }
            operation => {
                if len < 2 {
                    return Err(NumericError::UnsupportedShape);
                }
                let right = stack[len - 1];
                let left = stack[len - 2];
                len -= 1;
                stack[len - 1] = match operation {
                    CsgInstruction::Union => left || right,
                    CsgInstruction::Intersection => left && right,
                    CsgInstruction::Subtract => left && !right,
                    CsgInstruction::Object(_) | CsgInstruction::Negate => unreachable!(),
                };
            }
        }
    }
    if len != 1 {
        return Err(NumericError::UnsupportedShape);
    }
    Ok(stack[0])
}

pub fn oriented_toggle(
    inside_bits: u64,
    object: u8,
    orientation: Orientation,
) -> Result<u64, NumericError> {
    if object >= 64 {
        return Err(NumericError::CapacityExceeded);
    }
    let mask = 1_u64 << object;
    let is_inside = inside_bits & mask != 0;
    if matches!(
        (is_inside, orientation),
        (false, Orientation::Exit) | (true, Orientation::Enter)
    ) {
        return Err(NumericError::UnsupportedShape);
    }
    Ok(inside_bits ^ mask)
}

pub fn boundary_influences(
    program: &[CsgInstruction],
    inside_bits: u64,
    object: u8,
) -> Result<bool, NumericError> {
    if object >= 64 {
        return Err(NumericError::CapacityExceeded);
    }
    let mask = 1_u64 << object;
    Ok(evaluate(program, inside_bits & !mask)? != evaluate(program, inside_bits | mask)?)
}

pub fn first_transition(
    program: &[CsgInstruction],
    mut inside_bits: u64,
    crossings: &[(u8, Orientation)],
) -> Result<Option<usize>, NumericError> {
    let mut occupied = evaluate(program, inside_bits)?;
    for (index, (object, orientation)) in crossings.iter().copied().enumerate() {
        if !boundary_influences(program, inside_bits, object)? {
            inside_bits = oriented_toggle(inside_bits, object, orientation)?;
            continue;
        }
        inside_bits = oriented_toggle(inside_bits, object, orientation)?;
        let next = evaluate(program, inside_bits)?;
        if next != occupied {
            return Ok(Some(index));
        }
        occupied = next;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_program_matches_boolean_exhaustively() {
        let program = [
            CsgInstruction::Object(0),
            CsgInstruction::Object(1),
            CsgInstruction::Union,
            CsgInstruction::Object(2),
            CsgInstruction::Subtract,
        ];
        for bits in 0_u64..8 {
            assert_eq!(
                evaluate(&program, bits).unwrap(),
                ((bits & 1 != 0) || (bits & 2 != 0)) && bits & 4 == 0
            );
        }
    }

    #[test]
    fn noninfluential_boundary_is_skipped() {
        let program = [CsgInstruction::Object(0)];
        assert!(!boundary_influences(&program, 0, 1).unwrap());
        assert_eq!(
            first_transition(
                &program,
                0,
                &[(1, Orientation::Enter), (0, Orientation::Enter)]
            ),
            Ok(Some(1))
        );
    }
}
