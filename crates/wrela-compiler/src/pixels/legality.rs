//! Finite-body legality checks for `@field` and `@material` roots.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::typed::{
    PixelsFnKind, TypedCallArg, TypedDeferBody, TypedExpr, TypedExprKind, TypedForIter,
    TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::sema::types::{Type, TypeArg};

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
                if active.contains(name) {
                    return None;
                }
                active.push(name.clone());
                let program = programs.get(module)?;
                let value = program
                    .consts
                    .get(name)
                    .or_else(|| program.imported.consts.get(name))?;
                let result = evaluate(programs, module, &value.value, active);
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
                if active.contains(name) {
                    return None;
                }
                active.push(name.clone());
                let program = programs.get(module)?;
                let value = program
                    .consts
                    .get(name)
                    .or_else(|| program.imported.consts.get(name))?;
                let result = evaluate(programs, module, &value.value, active);
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

    fn args_dependency(
        &mut self,
        args: &[TypedCallArg],
        module: &str,
        env: &mut BTreeMap<String, Dependency>,
    ) -> Result<Vec<Dependency>, String> {
        args.iter()
            .map(|arg| match &arg.value {
                Some(value) => self.check_expr(value, module, env),
                None => Ok(Dependency::Comptime),
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
        if matches!(name, "rotate" | "rigid_transform") {
            let row_start = if name == "rotate" { 1 } else { 2 };
            let mut rows = [[0.0; 3]; 3];
            for (row, argument) in rows.iter_mut().zip(&args[row_start..]) {
                let Some(value) = argument.value.as_ref() else {
                    return Err(self.reject(format!(
                        "P004: field operation `{name}` requires a compile-time rigid rotation"
                    )));
                };
                let TypedExprKind::StructLiteral { fields, .. } = &value.kind else {
                    return Err(self.reject(format!(
                        "P004: field operation `{name}` requires compile-time `Vec3` rotation rows"
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
                    *component = self.constant_number(module, value).ok_or_else(|| {
                        self.reject(format!(
                            "P004: field operation `{name}` rotation rows may not depend on renderer parameters"
                        ))
                    })?;
                }
            }
            // Rotation coefficients are f32 source values evaluated as f64 by
            // the checker. Permit only the accumulated rounding error of the
            // three f32 products in each dot product; a wider geometric
            // tolerance would admit actual nonuniform scale.
            const ORTHONORMAL_TOLERANCE: f64 = f32::EPSILON as f64 * 8.0;
            let dot = |left: &[f64; 3], right: &[f64; 3]| {
                left.iter()
                    .zip(right)
                    .map(|(left, right)| left * right)
                    .sum::<f64>()
            };
            let orthonormal = rows
                .iter()
                .all(|row| (dot(row, row) - 1.0).abs() <= ORTHONORMAL_TOLERANCE)
                && (0..3).all(|left| {
                    ((left + 1)..3)
                        .all(|right| dot(&rows[left], &rows[right]).abs() <= ORTHONORMAL_TOLERANCE)
                });
            if !orthonormal {
                return Err(self.reject(format!(
                    "P004: field operation `{name}` rotation rows are not orthonormal \
                     within f32 roundoff"
                )));
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
            TypedExprKind::Const(name) => self
                .programs
                .get(module)
                .and_then(|program| {
                    program
                        .consts
                        .get(name)
                        .or_else(|| program.imported.consts.get(name))
                })
                .is_some_and(|value| self.enum_constant(module, &value.value)),
            _ => false,
        }
    }

    fn constant_number(&self, module: &str, expr: &TypedExpr) -> Option<f64> {
        constant_number(self.programs, module, expr)
    }

    fn check_expr(
        &mut self,
        expr: &TypedExpr,
        module: &str,
        env: &mut BTreeMap<String, Dependency>,
    ) -> Result<Dependency, String> {
        match &expr.kind {
            TypedExprKind::Int(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::Str(_)
            | TypedExprKind::BStr(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Unit
            | TypedExprKind::Const(_)
            | TypedExprKind::PoolName(_) => Ok(Dependency::Comptime),
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
                        self.check_expr(default, default_module, &mut BTreeMap::new())?
                    } else {
                        Dependency::Comptime
                    };
                    arg_dependencies.push(argument_dependency);
                }
                for arg in &arg_dependencies {
                    dependency = dependency.combine(*arg);
                }
                if let Some(located) = resolved
                    .as_ref()
                    .filter(|located| super::is_core_field_function(located))
                {
                    self.validate_field_call(module, &located.decl_name, args, &arg_dependencies)?;
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
            TypedExprKind::Field(base, _)
            | TypedExprKind::ToScalar(base)
            | TypedExprKind::Neg(base)
            | TypedExprKind::BitNot(base)
            | TypedExprKind::Take(base)
            | TypedExprKind::Not(base) => self.check_expr(base, module, env),
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
                Ok(dependency)
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
            TypedExprKind::Binary(_, base, index)
            | TypedExprKind::And(base, index)
            | TypedExprKind::Or(base, index) => {
                let a = self.check_expr(base, module, env)?;
                let b = self.check_expr(index, module, env)?;
                Ok(a.combine(b))
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
            TypedExprKind::Is(value, _) => self.check_expr(value, module, env),
            TypedExprKind::EnumConstruct { args, .. } => {
                let mut dependency = Dependency::Comptime;
                for arg in self.args_dependency(args, module, env)? {
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
                        if let Some(guard) = &arm.guard {
                            let guard_dependency = self.check_expr(guard, module, &mut env)?;
                            if self.kind == PixelsFnKind::Field
                                && guard_dependency != Dependency::Comptime
                            {
                                return Err(self
                                    .reject("P003: `@field` runtime match guards are forbidden"));
                            }
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
                TypedStmtKind::Break | TypedStmtKind::Continue => {}
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
) -> Result<(), String> {
    let field = root_function(owner, programs, field)?;
    let material = root_function(owner, programs, material)?;
    Checker {
        programs,
        active: Vec::new(),
        kind: PixelsFnKind::Field,
    }
    .check_root(field)?;
    Checker {
        programs,
        active: Vec::new(),
        kind: PixelsFnKind::Material,
    }
    .check_root(material)
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
        };
        let mut env = BTreeMap::new();
        let unit = || expr(TypedExprKind::Unit);
        let actor_key = crate::sema::typed::CalleeKey::Fn("Actor.tick".to_string());

        let expressions = [
            expr(TypedExprKind::Static("MMIO".to_string())),
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
}
