//! Finite-body legality checks for `@field` and `@material` roots.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::typed::{
    PixelsFnKind, TypedCallArg, TypedDeferBody, TypedExpr, TypedExprKind, TypedForIter,
    TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::sema::types::{Type, TypeArg};
use crate::syntax::ast::Span;

use super::{LocatedFn, called_function, root_function};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dependency {
    Comptime,
    Coordinate,
    Parameter,
    CoordinateAndParameter,
}

impl Dependency {
    fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Comptime, value) | (value, Self::Comptime) => value,
            (Self::Coordinate, Self::Coordinate) => Self::Coordinate,
            (Self::Parameter, Self::Parameter) => Self::Parameter,
            _ => Self::CoordinateAndParameter,
        }
    }

    fn has_coordinate(self) -> bool {
        matches!(self, Self::Coordinate | Self::CoordinateAndParameter)
    }
}

fn merge_envs(
    base: &BTreeMap<String, Dependency>,
    branches: impl IntoIterator<Item = BTreeMap<String, Dependency>>,
) -> BTreeMap<String, Dependency> {
    let mut merged = base.clone();
    for branch in branches {
        for (name, dependency) in branch {
            merged
                .entry(name)
                .and_modify(|current| *current = current.combine(dependency))
                .or_insert(dependency);
        }
    }
    merged
}

fn is_field_type(ty: &Type, program: &TypedProgram) -> bool {
    match ty {
        Type::Named(name, args) => {
            super::nominal_decl(program, name).is_some_and(|(module, declaration)| {
                declaration == "Field" && matches!(module, "field" | "core.field")
            }) || args.iter().any(|arg| match arg {
                TypeArg::Type(ty) => is_field_type(ty, program),
                TypeArg::Const(_) | TypeArg::Bound(_) | TypeArg::Pool(_) => false,
            })
        }
        Type::Array(elem, _) | Type::Own(_, elem) | Type::Static(elem) | Type::Option(elem) => {
            is_field_type(elem, program)
        }
        Type::Tuple(items) => items.iter().any(|item| is_field_type(item, program)),
        Type::Result(ok, error) => is_field_type(ok, program) || is_field_type(error, program),
        Type::Fn(params, ret) => {
            params.iter().any(|(_, ty)| is_field_type(ty, program)) || is_field_type(ret, program)
        }
        _ => false,
    }
}

fn literal_number(expr: &TypedExpr) -> Option<f64> {
    match &expr.kind {
        TypedExprKind::Int(text) => {
            crate::eval::value::parse_int_literal(text).map(|value| value as f64)
        }
        TypedExprKind::Float(text) => text.replace('_', "").parse().ok(),
        TypedExprKind::Neg(inner) => literal_number(inner).map(|value| -value),
        _ => None,
    }
}

pub(super) fn constant_number(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    expr: &TypedExpr,
) -> Option<f64> {
    fn evaluate(
        programs: &BTreeMap<String, TypedProgram>,
        module: &str,
        expr: &TypedExpr,
        active: &mut Vec<String>,
    ) -> Option<f64> {
        match &expr.kind {
            TypedExprKind::Const(name) => {
                let located = super::located_constant(programs, module, name)?;
                let identity = format!("{}::{}", located.module, located.name);
                if active.contains(&identity) {
                    return None;
                }
                active.push(identity);
                let result = evaluate(programs, &located.module, &located.constant.value, active);
                active.pop();
                result
            }
            TypedExprKind::ToScalar(value) => evaluate(programs, module, value, active),
            TypedExprKind::Neg(value) => {
                evaluate(programs, module, value, active).map(|value| -value)
            }
            TypedExprKind::Binary(op, left, right)
                if matches!(
                    op,
                    crate::syntax::ast::BinOp::Add
                        | crate::syntax::ast::BinOp::Sub
                        | crate::syntax::ast::BinOp::Mul
                        | crate::syntax::ast::BinOp::Div
                        | crate::syntax::ast::BinOp::Rem
                ) =>
            {
                let left = evaluate(programs, module, left, active)?;
                let right = evaluate(programs, module, right, active)?;
                Some(match op {
                    crate::syntax::ast::BinOp::Add => left + right,
                    crate::syntax::ast::BinOp::Sub => left - right,
                    crate::syntax::ast::BinOp::Mul => left * right,
                    crate::syntax::ast::BinOp::Div => left / right,
                    crate::syntax::ast::BinOp::Rem => left % right,
                    _ => unreachable!("guarded arithmetic operator"),
                })
            }
            _ => literal_number(expr),
        }
    }

    evaluate(programs, module, expr, &mut Vec::new())
}

pub(super) fn constant_integer(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    expr: &TypedExpr,
) -> Option<i128> {
    fn evaluate(
        programs: &BTreeMap<String, TypedProgram>,
        module: &str,
        expr: &TypedExpr,
        active: &mut Vec<String>,
    ) -> Option<i128> {
        match &expr.kind {
            TypedExprKind::Int(text) => crate::eval::value::parse_int_literal(text),
            TypedExprKind::Const(name) => {
                let located = super::located_constant(programs, module, name)?;
                let identity = format!("{}::{}", located.module, located.name);
                if active.contains(&identity) {
                    return None;
                }
                active.push(identity);
                let result = evaluate(programs, &located.module, &located.constant.value, active);
                active.pop();
                result
            }
            TypedExprKind::Neg(value) => evaluate(programs, module, value, active)?.checked_neg(),
            TypedExprKind::ToScalar(value) => evaluate(programs, module, value, active),
            TypedExprKind::Binary(op, left, right)
                if matches!(
                    op,
                    crate::syntax::ast::BinOp::Add
                        | crate::syntax::ast::BinOp::Sub
                        | crate::syntax::ast::BinOp::Mul
                        | crate::syntax::ast::BinOp::Div
                        | crate::syntax::ast::BinOp::Rem
                ) =>
            {
                let left = evaluate(programs, module, left, active)?;
                let right = evaluate(programs, module, right, active)?;
                match op {
                    crate::syntax::ast::BinOp::Add => left.checked_add(right),
                    crate::syntax::ast::BinOp::Sub => left.checked_sub(right),
                    crate::syntax::ast::BinOp::Mul => left.checked_mul(right),
                    crate::syntax::ast::BinOp::Div => left.checked_div(right),
                    crate::syntax::ast::BinOp::Rem => left.checked_rem(right),
                    _ => unreachable!("guarded integer operator"),
                }
            }
            _ => None,
        }
    }

    evaluate(programs, module, expr, &mut Vec::new())
}

struct Checker<'a> {
    programs: &'a BTreeMap<String, TypedProgram>,
    active: Vec<String>,
    kind: PixelsFnKind,
    last_span: Span,
}

fn rotation_rows_are_orthonormal(rows: &[[f64; 3]; 3]) -> bool {
    const ORTHONORMAL_TOLERANCE: f64 = f32::EPSILON as f64 * 8.0;
    let dot = |left: &[f64; 3], right: &[f64; 3]| {
        left.iter()
            .zip(right)
            .map(|(left, right)| left * right)
            .sum::<f64>()
    };
    rows.iter()
        .all(|row| (dot(row, row) - 1.0).abs() <= ORTHONORMAL_TOLERANCE)
        && (0..3).all(|left| {
            ((left + 1)..3)
                .all(|right| dot(&rows[left], &rows[right]).abs() <= ORTHONORMAL_TOLERANCE)
        })
}

#[derive(Default)]
struct DirectCallCollector {
    calls: BTreeSet<String>,
}

impl crate::eval::walk::Visitor for DirectCallCollector {
    fn on_callee(&mut self, key: String) {
        self.calls.insert(key);
    }
}

fn function_identity(function: &LocatedFn) -> String {
    format!("{}::{}", function.module, function.key)
}

fn reachable_nodes(graph: &BTreeMap<String, BTreeSet<String>>, start: &str) -> BTreeSet<String> {
    let mut reached = BTreeSet::new();
    let mut pending = vec![start.to_string()];
    while let Some(node) = pending.pop() {
        if !reached.insert(node.clone()) {
            continue;
        }
        if let Some(edges) = graph.get(&node) {
            pending.extend(edges.iter().rev().cloned());
        }
    }
    reached
}

