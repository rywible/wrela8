//! Bounded postfix hard-CSG occupancy programs.

use super::ids::ObjectId;
use super::objects::CsgExpr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsgInst {
    Push(ObjectId),
    Not,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsgCofactorProgram {
    pub constant: Option<bool>,
    pub instructions: Vec<CsgInst>,
    pub max_stack: u32,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsgInfluence {
    pub object: ObjectId,
    pub when_false: CsgCofactorProgram,
    pub when_true: CsgCofactorProgram,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsgProgram {
    pub constant: Option<bool>,
    pub instructions: Vec<CsgInst>,
    pub max_stack: u32,
    pub influence: Vec<CsgInfluence>,
}

fn are_complements(a: &CsgExpr, b: &CsgExpr) -> bool {
    matches!(a, CsgExpr::Not(child) if child.as_ref() == b)
        || matches!(b, CsgExpr::Not(child) if child.as_ref() == a)
}

fn simplify(expr: CsgExpr) -> CsgExpr {
    match expr {
        CsgExpr::Not(child) => match simplify(*child) {
            CsgExpr::Const(value) => CsgExpr::Const(!value),
            CsgExpr::Not(grandchild) => *grandchild,
            child => CsgExpr::Not(Box::new(child)),
        },
        CsgExpr::And(a, b) => {
            let a = simplify(*a);
            let b = simplify(*b);
            if matches!(a, CsgExpr::Const(false))
                || matches!(b, CsgExpr::Const(false))
                || are_complements(&a, &b)
            {
                CsgExpr::Const(false)
            } else if matches!(a, CsgExpr::Const(true)) {
                b
            } else if matches!(b, CsgExpr::Const(true)) || a == b {
                a
            } else {
                CsgExpr::And(Box::new(a), Box::new(b))
            }
        }
        CsgExpr::Or(a, b) => {
            let a = simplify(*a);
            let b = simplify(*b);
            if matches!(a, CsgExpr::Const(true))
                || matches!(b, CsgExpr::Const(true))
                || are_complements(&a, &b)
            {
                CsgExpr::Const(true)
            } else if matches!(a, CsgExpr::Const(false)) {
                b
            } else if matches!(b, CsgExpr::Const(false)) || a == b {
                a
            } else {
                CsgExpr::Or(Box::new(a), Box::new(b))
            }
        }
        other => other,
    }
}

fn emit(expr: &CsgExpr, output: &mut Vec<CsgInst>) -> Result<(), String> {
    match expr {
        CsgExpr::Const(_) => {
            return Err("pixels::csg: constant nested after simplification".to_string());
        }
        CsgExpr::Leaf(object) => output.push(CsgInst::Push(*object)),
        CsgExpr::Not(child) => {
            emit(child, output)?;
            output.push(CsgInst::Not);
        }
        CsgExpr::And(a, b) => {
            emit(a, output)?;
            emit(b, output)?;
            output.push(CsgInst::And);
        }
        CsgExpr::Or(a, b) => {
            emit(a, output)?;
            emit(b, output)?;
            output.push(CsgInst::Or);
        }
    }
    Ok(())
}

fn stack_depth(instructions: &[CsgInst]) -> Result<u32, String> {
    let mut depth = 0_i64;
    let mut maximum = 0_i64;
    for instruction in instructions {
        match instruction {
            CsgInst::Push(_) => depth += 1,
            CsgInst::Not => {
                if depth < 1 {
                    return Err("pixels::csg: Not underflows stack".to_string());
                }
            }
            CsgInst::And | CsgInst::Or => {
                if depth < 2 {
                    return Err("pixels::csg: binary instruction underflows stack".to_string());
                }
                depth -= 1;
            }
        }
        maximum = maximum.max(depth);
    }
    if depth != 1 && !instructions.is_empty() {
        return Err(format!(
            "pixels::csg: final stack depth is {depth}, expected 1"
        ));
    }
    u32::try_from(maximum).map_err(|_| "pixels::csg: stack depth overflow".to_string())
}

fn force_object(expr: &CsgExpr, target: ObjectId, value: bool) -> CsgExpr {
    match expr {
        CsgExpr::Const(value) => CsgExpr::Const(*value),
        CsgExpr::Leaf(object) if *object == target => CsgExpr::Const(value),
        CsgExpr::Leaf(object) => CsgExpr::Leaf(*object),
        CsgExpr::Not(child) => CsgExpr::Not(Box::new(force_object(child, target, value))),
        CsgExpr::And(a, b) => CsgExpr::And(
            Box::new(force_object(a, target, value)),
            Box::new(force_object(b, target, value)),
        ),
        CsgExpr::Or(a, b) => CsgExpr::Or(
            Box::new(force_object(a, target, value)),
            Box::new(force_object(b, target, value)),
        ),
    }
}

fn encode(constant: Option<bool>, instructions: &[CsgInst]) -> Vec<u8> {
    let mut encoded = vec![match constant {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }];
    for instruction in instructions {
        match instruction {
            CsgInst::Push(object) => {
                encoded.push(3);
                encoded.extend_from_slice(&object.0.to_le_bytes());
            }
            CsgInst::Not => encoded.push(4),
            CsgInst::And => encoded.push(5),
            CsgInst::Or => encoded.push(6),
        }
    }
    encoded
}

fn compile_cofactor(expr: CsgExpr) -> Result<CsgCofactorProgram, String> {
    let expr = simplify(expr);
    let (constant, instructions, max_stack) = if let CsgExpr::Const(value) = expr {
        (Some(value), Vec::new(), 0)
    } else {
        let mut instructions = Vec::new();
        emit(&expr, &mut instructions)?;
        let max_stack = stack_depth(&instructions)?;
        (None, instructions, max_stack)
    };
    let digest = wrela_machine::sha256::sha256_hex(&encode(constant, &instructions));
    Ok(CsgCofactorProgram {
        constant,
        instructions,
        max_stack,
        digest,
    })
}

pub fn cofactor_digest(program: &CsgCofactorProgram) -> String {
    wrela_machine::sha256::sha256_hex(&encode(program.constant, &program.instructions))
}

fn evaluate_parts(
    constant: Option<bool>,
    instructions: &[CsgInst],
    max_stack: u32,
    occupancy: impl Fn(ObjectId) -> bool,
) -> Result<bool, String> {
    if let Some(value) = constant {
        return Ok(value);
    }
    let mut stack = Vec::with_capacity(max_stack as usize);
    for instruction in instructions {
        match instruction {
            CsgInst::Push(object) => stack.push(occupancy(*object)),
            CsgInst::Not => {
                let value = stack
                    .last_mut()
                    .ok_or_else(|| "pixels::csg: Not underflow during evaluation".to_string())?;
                *value = !*value;
            }
            CsgInst::And | CsgInst::Or => {
                let b = stack
                    .pop()
                    .ok_or_else(|| "pixels::csg: binary underflow during evaluation".to_string())?;
                let a = stack
                    .pop()
                    .ok_or_else(|| "pixels::csg: binary underflow during evaluation".to_string())?;
                stack.push(if matches!(instruction, CsgInst::And) {
                    a && b
                } else {
                    a || b
                });
            }
        }
    }
    match stack.as_slice() {
        [value] => Ok(*value),
        _ => Err(format!(
            "pixels::csg: evaluation ended with {} values",
            stack.len()
        )),
    }
}

pub fn evaluate(
    program: &CsgProgram,
    occupancy: impl Fn(ObjectId) -> bool,
) -> Result<bool, String> {
    evaluate_parts(
        program.constant,
        &program.instructions,
        program.max_stack,
        occupancy,
    )
}

pub fn evaluate_cofactor(
    program: &CsgCofactorProgram,
    occupancy: impl Fn(ObjectId) -> bool,
) -> Result<bool, String> {
    evaluate_parts(
        program.constant,
        &program.instructions,
        program.max_stack,
        occupancy,
    )
}

pub fn compile(expr: CsgExpr, object_count: usize) -> Result<CsgProgram, String> {
    let expr = simplify(expr);
    if let CsgExpr::Const(value) = &expr {
        return Ok(CsgProgram {
            constant: Some(*value),
            instructions: Vec::new(),
            max_stack: 0,
            influence: Vec::new(),
        });
    }
    let mut instructions = Vec::new();
    emit(&expr, &mut instructions)?;
    let max_stack = stack_depth(&instructions)?;
    let mut program = CsgProgram {
        constant: None,
        instructions,
        max_stack,
        influence: Vec::new(),
    };
    for index in 0..object_count {
        let object = ObjectId(
            u32::try_from(index).map_err(|_| "pixels::csg: object ID overflow".to_string())?,
        );
        program.influence.push(CsgInfluence {
            object,
            when_false: compile_cofactor(force_object(&expr, object, false))?,
            when_true: compile_cofactor(force_object(&expr, object, true))?,
        });
    }
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_program_matches_boolean_tree_exhaustively() {
        let expr = CsgExpr::And(
            Box::new(CsgExpr::Leaf(ObjectId(0))),
            Box::new(CsgExpr::Not(Box::new(CsgExpr::Or(
                Box::new(CsgExpr::Leaf(ObjectId(1))),
                Box::new(CsgExpr::Leaf(ObjectId(2))),
            )))),
        );
        let program = compile(expr, 3).unwrap();
        for bits in 0_u8..8 {
            let actual = evaluate(&program, |object| bits & (1 << object.0) != 0).unwrap();
            let expected = bits & 1 != 0 && !(bits & 2 != 0 || bits & 4 != 0);
            assert_eq!(actual, expected);
        }
        assert_eq!(program.max_stack, 3);
        for influence in &program.influence {
            for bits in 0_u8..8 {
                assert_eq!(
                    evaluate_cofactor(&influence.when_false, |object| {
                        bits & (1 << object.0) != 0
                    })
                    .unwrap(),
                    evaluate(&program, |object| {
                        object != influence.object && bits & (1 << object.0) != 0
                    })
                    .unwrap()
                );
                assert_eq!(
                    evaluate_cofactor(&influence.when_true, |object| {
                        bits & (1 << object.0) != 0
                    })
                    .unwrap(),
                    evaluate(&program, |object| {
                        object == influence.object || bits & (1 << object.0) != 0
                    })
                    .unwrap()
                );
            }
        }
    }

    fn eval_expr(expr: &CsgExpr, bits: u32) -> bool {
        match expr {
            CsgExpr::Const(value) => *value,
            CsgExpr::Leaf(object) => bits & (1 << object.0) != 0,
            CsgExpr::Not(child) => !eval_expr(child, bits),
            CsgExpr::And(a, b) => eval_expr(a, bits) && eval_expr(b, bits),
            CsgExpr::Or(a, b) => eval_expr(a, bits) || eval_expr(b, bits),
        }
    }

    fn alternating_tree(count: u32) -> CsgExpr {
        (1..count).fold(CsgExpr::Leaf(ObjectId(0)), |tree, object| {
            if object % 2 == 0 {
                CsgExpr::And(Box::new(tree), Box::new(CsgExpr::Leaf(ObjectId(object))))
            } else {
                CsgExpr::Or(
                    Box::new(tree),
                    Box::new(CsgExpr::Not(Box::new(CsgExpr::Leaf(ObjectId(object))))),
                )
            }
        })
    }

    #[test]
    fn stack_program_agrees_exhaustively_through_twelve_objects() {
        for object_count in 1..=12 {
            let expr = alternating_tree(object_count);
            let program = compile(expr.clone(), object_count as usize).unwrap();
            for bits in 0..(1_u32 << object_count) {
                assert_eq!(
                    evaluate(&program, |object| bits & (1 << object.0) != 0).unwrap(),
                    eval_expr(&expr, bits)
                );
            }
        }
    }

    #[test]
    fn stack_program_agrees_on_deterministic_samples_beyond_twelve_objects() {
        let expr = alternating_tree(20);
        let program = compile(expr.clone(), 20).unwrap();
        let mut bits = 0x9e37_79b9_u32;
        for _ in 0..10_000 {
            bits ^= bits << 13;
            bits ^= bits >> 17;
            bits ^= bits << 5;
            assert_eq!(
                evaluate(&program, |object| bits & (1 << object.0) != 0).unwrap(),
                eval_expr(&expr, bits)
            );
        }
    }
}