impl<'a> Checker<'a> {
    fn direct_callees(&self, located: &LocatedFn) -> Vec<LocatedFn> {
        let mut collector = DirectCallCollector::default();
        for param in &located.function.params {
            if let Some(default) = &param.default {
                crate::eval::walk::walk_expr(default, &mut collector);
            }
        }
        crate::eval::walk::walk_stmts(&located.function.body, &mut collector);
        collector
            .calls
            .into_iter()
            .filter_map(|spelling| {
                let function = called_function(self.programs, &located.module, &spelling)?;
                if super::is_core_field_function(&function)
                    || super::is_core_scalar_function(&function)
                    || (self.kind == PixelsFnKind::Material
                        && super::is_core_material_constructor(
                            self.programs,
                            &located.module,
                            &spelling,
                        ))
                {
                    None
                } else {
                    Some(function)
                }
            })
            .collect()
    }

    fn recursive_component(&self, root: &LocatedFn) -> Option<Vec<String>> {
        let mut graph = BTreeMap::<String, BTreeSet<String>>::new();
        let mut pending = vec![root.clone()];
        while let Some(function) = pending.pop() {
            let identity = function_identity(&function);
            if graph.contains_key(&identity) {
                continue;
            }
            let callees = self.direct_callees(&function);
            let edges = callees.iter().map(function_identity).collect();
            graph.insert(identity, edges);
            pending.extend(callees);
        }

        let mut reverse = graph
            .keys()
            .map(|identity| (identity.clone(), BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        for (caller, callees) in &graph {
            for callee in callees {
                reverse
                    .entry(callee.clone())
                    .or_default()
                    .insert(caller.clone());
            }
        }
        for identity in graph.keys() {
            let descendants = reachable_nodes(&graph, identity);
            let ancestors = reachable_nodes(&reverse, identity);
            let component: Vec<String> = graph
                .keys()
                .filter(|candidate| {
                    descendants.contains(*candidate) && ancestors.contains(*candidate)
                })
                .cloned()
                .collect();
            let self_recursive = graph
                .get(identity)
                .is_some_and(|edges| edges.contains(identity));
            if component.len() > 1 || self_recursive {
                return Some(component);
            }
        }
        None
    }

    fn is_nominal_type(
        &self,
        module: &str,
        ty: &Type,
        declaration: &str,
        declaring_modules: &[&str],
    ) -> bool {
        let Type::Named(name, args) = ty else {
            return false;
        };
        args.is_empty()
            && super::program_for_decl_module(self.programs, module).is_some_and(|program| {
                super::nominal_decl(program, name).is_some_and(|(found_module, found_name)| {
                    found_name == declaration && declaring_modules.contains(&found_module)
                })
            })
    }

    fn is_field_vector_type(&self, module: &str, ty: &Type) -> bool {
        ["Vec2", "Vec3", "Vec4"]
            .iter()
            .any(|name| self.is_nominal_type(module, ty, name, &["field", "core.field"]))
    }

    fn is_field_type(&self, module: &str, ty: &Type) -> bool {
        super::program_for_decl_module(self.programs, module)
            .is_some_and(|program| is_field_type(ty, program))
    }
    fn root_name(&self) -> &str {
        self.active
            .first()
            .and_then(|identity| identity.rsplit("::").next())
            .unwrap_or("<unknown>")
    }

    fn in_core_field_vector_method(&self) -> bool {
        self.active
            .last()
            .and_then(|identity| identity.split_once("::"))
            .is_some_and(|(module, key)| super::is_core_field_vector_method_identity(module, key))
    }

    fn reject(&self, message: impl AsRef<str>) -> String {
        let mut message = message.as_ref().to_string();
        if !self.active.is_empty() {
            message.push_str("\n  renderer call chain: ");
            message.push_str(&self.active.join(" -> "));
        }
        message
    }

    fn check_root(&mut self, located: LocatedFn) -> Result<(), String> {
        if let Some(component) = self.recursive_component(&located) {
            return Err(format!(
                "P023: field recursion is forbidden; cycle members: {}",
                component.join(", ")
            ));
        }
        let mut env = BTreeMap::new();
        for (index, param) in located.function.params.iter().enumerate() {
            let dependency = match self.kind {
                PixelsFnKind::Field if index == 0 => Dependency::Coordinate,
                PixelsFnKind::Field if index == 1 => Dependency::Parameter,
                PixelsFnKind::Material => Dependency::Parameter,
                _ => Dependency::Comptime,
            };
            env.insert(param.name.clone(), dependency);
        }
        self.check_function(located, env)
    }

    fn check_function(
        &mut self,
        located: LocatedFn,
        env: BTreeMap<String, Dependency>,
    ) -> Result<(), String> {
        let identity = function_identity(&located);
        if let Some(start) = self.active.iter().position(|item| item == &identity) {
            let mut cycle = self.active[start..].to_vec();
            cycle.push(identity);
            return Err(format!(
                "P023: field recursion is forbidden; cycle members: {}",
                cycle.join(" -> ")
            ));
        }
        self.active.push(identity);
        let result = self
            .check_stmts(&located.function.body, &located.module, env)
            .map(|_| ());
        self.active.pop();
        result
    }

    fn check_resolved_function(
        &mut self,
        function: LocatedFn,
        receiver_dependency: Option<Dependency>,
        argument_dependencies: Vec<Dependency>,
    ) -> Result<(), String> {
        let mut call_env: BTreeMap<String, Dependency> = function
            .function
            .params
            .iter()
            .zip(argument_dependencies)
            .map(|(param, dependency)| (param.name.clone(), dependency))
            .collect();
        if function.function.receiver.is_some() {
            let Some(receiver_dependency) = receiver_dependency else {
                return Err(self.reject(format!(
                    "P003: renderer method `{}` has no checked receiver",
                    function.key
                )));
            };
            call_env.insert("self".to_string(), receiver_dependency);
        }
        self.check_function(function, call_env)
    }

    fn enum_payload_dependencies(
        &mut self,
        args: &[TypedCallArg],
        module: &str,
        env: &mut BTreeMap<String, Dependency>,
    ) -> Result<Vec<Dependency>, String> {
        args.iter()
            .map(|arg| match &arg.value {
                Some(value) => self.check_expr(value, module, env),
                None => Err(self.reject(
                    "P003: renderer enum payloads may not contain an omitted or moved-out value",
                )),
            })
            .collect()
    }

    fn validate_field_call(
        &self,
        module: &str,
        name: &str,
        args: &[TypedCallArg],
        arg_dependencies: &[Dependency],
    ) -> Result<(), String> {
        let coordinate_index = if name == "sinusoidal_displace" {
            Some(1)
        } else if matches!(
            name,
            "plane"
                | "sphere"
                | "box"
                | "round_box"
                | "capsule"
                | "finite_cylinder"
                | "finite_cone"
                | "torus"
                | "translate"
                | "rotate"
                | "rigid_transform"
                | "finite_repeat_x"
                | "finite_repeat_y"
                | "finite_repeat_z"
        ) {
            Some(0)
        } else {
            None
        };
        if let Some(index) = coordinate_index
            && !matches!(
                arg_dependencies.get(index),
                Some(Dependency::Coordinate | Dependency::CoordinateAndParameter)
            )
        {
            return Err(self.reject(format!(
                "P004: field operation `{name}` coordinate input must be derived from the renderer coordinate",
            )));
        }
        let coefficient_indices: &[usize] = match name {
            "plane" => &[1, 2],
            "sphere" => &[1, 2],
            "box" => &[1],
            "round_box" => &[1, 2],
            "capsule" => &[1, 2, 3],
            "finite_cylinder" | "finite_cone" | "torus" => &[1, 2],
            "translate" => &[1],
            "rotate" => &[1, 2, 3],
            "rigid_transform" => &[1, 2, 3, 4],
            "finite_repeat_x" | "finite_repeat_y" | "finite_repeat_z" => &[1, 2, 3],
            "uniform_scale" => &[1],
            "smooth_union" | "smooth_intersection" | "smooth_subtract" => &[2],
            "sinusoidal_displace" => &[2, 3, 4],
            _ => &[],
        };
        for index in coefficient_indices {
            if arg_dependencies
                .get(*index)
                .is_some_and(|dependency| dependency.has_coordinate())
            {
                return Err(self.reject(format!(
                    "P004: field operation `{name}` coefficient argument {} may depend on renderer parameters but not on the renderer coordinate",
                    index + 1,
                )));
            }
        }
        if matches!(name, "rotate" | "rigid_transform") {
            let row_start = if name == "rotate" { 1 } else { 2 };
            let mut rows = [[0.0; 3]; 3];
            let mut all_constant = true;
            for (row, argument) in rows.iter_mut().zip(&args[row_start..]) {
                let Some(value) = argument.value.as_ref() else {
                    return Err(self.reject(format!(
                        "P004: field operation `{name}` requires explicit `Vec3` rotation rows"
                    )));
                };
                let TypedExprKind::StructLiteral { fields, .. } = &value.kind else {
                    return Err(self.reject(format!(
                        "P004: field operation `{name}` requires `Vec3` rotation rows"
                    )));
                };
                if !self.is_nominal_type(module, &value.ty, "Vec3", &["field", "core.field"])
                    || fields.len() != 3
                {
                    return Err(self.reject(format!(
                        "P004: field operation `{name}` has malformed rotation rows"
                    )));
                }
                for (component, (_, value)) in row.iter_mut().zip(fields) {
                    if let Some(constant) = self.constant_number(module, value) {
                        *component = constant;
                    } else {
                        all_constant = false;
                    }
                }
            }
            if !all_constant {
                return Err(self.reject(format!(
                    "P004: field operation `{name}` rotation rows must be compiler-proven rigid; \
                     arbitrary parameterized matrix coefficients are not a rotation"
                )));
            } else {
                // Rotation coefficients are f32 source values evaluated as
                // f64 by the checker. Nonsingularity alone is insufficient:
                // accepting a parameterized shear here would invalidate every
                // downstream rigid-transform derivative contract.
                if !rotation_rows_are_orthonormal(&rows) {
                    return Err(self.reject(format!(
                        "P004: field operation `{name}` rotation rows are not orthonormal \
                         within f32 roundoff"
                    )));
                }
            }
        }
        if name == "mark" {
            let object = args.get(1).and_then(|arg| arg.value.as_ref());
            let material = args.get(2).and_then(|arg| arg.value.as_ref());
            if !object.is_some_and(|value| self.enum_constant(module, value))
                || !material.is_some_and(|value| self.enum_constant(module, value))
                || arg_dependencies.get(1) != Some(&Dependency::Comptime)
                || arg_dependencies.get(2) != Some(&Dependency::Comptime)
            {
                return Err(self.reject(
                    "P004: `mark` object and material identities must be comptime enum variants",
                ));
            }
        }
        if matches!(
            name,
            "finite_repeat_x" | "finite_repeat_y" | "finite_repeat_z"
        ) {
            for (index, label) in [(1, "first"), (2, "count")] {
                let Some(value) = args.get(index).and_then(|arg| arg.value.as_ref()) else {
                    return Err(self.reject(format!(
                        "P012: `{name}` is missing `{label}` after type checking"
                    )));
                };
                if self.constant_number(module, value).is_none() {
                    return Err(self.reject(format!(
                        "P012: `{name}` `{label}` must be a comptime constant"
                    )));
                }
            }
            if let Some(count) = args
                .get(2)
                .and_then(|arg| arg.value.as_ref())
                .and_then(|value| self.constant_number(module, value))
                && count <= 0.0
            {
                return Err(self.reject(format!("P012: `{name}` count must be positive")));
            }
            if let Some(period) = args
                .get(3)
                .and_then(|arg| arg.value.as_ref())
                .and_then(|value| self.constant_number(module, value))
            {
                if !period.is_finite() || period <= 0.0 {
                    return Err(
                        self.reject(format!("P012: `{name}` period must be finite and positive"))
                    );
                }
            }
        }
        if matches!(
            name,
            "smooth_union" | "smooth_intersection" | "smooth_subtract"
        ) {
            if let Some(k) = args
                .get(2)
                .and_then(|arg| arg.value.as_ref())
                .and_then(|value| self.constant_number(module, value))
            {
                if !k.is_finite() || k <= 0.0 {
                    return Err(self.reject(format!(
                        "P011: `{name}` smoothing width must be finite and positive"
                    )));
                }
            }
        }
        if name == "uniform_scale"
            && let Some(scale) = args
                .get(1)
                .and_then(|arg| arg.value.as_ref())
                .and_then(|value| self.constant_number(module, value))
            && (!scale.is_finite() || scale <= 0.0)
        {
            return Err(
                self.reject("P004: `uniform_scale` scale must be finite and strictly positive")
            );
        }
        Ok(())
    }

    fn enum_constant(&self, module: &str, expr: &TypedExpr) -> bool {
        match &expr.kind {
            TypedExprKind::EnumConstruct { .. } => true,
            TypedExprKind::Const(name) => super::located_constant(self.programs, module, name)
                .is_some_and(|located| {
                    self.enum_constant(&located.module, &located.constant.value)
                }),
            _ => false,
        }
    }

    fn constant_number(&self, module: &str, expr: &TypedExpr) -> Option<f64> {
        constant_number(self.programs, module, expr)
    }

    fn constant_bool(&self, module: &str, expr: &TypedExpr) -> Option<bool> {
        match &expr.kind {
            TypedExprKind::Bool(value) => Some(*value),
            TypedExprKind::Const(name) => super::located_constant(self.programs, module, name)
                .and_then(|located| self.constant_bool(&located.module, &located.constant.value)),
            TypedExprKind::Not(value) => self.constant_bool(module, value).map(|value| !value),
            TypedExprKind::And(left, right) => {
                let left = self.constant_bool(module, left)?;
                if left {
                    self.constant_bool(module, right)
                } else {
                    Some(false)
                }
            }
            TypedExprKind::Or(left, right) => {
                let left = self.constant_bool(module, left)?;
                if left {
                    Some(true)
                } else {
                    self.constant_bool(module, right)
                }
            }
            TypedExprKind::Binary(op, left, right) => {
                let left = self.constant_number(module, left)?;
                let right = self.constant_number(module, right)?;
                Some(match op {
                    crate::syntax::ast::BinOp::Lt => left < right,
                    crate::syntax::ast::BinOp::Le => left <= right,
                    crate::syntax::ast::BinOp::Gt => left > right,
                    crate::syntax::ast::BinOp::Ge => left >= right,
                    crate::syntax::ast::BinOp::Eq => left == right,
                    crate::syntax::ast::BinOp::Ne => left != right,
                    _ => return None,
                })
            }
            _ => None,
        }
    }

    fn check_expr(
        &mut self,
        expr: &TypedExpr,
        module: &str,
        env: &mut BTreeMap<String, Dependency>,
    ) -> Result<Dependency, String> {
        let span = expr.span;
        self.last_span = span;
        let result = self.check_expr_inner(expr, module, env);
        if result.is_ok() {
            self.last_span = span;
        }
        result
    }

    fn check_expr_inner(
        &mut self,
        expr: &TypedExpr,
        module: &str,
        env: &mut BTreeMap<String, Dependency>,
    ) -> Result<Dependency, String> {
        match &expr.kind {
            TypedExprKind::Int(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Unit => Ok(Dependency::Comptime),
            TypedExprKind::Str(_)
            | TypedExprKind::BStr(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::PoolName(_) => Err(self.reject(
                "P003: renderer bodies may not use text, character, byte-string, or pool values",
            )),
            TypedExprKind::Const(_)
                if matches!(
                    expr.ty,
                    Type::Char | Type::Str | Type::Bytes(_) | Type::String(_)
                ) =>
            {
                Err(self.reject(
                    "P003: renderer bodies may not use text, character, or byte-string constants",
                ))
            }
            TypedExprKind::Const(_) => Ok(Dependency::Comptime),
            TypedExprKind::Local(name) => Ok(*env.get(name).unwrap_or(&Dependency::Parameter)),
            TypedExprKind::Static(name) => Err(self.reject(format!(
                "P003: renderer bodies may not read placed/static value `{name}`"
            ))),
            TypedExprKind::FnRef(key) => Err(self.reject(format!(
                "P003: renderer bodies may not store function value `{}`",
                key.spelling()
            ))),
            TypedExprKind::Call {
                callee,
                receiver,
                args,
            } => {
                let mut dependency = Dependency::Comptime;
                let receiver_dependency = receiver
                    .as_ref()
                    .map(|receiver| self.check_expr(receiver, module, env))
                    .transpose()?;
                if let Some(receiver_dependency) = receiver_dependency {
                    dependency = dependency.combine(receiver_dependency);
                }
                let spelling = callee.spelling();
                let resolved = called_function(self.programs, module, &spelling);
                let mut arg_dependencies = Vec::with_capacity(args.len());
                let mut default_env = BTreeMap::new();
                if let Some(receiver_dependency) = receiver_dependency {
                    default_env.insert("self".to_string(), receiver_dependency);
                }
                for (index, argument) in args.iter().enumerate() {
                    let argument_dependency = if let Some(value) = &argument.value {
                        self.check_expr(value, module, env)?
                    } else if let Some((default, default_module)) =
                        resolved.as_ref().and_then(|function| {
                            function
                                .function
                                .params
                                .get(index)
                                .and_then(|param| param.default.as_ref())
                                .map(|default| (default, function.module.as_str()))
                        })
                    {
                        self.check_expr(default, default_module, &mut default_env)?
                    } else {
                        Dependency::Comptime
                    };
                    arg_dependencies.push(argument_dependency);
                    if let Some(param) = resolved
                        .as_ref()
                        .and_then(|function| function.function.params.get(index))
                    {
                        default_env.insert(param.name.clone(), argument_dependency);
                    }
                }
                for arg in &arg_dependencies {
                    dependency = dependency.combine(*arg);
                }
                if resolved
                    .as_ref()
                    .is_some_and(super::is_core_field_vector_method)
                {
                    if self.kind == PixelsFnKind::Field
                        && arg_dependencies
                            .first()
                            .is_some_and(|dependency| dependency.has_coordinate())
                    {
                        return Err(self.reject(
                            "P004: Vec3 coordinate methods require the coordinate vector as the receiver and a coordinate-free argument",
                        ));
                    }
                    return Ok(dependency);
                }
                if let Some(located) = resolved
                    .as_ref()
                    .filter(|located| super::is_core_field_function(located))
                {
                    self.validate_field_call(module, &located.decl_name, args, &arg_dependencies)?;
                    return Ok(dependency);
                }
                if let Some(located) = resolved
                    .as_ref()
                    .filter(|located| super::is_core_scalar_function(located))
                {
                    if self.kind == PixelsFnKind::Field
                        && self.is_field_vector_type(module, &expr.ty)
                        && dependency.has_coordinate()
                    {
                        return Err(self.reject(format!(
                            "P004: coordinate-dependent `{}` output cannot replace the closed rigid/translation coordinate set",
                            located.decl_name
                        )));
                    }
                    return Ok(dependency);
                }
                if self.kind == PixelsFnKind::Material
                    && super::is_core_material_constructor(self.programs, module, &spelling)
                {
                    return Ok(dependency);
                }
                let Some(function) = resolved else {
                    if self.is_field_type(module, &expr.ty) {
                        return Err(self.reject(format!(
                            "P004: field operation `{spelling}` is not available in `AaaByteExact`: \
                             no canonical checked callee is available"
                        )));
                    }
                    return Err(self.reject(format!(
                        "P003: renderer callee `{spelling}` has no available checked body"
                    )));
                };
                self.check_resolved_function(function, receiver_dependency, arg_dependencies)?;
                Ok(dependency)
            }
            TypedExprKind::CallValue(_, _) => {
                Err(self.reject("P003: renderer bodies may not call closures or function values"))
            }
            TypedExprKind::Field(base, _) | TypedExprKind::Take(base) => {
                self.check_expr(base, module, env)
            }
            TypedExprKind::Neg(base) => {
                let dependency = self.check_expr(base, module, env)?;
                if dependency != Dependency::Comptime
                    && (expr.ty == Type::F64 || base.ty == Type::F64)
                {
                    return Err(self.reject(
                        "P003: runtime f64 arithmetic is outside the closed renderer subset",
                    ));
                }
                if dependency != Dependency::Comptime
                    && matches!(
                        base.ty,
                        Type::U8
                            | Type::U16
                            | Type::U32
                            | Type::U64
                            | Type::Usize
                            | Type::I8
                            | Type::I16
                            | Type::I32
                            | Type::I64
                            | Type::Isize
                    )
                {
                    return Err(self.reject(
                        "P003: runtime integer arithmetic is outside the closed renderer subset",
                    ));
                }
                Ok(dependency)
            }
            TypedExprKind::ToScalar(base) => {
                let dependency = self.check_expr(base, module, env)?;
                let output_is_integer = matches!(
                    expr.ty,
                    Type::U8
                        | Type::U16
                        | Type::U32
                        | Type::U64
                        | Type::Usize
                        | Type::I8
                        | Type::I16
                        | Type::I32
                        | Type::I64
                        | Type::Isize
                );
                if dependency != Dependency::Comptime
                    && (expr.ty == Type::F64
                        || base.ty == Type::F64
                        || (output_is_integer && base.ty == Type::F32))
                {
                    return Err(self.reject(
                        "P003: this runtime scalar conversion is outside the closed renderer subset",
                    ));
                }
                Ok(dependency)
            }
            TypedExprKind::BitNot(base) => {
                self.check_expr(base, module, env)?;
                Err(self.reject("P003: bitwise negation is outside the closed renderer subset"))
            }
            TypedExprKind::Not(base) => {
                let dependency = self.check_expr(base, module, env)?;
                if dependency != Dependency::Comptime {
                    return Err(self.reject(
                        "P003: runtime boolean negation is outside the closed material predicate subset",
                    ));
                }
                Ok(dependency)
            }
            TypedExprKind::Try(base, conversion) => {
                let dependency = self.check_expr(base, module, env)?;
                if let Some(conversion) = conversion {
                    let spelling = conversion.spelling();
                    let Some(function) = called_function(self.programs, module, &spelling) else {
                        return Err(self.reject(format!(
                            "P003: renderer conversion `{spelling}` has no available checked body"
                        )));
                    };
                    self.check_resolved_function(function, None, vec![dependency])?;
                }
                Err(self.reject("P003: renderer bodies may not use `?` or exception-like recovery"))
            }
            TypedExprKind::Panic(_) => {
                Err(self.reject("P003: renderer bodies may not panic on a reachable path"))
            }
            TypedExprKind::Await(_) | TypedExprKind::Send(_) | TypedExprKind::GroupChild(_) => {
                Err(self.reject("P003: renderer bodies may not use actors, groups, await, or send"))
            }
            TypedExprKind::Index(base, index) => {
                let a = self.check_expr(base, module, env)?;
                let b = self.check_expr(index, module, env)?;
                let finite_extent = match &base.ty {
                    Type::Array(_, length) => crate::sema::bodies::literal_array_len(length)
                        .and_then(|length| usize::try_from(length).ok())
                        .filter(|length| *length <= crate::sema::bodies::MAX_ARRAY_LEN as usize),
                    _ => None,
                };
                if let Some(constant) = constant_integer(self.programs, module, index) {
                    if !finite_extent.is_some_and(|length| {
                        usize::try_from(constant).is_ok_and(|index| index < length)
                    }) {
                        return Err(self.reject(format!(
                            "P003: renderer array index {constant} is outside its comptime extent"
                        )));
                    }
                } else if b != Dependency::Comptime && finite_extent.is_none() {
                    return Err(self.reject(
                        "P003: renderer dynamic indexing must have comptime finite alternatives",
                    ));
                }
                Ok(a.combine(b))
            }
            TypedExprKind::Binary(_, base, index) => {
                let a = self.check_expr(base, module, env)?;
                let b = self.check_expr(index, module, env)?;
                let dependency = a.combine(b);
                let integer_operand = matches!(
                    base.ty,
                    Type::U8
                        | Type::U16
                        | Type::U32
                        | Type::U64
                        | Type::Usize
                        | Type::I8
                        | Type::I16
                        | Type::I32
                        | Type::I64
                        | Type::Isize
                );
                if integer_operand && dependency != Dependency::Comptime {
                    return Err(self.reject(
                        "P003: runtime integer arithmetic and comparison are outside the closed renderer subset",
                    ));
                }
                if dependency != Dependency::Comptime
                    && (base.ty == Type::F64 || index.ty == Type::F64)
                {
                    return Err(self.reject(
                        "P003: runtime f64 arithmetic and comparison are outside the closed renderer subset",
                    ));
                }
                Ok(dependency)
            }
            TypedExprKind::And(base, index) | TypedExprKind::Or(base, index) => {
                let left_dependency = self.check_expr(base, module, env)?;
                if left_dependency == Dependency::Comptime
                    && self.constant_bool(module, base).is_some_and(|left| {
                        matches!(expr.kind, TypedExprKind::And(_, _)) && !left
                            || matches!(expr.kind, TypedExprKind::Or(_, _)) && left
                    })
                {
                    return Ok(Dependency::Comptime);
                }
                let dependency = left_dependency.combine(self.check_expr(index, module, env)?);
                if dependency != Dependency::Comptime {
                    return Err(self.reject(
                        "P003: runtime boolean conjunction is outside the closed material predicate subset",
                    ));
                }
                Ok(dependency)
            }
            TypedExprKind::OpCall(callee, receiver, argument) => {
                let receiver_dependency = self.check_expr(receiver, module, env)?;
                let argument_dependency = self.check_expr(argument, module, env)?;
                let spelling = callee.spelling();
                let Some(function) = called_function(self.programs, module, &spelling) else {
                    return Err(self.reject(format!(
                        "P003: renderer operator `{spelling}` has no available checked body"
                    )));
                };
                self.check_resolved_function(
                    function,
                    Some(receiver_dependency),
                    vec![argument_dependency],
                )?;
                Ok(receiver_dependency.combine(argument_dependency))
            }
            TypedExprKind::Is(value, _) => {
                let dependency = self.check_expr(value, module, env)?;
                if dependency != Dependency::Comptime {
                    return Err(self.reject(
                        "P003: runtime identity tests are outside the closed renderer subset; use an explicit material identity match",
                    ));
                }
                Ok(dependency)
            }
            TypedExprKind::EnumConstruct { args, .. } => {
                let mut dependency = Dependency::Comptime;
                for arg in self.enum_payload_dependencies(args, module, env)? {
                    dependency = dependency.combine(arg);
                }
                Ok(dependency)
            }
            TypedExprKind::Closure { .. } => {
                Err(self.reject("P003: renderer bodies may not construct closures"))
            }
            TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
                if self.is_field_type(module, &expr.ty)
                    && matches!(expr.kind, TypedExprKind::List(_))
                {
                    return Err(self.reject("P004: `Field` values may not be stored in arrays"));
                }
                let mut dependency = Dependency::Comptime;
                for item in items {
                    dependency = dependency.combine(self.check_expr(item, module, env)?);
                }
                Ok(dependency)
            }
            TypedExprKind::StructLiteral { fields, .. } => {
                if self.is_field_type(module, &expr.ty)
                    && !self.is_nominal_type(module, &expr.ty, "Field", &["field", "core.field"])
                {
                    return Err(
                        self.reject("P004: `Field` values may not be stored in user structs")
                    );
                }
                let mut dependency = Dependency::Comptime;
                for (_, value) in fields {
                    dependency = dependency.combine(self.check_expr(value, module, env)?);
                }
                let closed_field_vector = ["Vec2", "Vec3", "Rgb"].iter().any(|name| {
                    self.is_nominal_type(module, &expr.ty, name, &["field", "core.field"])
                });
                if !closed_field_vector {
                    let supplied = fields
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<BTreeSet<_>>();
                    let (default_module, strukt) =
                        super::typed_struct_decl(self.programs, module, &expr.ty)
                            .map(|(module, strukt)| (module.to_string(), strukt.clone()))
                            .ok_or_else(|| {
                                self.reject("P004: renderer struct type is unresolved")
                            })?;
                    for field in &strukt.fields {
                        if supplied.contains(field.as_str()) {
                            continue;
                        }
                        let default = strukt.field_defaults.get(field).ok_or_else(|| {
                            self.reject(format!(
                                "P004: renderer struct field `{field}` has no value or default"
                            ))
                        })?;
                        dependency = dependency.combine(self.check_expr(
                            default,
                            &default_module,
                            &mut BTreeMap::new(),
                        )?);
                    }
                }
                if self.kind == PixelsFnKind::Field
                    && self.is_field_vector_type(module, &expr.ty)
                    && dependency.has_coordinate()
                    && !self.in_core_field_vector_method()
                {
                    return Err(self.reject(
                        "P004: coordinate vectors must come from the closed rigid/translation transform set",
                    ));
                }
                Ok(dependency)
            }
            TypedExprKind::Intrinsic { key, .. } => Err(self.reject(format!(
                "P003: renderer bodies may not use compiler intrinsic `{key}`"
            ))),
        }
    }

    fn exact_for_iter(
        &mut self,
        iter: &TypedForIter,
        module: &str,
        env: &mut BTreeMap<String, Dependency>,
    ) -> Result<(usize, Dependency), String> {
        match iter {
            TypedForIter::Range(start, end, inclusive) => {
                let dependency = self
                    .check_expr(start, module, env)?
                    .combine(self.check_expr(end, module, env)?);
                if dependency != Dependency::Comptime {
                    return Err(self.reject("P024: field loop bound is not comptime-known"));
                }
                let start = constant_integer(self.programs, module, start)
                    .ok_or_else(|| self.reject("P024: field loop bound is not comptime-known"))?;
                let end = constant_integer(self.programs, module, end)
                    .ok_or_else(|| self.reject("P024: field loop bound is not comptime-known"))?;
                let trips = if end < start {
                    Some(0)
                } else if *inclusive {
                    end.checked_sub(start)
                        .and_then(|trips| trips.checked_add(1))
                } else {
                    end.checked_sub(start)
                }
                .ok_or_else(|| {
                    self.reject("P024: field loop extent overflows exact integer arithmetic")
                })?;
                let trips = usize::try_from(trips).map_err(|_| {
                    self.reject("P024: field loop extent is not representable as `usize`")
                })?;
                if trips > crate::sema::bodies::MAX_ARRAY_LEN as usize {
                    return Err(self.reject(format!(
                        "P024: field loop extent exceeds the {}-iteration build limit",
                        crate::sema::bodies::MAX_ARRAY_LEN
                    )));
                }
                Ok((trips, Dependency::Comptime))
            }
            TypedForIter::Expr(value) => {
                let dependency = self.check_expr(value, module, env)?;
                if !matches!(value.ty, Type::Array(_, _)) {
                    return Err(self.reject("P024: field loop bound is not comptime-known"));
                }
                let Type::Array(_, length) = &value.ty else {
                    unreachable!("array type checked above")
                };
                let length = crate::sema::bodies::literal_array_len(length).ok_or_else(|| {
                    self.reject("P024: field loop array extent is not comptime-known")
                })?;
                usize::try_from(length)
                    .map(|length| (length, dependency))
                    .map_err(|_| self.reject("P024: field loop array extent is out of range"))
            }
        }
    }

    fn check_stmts(
        &mut self,
        stmts: &[TypedStmt],
        module: &str,
        mut env: BTreeMap<String, Dependency>,
    ) -> Result<BTreeMap<String, Dependency>, String> {
        for stmt in stmts {
            match &stmt.kind {
                TypedStmtKind::Let { name, value, .. } => {
                    let dependency = self.check_expr(value, module, &mut env)?;
                    env.insert(name.clone(), dependency);
                }
                TypedStmtKind::Assign { target, value } => {
                    let dependency = self.check_expr(value, module, &mut env)?;
                    let TypedExprKind::Local(name) = &target.kind else {
                        return Err(self.reject(
                            "P003: renderer bodies may mutate only local scalar/data values",
                        ));
                    };
                    env.insert(name.clone(), dependency);
                }
                TypedStmtKind::If {
                    cond,
                    then_branch,
                    elifs,
                    else_branch,
                } => {
                    let dependency = self.check_expr(cond, module, &mut env)?;
                    if self.kind == PixelsFnKind::Field && dependency != Dependency::Comptime {
                        return Err(self.reject(format!(
                            "P003: `@field` function `{}` uses runtime control flow that changes field topology",
                            self.root_name()
                        )));
                    }
                    let mut branch_envs =
                        vec![self.check_stmts(then_branch, module, env.clone())?];
                    for elif in elifs {
                        let dependency = self.check_expr(&elif.cond, module, &mut env)?;
                        if self.kind == PixelsFnKind::Field && dependency != Dependency::Comptime {
                            return Err(self.reject(format!(
                                "P003: `@field` function `{}` uses runtime control flow that changes field topology",
                                self.root_name()
                            )));
                        }
                        branch_envs.push(self.check_stmts(&elif.body, module, env.clone())?);
                    }
                    if let Some(body) = else_branch {
                        branch_envs.push(self.check_stmts(body, module, env.clone())?);
                    }
                    env = merge_envs(&env, branch_envs);
                }
                TypedStmtKind::Match { scrutinee, arms } => {
                    let dependency = self.check_expr(scrutinee, module, &mut env)?;
                    if self.kind == PixelsFnKind::Field && dependency != Dependency::Comptime {
                        return Err(self.reject(format!(
                            "P003: `@field` function `{}` uses runtime control flow that changes field topology",
                            self.root_name()
                        )));
                    }
                    let mut branch_envs = Vec::new();
                    for arm in arms {
                        if arm.guard.is_some() {
                            return Err(self.reject(
                                "P003: guarded matches are outside the closed renderer subset",
                            ));
                        }
                        if self.kind == PixelsFnKind::Material
                            && matches!(arm.pattern.kind, TypedPatternKind::Wildcard)
                        {
                            return Err(self.reject(
                                "P003: material identity matches require explicit nominal variants",
                            ));
                        }
                        branch_envs.push(self.check_stmts(&arm.body, module, env.clone())?);
                    }
                    env = merge_envs(&env, branch_envs);
                }
                TypedStmtKind::For {
                    name, iter, body, ..
                } => {
                    let (trips, item_dependency) = self.exact_for_iter(iter, module, &mut env)?;
                    let mut loop_env = env.clone();
                    let mut states = Vec::with_capacity(trips);
                    for _ in 0..trips {
                        loop_env.insert(name.clone(), item_dependency);
                        loop_env = self.check_stmts(body, module, loop_env)?;
                        states.push(loop_env.clone());
                    }
                    env = merge_envs(&env, states);
                }
                TypedStmtKind::While { .. } => {
                    return Err(
                        self.reject("P024: field loop bound is not comptime-known (`while`)")
                    );
                }
                TypedStmtKind::Break | TypedStmtKind::Continue => {
                    return Err(
                        self.reject("P003: renderer loops may not use `break` or `continue`")
                    );
                }
                TypedStmtKind::Pass | TypedStmtKind::Return(None) => {}
                TypedStmtKind::Return(Some(value)) | TypedStmtKind::ExprStmt(value) => {
                    self.check_expr(value, module, &mut env)?;
                }
                TypedStmtKind::BareSend { .. } | TypedStmtKind::WithGroup { .. } => {
                    return Err(self.reject(
                        "P003: renderer bodies may not use actors, groups, await, or send",
                    ));
                }
                TypedStmtKind::Assert { .. } => {
                    return Err(
                        self.reject("P003: renderer bodies may not contain runtime assertions")
                    );
                }
                TypedStmtKind::ComptimeAssert { cond, message, .. } => {
                    if self.check_expr(cond, module, &mut env)? != Dependency::Comptime {
                        return Err(self
                            .reject("P003: renderer comptime assertion depends on runtime data"));
                    }
                    if let Some(message) = message {
                        self.check_expr(message, module, &mut env)?;
                    }
                }
                TypedStmtKind::Defer(TypedDeferBody::Expr(_))
                | TypedStmtKind::Defer(TypedDeferBody::Suite(_)) => {
                    return Err(self.reject("P003: renderer bodies may not use `defer`"));
                }
            }
        }
        Ok(env)
    }
}

pub fn check_renderer_roots(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    field: &str,
    material: &str,
) -> Result<(), super::diagnostics::PixelsError> {
    let fallback = |message| {
        super::diagnostics::PixelsError::Diagnostic(
            super::diagnostics::PixelsDiagnostic::from_prefixed(message, Span::default(), "P003"),
        )
    };
    let field = root_function(owner, programs, field).map_err(fallback)?;
    let material = root_function(owner, programs, material).map_err(fallback)?;
    let mut field_checker = Checker {
        programs,
        active: Vec::new(),
        kind: PixelsFnKind::Field,
        last_span: Span::default(),
    };
    if let Err(message) = field_checker.check_root(field) {
        return Err(super::diagnostics::PixelsError::Diagnostic(
            super::diagnostics::PixelsDiagnostic::from_prefixed(
                message,
                field_checker.last_span,
                "P003",
            ),
        ));
    }
    let mut material_checker = Checker {
        programs,
        active: Vec::new(),
        kind: PixelsFnKind::Material,
        last_span: Span::default(),
    };
    material_checker.check_root(material).map_err(|message| {
        super::diagnostics::PixelsError::Diagnostic(
            super::diagnostics::PixelsDiagnostic::from_prefixed(
                message,
                material_checker.last_span,
                "P003",
            ),
        )
    })
}

pub fn check_field_storage(programs: &BTreeMap<String, TypedProgram>) -> Result<(), String> {
    for (module, program) in programs {
        for (name, strukt) in &program.structs {
            for (field, ty) in &strukt.field_types {
                if is_field_type(ty, program) {
                    return Err(format!(
                        "P004: `Field` may not be stored in user struct `{module}::{name}.{field}`"
                    ));
                }
            }
        }
        for (name, enm) in &program.enums {
            for (variant_index, payload) in enm.variant_payload_types.iter().enumerate() {
                for (payload_index, ty) in payload.iter().enumerate() {
                    if is_field_type(ty, program) {
                        let variant = enm
                            .variants
                            .get(variant_index)
                            .map(String::as_str)
                            .unwrap_or("<unknown>");
                        return Err(format!(
                            "P004: `Field` may not be stored in enum payload `{module}::{name}.{variant}[{payload_index}]`"
                        ));
                    }
                }
            }
        }
        for (name, value) in &program.statics {
            if is_field_type(&value.ty, program) {
                return Err(format!(
                    "P004: `Field` may not be stored in static `{module}::{name}`"
                ));
            }
        }
        for (name, function) in &program.fns {
            if function.is_pub
                && is_field_type(&function.ret, program)
                && !program
                    .pixels_fns
                    .get(name)
                    .is_some_and(|meta| meta.kind == PixelsFnKind::Field)
                && !matches!(module.as_str(), "field" | "core.field")
            {
                return Err(format!(
                    "P004: public function `{module}::{name}` may not return opaque `Field`"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_type_search_looks_through_containers() {
        let mut program = TypedProgram::default();
        program
            .type_decl_modules
            .insert("F".to_string(), "core.field".to_string());
        program
            .type_decl_names
            .insert("F".to_string(), "Field".to_string());
        assert!(is_field_type(
            &Type::Array(
                Box::new(Type::Named("F".to_string(), vec![])),
                Box::new(crate::syntax::ast::Expr::Int(
                    Default::default(),
                    "2".to_string()
                ))
            ),
            &program
        ));
        assert!(!is_field_type(
            &Type::Named("Vec3".to_string(), vec![]),
            &program
        ));
    }

    #[test]
    fn field_storage_check_visits_enum_payloads() {
        let mut program = TypedProgram::default();
        program
            .type_decl_modules
            .insert("F".to_string(), "core.field".to_string());
        program
            .type_decl_names
            .insert("F".to_string(), "Field".to_string());
        program.enums.insert(
            "Holder".to_string(),
            crate::sema::typed::TypedEnum {
                variants: vec!["Stored".to_string()],
                variant_payload_types: vec![vec![Type::Named("F".to_string(), vec![])]],
                generic_type_params: Vec::new(),
                methods: BTreeMap::new(),
                assoc_fns: BTreeMap::new(),
            },
        );
        let programs = BTreeMap::from([("scene".to_string(), program)]);
        let error = check_field_storage(&programs).unwrap_err();
        assert!(error.contains("scene::Holder.Stored[0]"), "{error}");
    }

    #[test]
    fn literal_number_accepts_negative_floats() {
        let expr = TypedExpr {
            ty: Type::F32,
            span: Default::default(),
            kind: TypedExprKind::Neg(Box::new(TypedExpr {
                ty: Type::F32,
                span: Default::default(),
                kind: TypedExprKind::Float("0.5".to_string()),
            })),
        };
        assert_eq!(literal_number(&expr), Some(-0.5));
    }

    #[test]
    fn rigid_rotation_validation_rejects_shear_rows() {
        assert!(rotation_rows_are_orthonormal(&[
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
        ]));
        assert!(!rotation_rows_are_orthonormal(&[
            [1.0, 0.01, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ]));
    }

    #[test]
    fn constant_number_evaluates_arithmetic() {
        let literal = |text: &str| TypedExpr {
            ty: Type::F32,
            span: Default::default(),
            kind: TypedExprKind::Float(text.to_string()),
        };
        let expr = TypedExpr {
            ty: Type::F32,
            span: Default::default(),
            kind: TypedExprKind::Binary(
                crate::syntax::ast::BinOp::Add,
                Box::new(literal("1.25")),
                Box::new(literal("0.75")),
            ),
        };
        assert_eq!(
            constant_number(&BTreeMap::new(), "example", &expr),
            Some(2.0)
        );
    }

    #[test]
    fn constant_integer_resolves_named_array_indices_exactly() {
        let mut program = TypedProgram::default();
        program.consts.insert(
            "INDEX".to_string(),
            crate::sema::typed::TypedConst {
                ty: Type::Usize,
                value: TypedExpr {
                    ty: Type::Usize,
                    span: Default::default(),
                    kind: TypedExprKind::Int("1".to_string()),
                },
            },
        );
        let programs = BTreeMap::from([("test".to_string(), program)]);
        let index = TypedExpr {
            ty: Type::Usize,
            span: Default::default(),
            kind: TypedExprKind::Const("INDEX".to_string()),
        };
        assert_eq!(constant_integer(&programs, "test", &index), Some(1));
    }

    #[test]
    fn renderer_effect_kinds_fail_closed_with_a_call_chain() {
        fn expr(kind: TypedExprKind) -> TypedExpr {
            TypedExpr {
                ty: Type::Unit,
                span: Default::default(),
                kind,
            }
        }
        let programs = BTreeMap::new();
        let mut checker = Checker {
            programs: &programs,
            active: vec!["scene::world".to_string()],
            kind: PixelsFnKind::Field,
            last_span: Span::default(),
        };
        let mut env = BTreeMap::new();
        let unit = || expr(TypedExprKind::Unit);
        let actor_key = crate::sema::typed::CalleeKey::Fn("Actor.tick".to_string());

        let expressions = [
            expr(TypedExprKind::Static("MMIO".to_string())),
            expr(TypedExprKind::Str("text".to_string())),
            expr(TypedExprKind::BStr("bytes".to_string())),
            expr(TypedExprKind::Char("x".to_string())),
            expr(TypedExprKind::PoolName("heap".to_string())),
            expr(TypedExprKind::BitNot(Box::new(unit()))),
            expr(TypedExprKind::Try(Box::new(unit()), None)),
            expr(TypedExprKind::Intrinsic {
                key: "entropy".to_string(),
                receiver: None,
                type_arg: None,
                const_arg: Some(8),
                args: Vec::new(),
            }),
            expr(TypedExprKind::Intrinsic {
                key: "now".to_string(),
                receiver: None,
                type_arg: None,
                const_arg: None,
                args: Vec::new(),
            }),
            expr(TypedExprKind::Await(Box::new(unit()))),
            expr(TypedExprKind::Send(Box::new(unit()))),
            expr(TypedExprKind::GroupChild(actor_key.clone())),
            expr(TypedExprKind::Call {
                callee: actor_key,
                receiver: None,
                args: Vec::new(),
            }),
        ];
        for expression in expressions {
            let error = checker
                .check_expr(&expression, "scene", &mut env)
                .expect_err("renderer effect must fail closed");
            assert!(error.starts_with("P003:"), "{error}");
            assert!(error.contains("scene::world"), "{error}");
        }

        let static_target = expr(TypedExprKind::Static("MMIO".to_string()));
        let statements = [
            TypedStmt {
                span: Default::default(),
                kind: TypedStmtKind::Assign {
                    target: static_target,
                    value: unit(),
                },
            },
            TypedStmt {
                span: Default::default(),
                kind: TypedStmtKind::BareSend {
                    span: Default::default(),
                    expr: unit(),
                },
            },
            TypedStmt {
                span: Default::default(),
                kind: TypedStmtKind::WithGroup {
                    capacity: Some(unit()),
                    deadline: None,
                    as_name: Some("group".to_string()),
                    body: Vec::new(),
                },
            },
        ];
        for statement in statements {
            let error = checker
                .check_stmts(&[statement], "scene", BTreeMap::new())
                .expect_err("renderer effect statement must fail closed");
            assert!(error.starts_with("P003:"), "{error}");
            assert!(error.contains("scene::world"), "{error}");
        }
    }

    #[test]
    fn runtime_identity_tests_are_rejected_before_symbolic_evaluation() {
        use crate::sema::typed::TypedPattern;

        let programs = BTreeMap::new();
        let mut checker = Checker {
            programs: &programs,
            active: vec!["scene::shade".to_string()],
            kind: PixelsFnKind::Material,
            last_span: Span::default(),
        };
        let value = TypedExpr {
            ty: Type::Named("MaterialId".to_string(), vec![]),
            span: Default::default(),
            kind: TypedExprKind::Local("material".to_string()),
        };
        let pattern = TypedPattern {
            ty: value.ty.clone(),
            span: Default::default(),
            kind: TypedPatternKind::Wildcard,
        };
        let expression = TypedExpr {
            ty: Type::Bool,
            span: Default::default(),
            kind: TypedExprKind::Is(Box::new(value), Box::new(pattern)),
        };
        let mut env = BTreeMap::from([("material".to_string(), Dependency::Parameter)]);
        let error = checker
            .check_expr(&expression, "scene", &mut env)
            .expect_err("runtime identity tests must fail closed");
        assert!(error.starts_with("P003:"), "{error}");
        assert!(
            error.contains("explicit material identity match"),
            "{error}"
        );
    }

    #[test]
    fn unsupported_runtime_scalar_conversions_are_rejected_before_evaluation() {
        let programs = BTreeMap::new();
        let mut checker = Checker {
            programs: &programs,
            active: vec!["scene::shade".to_string()],
            kind: PixelsFnKind::Material,
            last_span: Span::default(),
        };
        let expression = TypedExpr {
            ty: Type::F64,
            span: Default::default(),
            kind: TypedExprKind::ToScalar(Box::new(TypedExpr {
                ty: Type::F32,
                span: Default::default(),
                kind: TypedExprKind::Local("value".to_string()),
            })),
        };
        let mut env = BTreeMap::from([("value".to_string(), Dependency::Parameter)]);
        let error = checker
            .check_expr(&expression, "scene", &mut env)
            .expect_err("unsupported runtime conversion must fail closed");
        assert!(error.starts_with("P003:"), "{error}");
        assert!(error.contains("runtime scalar conversion"), "{error}");
    }

    #[test]
    fn runtime_integer_operations_are_rejected_before_f32_lowering() {
        let programs = BTreeMap::new();
        let mut checker = Checker {
            programs: &programs,
            active: vec!["scene::world".to_string()],
            kind: PixelsFnKind::Field,
            last_span: Span::default(),
        };
        let operand = |kind| TypedExpr {
            ty: Type::U8,
            span: Default::default(),
            kind,
        };
        let expression = TypedExpr {
            ty: Type::U8,
            span: Default::default(),
            kind: TypedExprKind::Binary(
                crate::syntax::ast::BinOp::AddW,
                Box::new(operand(TypedExprKind::Local("value".to_string()))),
                Box::new(operand(TypedExprKind::Int("10".to_string()))),
            ),
        };
        let mut env = BTreeMap::from([("value".to_string(), Dependency::Parameter)]);
        let error = checker
            .check_expr(&expression, "scene", &mut env)
            .expect_err("runtime wrapping arithmetic must fail closed");
        assert!(error.starts_with("P003:"), "{error}");
        assert!(error.contains("runtime integer arithmetic"), "{error}");
    }

    #[test]
    fn runtime_f64_predicates_are_rejected_before_symbolic_evaluation() {
        let programs = BTreeMap::new();
        let mut checker = Checker {
            programs: &programs,
            active: vec!["scene::shade".to_string()],
            kind: PixelsFnKind::Material,
            last_span: Span::default(),
        };
        let operand = |kind| TypedExpr {
            ty: Type::F64,
            span: Default::default(),
            kind,
        };
        let expression = TypedExpr {
            ty: Type::Bool,
            span: Default::default(),
            kind: TypedExprKind::Binary(
                crate::syntax::ast::BinOp::Gt,
                Box::new(operand(TypedExprKind::Local("phase".to_string()))),
                Box::new(operand(TypedExprKind::Float("0.0".to_string()))),
            ),
        };
        let mut env = BTreeMap::from([("phase".to_string(), Dependency::Parameter)]);
        let error = checker
            .check_expr(&expression, "scene", &mut env)
            .expect_err("runtime f64 predicates must fail closed");
        assert!(error.starts_with("P003:"), "{error}");
        assert!(error.contains("runtime f64 arithmetic"), "{error}");
    }

    #[test]
    fn primitive_points_must_be_coordinate_derived_before_evaluation() {
        let programs = BTreeMap::new();
        let checker = Checker {
            programs: &programs,
            active: vec!["scene::world".to_string()],
            kind: PixelsFnKind::Field,
            last_span: Span::default(),
        };
        let error = checker
            .validate_field_call("scene", "sphere", &[], &[Dependency::Parameter])
            .unwrap_err();
        assert!(error.starts_with("P004:"), "{error}");
        assert!(
            error.contains("derived from the renderer coordinate"),
            "{error}"
        );
    }

    #[test]
    fn coordinate_vector_must_be_the_vec3_method_receiver() {
        use crate::sema::typed::{CalleeKey, TypedFn, TypedParam, TypedStruct};
        use crate::syntax::ast::AccessMode;

        let vec3 = Type::Named("Vec3".to_string(), vec![]);
        let method = TypedFn {
            receiver: Some((AccessMode::Read, vec3.clone())),
            params: vec![TypedParam {
                mode: AccessMode::Read,
                name: "right".to_string(),
                ty: vec3.clone(),
                default: None,
            }],
            ret: vec3.clone(),
            body: Vec::new(),
            is_async: false,
            is_task: false,
            is_layout_assert: false,
            is_pub: false,
        };
        let mut core = TypedProgram {
            module_path: "core.field".to_string(),
            ..Default::default()
        };
        core.structs.insert(
            "Vec3".to_string(),
            TypedStruct {
                name: "Vec3".to_string(),
                methods: BTreeMap::from([("add".to_string(), method)]),
                ..Default::default()
            },
        );
        let mut scene = TypedProgram {
            module_path: "scene".to_string(),
            ..Default::default()
        };
        scene
            .type_decl_modules
            .insert("Vec3".to_string(), "core.field".to_string());
        scene
            .type_decl_names
            .insert("Vec3".to_string(), "Vec3".to_string());
        let programs = BTreeMap::from([
            ("core.field".to_string(), core),
            ("scene".to_string(), scene),
        ]);
        let mut checker = Checker {
            programs: &programs,
            active: vec!["scene::world".to_string()],
            kind: PixelsFnKind::Field,
            last_span: Span::default(),
        };
        let local = |name: &str| TypedExpr {
            ty: vec3.clone(),
            span: Default::default(),
            kind: TypedExprKind::Local(name.to_string()),
        };
        let expression = TypedExpr {
            ty: vec3.clone(),
            span: Default::default(),
            kind: TypedExprKind::Call {
                callee: CalleeKey::Fn("Vec3.add".to_string()),
                receiver: Some(Box::new(local("offset"))),
                args: vec![TypedCallArg {
                    mode: AccessMode::Read,
                    value: Some(local("p")),
                }],
            },
        };
        let mut env = BTreeMap::from([
            ("offset".to_string(), Dependency::Parameter),
            ("p".to_string(), Dependency::Coordinate),
        ]);
        let error = checker
            .check_expr(&expression, "scene", &mut env)
            .expect_err("coordinate vector argument must fail closed");
        assert!(error.starts_with("P004:"), "{error}");
        assert!(
            error.contains("coordinate vector as the receiver"),
            "{error}"
        );
    }

    #[test]
    fn omitted_helper_defaults_inherit_earlier_argument_dependencies() {
        use crate::sema::typed::{CalleeKey, TypedFn, TypedParam};
        use crate::syntax::ast::AccessMode;

        let vec3 = Type::Named("Vec3".to_string(), vec![]);
        let local = |name: &str| TypedExpr {
            ty: vec3.clone(),
            span: Default::default(),
            kind: TypedExprKind::Local(name.to_string()),
        };
        let helper = TypedFn {
            receiver: None,
            params: vec![
                TypedParam {
                    mode: AccessMode::Read,
                    name: "first".to_string(),
                    ty: vec3.clone(),
                    default: None,
                },
                TypedParam {
                    mode: AccessMode::Read,
                    name: "second".to_string(),
                    ty: vec3.clone(),
                    default: Some(local("first")),
                },
            ],
            ret: vec3.clone(),
            body: vec![TypedStmt {
                span: Default::default(),
                kind: TypedStmtKind::Return(Some(local("second"))),
            }],
            is_async: false,
            is_task: false,
            is_layout_assert: false,
            is_pub: false,
        };
        let mut program = TypedProgram {
            module_path: "scene".to_string(),
            ..Default::default()
        };
        program.fns.insert("choose".to_string(), helper);
        let programs = BTreeMap::from([("scene".to_string(), program)]);
        let mut checker = Checker {
            programs: &programs,
            active: vec!["scene::world".to_string()],
            kind: PixelsFnKind::Field,
            last_span: Span::default(),
        };
        let call = TypedExpr {
            ty: vec3.clone(),
            span: Span {
                line: 12,
                col: 9,
                ..Span::default()
            },
            kind: TypedExprKind::Call {
                callee: CalleeKey::Fn("choose".to_string()),
                receiver: None,
                args: vec![
                    TypedCallArg {
                        mode: AccessMode::Read,
                        value: Some(local("p")),
                    },
                    TypedCallArg {
                        mode: AccessMode::Read,
                        value: None,
                    },
                ],
            },
        };
        let mut env = BTreeMap::from([("p".to_string(), Dependency::Coordinate)]);
        assert_eq!(
            checker.check_expr(&call, "scene", &mut env).unwrap(),
            Dependency::Coordinate
        );
    }

    #[test]
    fn coordinate_dependent_field_branch_is_rejected_before_evaluation() {
        let programs = BTreeMap::new();
        let mut checker = Checker {
            programs: &programs,
            active: vec!["scene::world".to_string()],
            kind: PixelsFnKind::Field,
            last_span: Span::default(),
        };
        let coordinate = TypedExpr {
            ty: Type::Named("Vec3".to_string(), Vec::new()),
            span: Default::default(),
            kind: TypedExprKind::Local("p".to_string()),
        };
        let x = TypedExpr {
            ty: Type::F32,
            span: Default::default(),
            kind: TypedExprKind::Field(Box::new(coordinate), "x".to_string()),
        };
        let zero = TypedExpr {
            ty: Type::F32,
            span: Default::default(),
            kind: TypedExprKind::Float("0.0".to_string()),
        };
        let condition = TypedExpr {
            ty: Type::Bool,
            span: Span {
                line: 9,
                col: 8,
                ..Span::default()
            },
            kind: TypedExprKind::Binary(crate::syntax::ast::BinOp::Gt, Box::new(x), Box::new(zero)),
        };
        let statement = TypedStmt {
            span: condition.span,
            kind: TypedStmtKind::If {
                cond: condition,
                then_branch: vec![TypedStmt {
                    span: Default::default(),
                    kind: TypedStmtKind::Pass,
                }],
                elifs: Vec::new(),
                else_branch: None,
            },
        };
        let env = BTreeMap::from([("p".to_string(), Dependency::Coordinate)]);
        let error = checker
            .check_stmts(&[statement], "scene", env)
            .expect_err("coordinate-dependent field branch must fail closed");
        assert!(
            error.contains("runtime control flow that changes field topology"),
            "{error}"
        );
        assert_eq!(checker.last_span.line, 9);
        assert_eq!(checker.last_span.col, 8);
    }

    #[test]
    fn coordinate_dependent_primitive_coefficients_fail_before_symbolic_lowering() {
        let programs = BTreeMap::new();
        let checker = Checker {
            programs: &programs,
            active: vec!["scene::world".to_string()],
            kind: PixelsFnKind::Field,
            last_span: Span::default(),
        };
        let error = checker
            .validate_field_call(
                "scene",
                "sphere",
                &[],
                &[
                    Dependency::Coordinate,
                    Dependency::Comptime,
                    Dependency::CoordinateAndParameter,
                ],
            )
            .unwrap_err();
        assert!(error.contains("coefficient argument 3"), "{error}");
        assert!(error.contains("not on the renderer coordinate"), "{error}");
    }
}
