//! Stable renderer-parameter path extraction for P1.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::attrs::{NumericRange, RateContract};
use crate::sema::typed::{
    TypedCallArg, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn,
    TypedForIter, TypedProgram, TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::Type;

use super::{LocatedFn, call_base, called_function, root_function};

#[derive(Debug, Clone, PartialEq)]
pub struct ParameterContract {
    pub path: Vec<usize>,
    pub ty: Type,
    pub range: NumericRange,
    pub rate: Option<RateContract>,
}

#[derive(Debug, Clone, PartialEq)]
struct Origin {
    path: Vec<usize>,
    /// Location of this source leaf inside a newly constructed aggregate.
    /// Projections consume components from the front without changing the
    /// canonical source `path`.
    projection: Vec<usize>,
    ty: Type,
    module: String,
    unmodified: bool,
    contracts: crate::sema::attrs::FieldContracts,
    exact_comptime_integer: Option<i128>,
}

struct Collector<'a> {
    programs: &'a BTreeMap<String, TypedProgram>,
    active: BTreeSet<String>,
    found: BTreeMap<Vec<usize>, ParameterContract>,
}

fn is_optional_integer_range(ty: &Type) -> bool {
    matches!(
        ty,
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
}

fn requires_parameter_range(ty: &Type) -> bool {
    matches!(ty, Type::F32 | Type::F64)
}

impl<'a> Collector<'a> {
    fn is_field_nominal(&self, origin: &Origin, declarations: &[&str]) -> bool {
        let Type::Named(name, args) = &origin.ty else {
            return false;
        };
        args.is_empty()
            && super::program_for_decl_module(self.programs, &origin.module).is_some_and(
                |program| {
                    super::nominal_decl(program, name).is_some_and(|(module, declaration)| {
                        matches!(module, "field" | "core.field")
                            && declarations.contains(&declaration)
                    })
                },
            )
    }

    fn requires_parameter_range(&self, origin: &Origin) -> bool {
        requires_parameter_range(&origin.ty)
            || self.is_field_nominal(origin, &["Vec2", "Vec3", "Vec4", "Rgb"])
    }

    fn merge_origins(groups: impl IntoIterator<Item = Vec<Origin>>) -> Vec<Origin> {
        let mut merged = Vec::new();
        for group in groups {
            for origin in group {
                if !merged.contains(&origin) {
                    merged.push(origin);
                }
            }
        }
        merged
    }

    fn record_origins(&mut self, origins: &[Origin]) -> Result<(), String> {
        for origin in origins {
            self.record(origin)?;
        }
        Ok(())
    }

    fn merge_envs(
        base: &BTreeMap<String, Vec<Origin>>,
        branches: impl IntoIterator<Item = BTreeMap<String, Vec<Origin>>>,
    ) -> BTreeMap<String, Vec<Origin>> {
        let mut merged = base.clone();
        for branch in branches {
            for (name, origins) in branch {
                let entry = merged.entry(name).or_default();
                for origin in origins {
                    if !entry.contains(&origin) {
                        entry.push(origin);
                    }
                }
            }
        }
        merged
    }

    fn find_struct<'b>(&'b self, origin: &Origin) -> Option<(&'b TypedStruct, String)> {
        let Type::Named(name, args) = &origin.ty else {
            return None;
        };
        let context = super::program_for_decl_module(self.programs, &origin.module)?;
        let (declaring_module, declaring_name) = super::nominal_decl(context, name)?;
        let declaration = super::program_for_decl_module(self.programs, declaring_module)?;
        if args.is_empty() {
            return Some((
                declaration.structs.get(declaring_name)?,
                declaring_module.to_string(),
            ));
        }
        super::instantiated_struct(context, name, args)
            .map(|strukt| (strukt, origin.module.clone()))
            .or_else(|| {
                super::instantiated_struct(declaration, declaring_name, args)
                    .map(|strukt| (strukt, declaring_module.to_string()))
            })
    }

    fn field_origin(&self, base: &Origin, field: &str) -> Option<Origin> {
        if self.is_field_nominal(base, &["Vec2", "Vec3", "Vec4", "Rgb"]) {
            return Some(base.clone());
        }
        let (strukt, declaring_module) = self.find_struct(base)?;
        let index = strukt.fields.iter().position(|name| name == field)?;
        let ty = strukt.field_types.get(field)?.clone();
        let contracts = strukt
            .field_contracts
            .get([index].as_slice())
            .cloned()
            .unwrap_or_default();
        let mut path = base.path.clone();
        path.push(index);
        Some(Origin {
            path,
            projection: Vec::new(),
            ty,
            module: declaring_module,
            unmodified: base.unmodified,
            contracts,
            exact_comptime_integer: None,
        })
    }

    fn field_index(&self, module: &str, ty: &Type, field: &str) -> Option<usize> {
        let aggregate = Origin {
            path: Vec::new(),
            projection: Vec::new(),
            ty: ty.clone(),
            module: module.to_string(),
            unmodified: true,
            contracts: Default::default(),
            exact_comptime_integer: None,
        };
        let (strukt, _) = self.find_struct(&aggregate)?;
        strukt.fields.iter().position(|name| name == field)
    }

    fn projected_origin(mut origin: Origin, index: usize) -> Option<Origin> {
        if origin.projection.first().copied()? != index {
            return None;
        }
        origin.projection.remove(0);
        Some(origin)
    }

    fn nest_origin(mut origin: Origin, index: usize) -> Origin {
        origin.projection.insert(0, index);
        origin
    }

    fn record(&mut self, origin: &Origin) -> Result<(), String> {
        if origin.exact_comptime_integer.is_some() {
            return Ok(());
        }
        let required = self.requires_parameter_range(origin);
        if !required && !is_optional_integer_range(&origin.ty) {
            return Ok(());
        }
        let Some(range) = origin.contracts.range else {
            if !required {
                return Ok(());
            }
            return Err(format!(
                "P005: parameter path {:?} influences rendering but has no `@range` (type `{}`)",
                origin.path,
                crate::sema::types::render_type(&origin.ty)
            ));
        };
        self.found
            .entry(origin.path.clone())
            .or_insert_with(|| ParameterContract {
                path: origin.path.clone(),
                ty: origin.ty.clone(),
                range,
                rate: origin.contracts.rate,
            });
        Ok(())
    }

    fn visit_args(
        &mut self,
        args: &[TypedCallArg],
        module: &str,
        env: &mut BTreeMap<String, Vec<Origin>>,
    ) -> Result<Vec<Vec<Origin>>, String> {
        args.iter()
            .map(|arg| match &arg.value {
                Some(value) => self.visit_expr(value, module, env),
                None => Ok(Vec::new()),
            })
            .collect()
    }

    fn traced_integer(
        &self,
        expr: &TypedExpr,
        module: &str,
        env: &BTreeMap<String, Vec<Origin>>,
    ) -> Option<i128> {
        if let Some(value) = super::legality::constant_integer(self.programs, module, expr) {
            return Some(value);
        }
        match &expr.kind {
            TypedExprKind::Local(name) => {
                let origins = env.get(name)?;
                (origins.len() == 1).then(|| origins[0].exact_comptime_integer)?
            }
            TypedExprKind::ToScalar(inner) => self.traced_integer(inner, module, env),
            TypedExprKind::Neg(inner) => self.traced_integer(inner, module, env)?.checked_neg(),
            TypedExprKind::Binary(operator, left, right) => {
                let left = self.traced_integer(left, module, env)?;
                let right = self.traced_integer(right, module, env)?;
                use crate::syntax::ast::BinOp;
                match operator {
                    BinOp::Add => left.checked_add(right),
                    BinOp::Sub => left.checked_sub(right),
                    BinOp::Mul => left.checked_mul(right),
                    BinOp::Div => left.checked_div(right),
                    BinOp::Rem => left.checked_rem(right),
                    BinOp::BitAnd => Some(left & right),
                    BinOp::BitOr => Some(left | right),
                    BinOp::BitXor => Some(left ^ right),
                    BinOp::Shl => u32::try_from(right)
                        .ok()
                        .and_then(|shift| left.checked_shl(shift)),
                    BinOp::Shr => u32::try_from(right)
                        .ok()
                        .and_then(|shift| left.checked_shr(shift)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn visit_expr(
        &mut self,
        expr: &TypedExpr,
        module: &str,
        env: &mut BTreeMap<String, Vec<Origin>>,
    ) -> Result<Vec<Origin>, String> {
        match &expr.kind {
            TypedExprKind::Local(name) => Ok(env.get(name).cloned().unwrap_or_default()),
            TypedExprKind::Field(base, field) => {
                let mut origins = Vec::new();
                let field_index = self.field_index(module, &base.ty, field);
                for base_origin in self.visit_expr(base, module, env)? {
                    let selected = if base_origin.projection.is_empty() {
                        self.field_origin(&base_origin, field)
                    } else {
                        field_index.and_then(|index| Self::projected_origin(base_origin, index))
                    };
                    if let Some(origin) = selected {
                        if !origins.contains(&origin) {
                            origins.push(origin);
                        }
                    }
                }
                Ok(origins)
            }
            TypedExprKind::Index(base, index) => {
                let bases = self.visit_expr(base, module, env)?;
                let index_origins = self.visit_expr(index, module, env)?;
                let constant_index =
                    super::legality::constant_integer(self.programs, module, index)
                        .or_else(|| self.traced_integer(index, module, env))
                        .and_then(|index| usize::try_from(index).ok());
                let extent = match &base.ty {
                    Type::Array(_, length) => crate::sema::bodies::literal_array_len(length)
                        .and_then(|length| usize::try_from(length).ok()),
                    _ => None,
                };
                if constant_index.is_none() {
                    let Some(extent) = extent else {
                        return Err(
                            "P005: renderer dynamic index has no comptime array extent".to_string()
                        );
                    };
                    if index_origins.is_empty() {
                        return Err(
                            "P005: renderer dynamic array index must be a direct parameter with an \
                             exact in-bounds integer `@range`"
                                .to_string(),
                        );
                    }
                    for origin in &index_origins {
                        let exact_range =
                            origin.contracts.range.and_then(|range| range.exact_integer);
                        let in_bounds = origin.unmodified
                            && exact_range.is_some_and(|(min, max)| {
                                min >= 0
                                    && usize::try_from(max).is_ok_and(|maximum| maximum < extent)
                            });
                        if !in_bounds {
                            return Err(format!(
                                "P005: dynamic array index parameter path {:?} must have an exact \
                                 integer `@range` wholly within [0, {})",
                                origin.path, extent
                            ));
                        }
                    }
                    self.record_origins(&index_origins)?;
                }
                let mut origins = Vec::new();
                for origin in bases {
                    if !origin.projection.is_empty() {
                        let indices: Vec<usize> = match constant_index {
                            Some(index) => vec![index],
                            None => origin.projection.first().copied().into_iter().collect(),
                        };
                        for index in indices {
                            if let Some(indexed) = Self::projected_origin(origin.clone(), index)
                                && !origins.contains(&indexed)
                            {
                                origins.push(indexed);
                            }
                        }
                        continue;
                    }
                    let Type::Array(element, length) = &origin.ty else {
                        continue;
                    };
                    let length = crate::sema::bodies::literal_array_len(length)
                        .and_then(|length| usize::try_from(length).ok())
                        .ok_or_else(|| {
                            "P005: renderer parameter array index cannot be resolved \
                             conservatively because its extent is not a nonnegative comptime \
                             integer"
                                .to_string()
                        })?;
                    let indices = match constant_index {
                        Some(index) if index < length => index..index + 1,
                        Some(index) => {
                            return Err(format!(
                                "P005: renderer parameter array index {index} is outside its \
                                 comptime extent {length}"
                            ));
                        }
                        None => 0..length,
                    };
                    for index in indices {
                        let mut indexed = origin.clone();
                        indexed.path.push(index);
                        indexed.ty = (**element).clone();
                        indexed.contracts = Default::default();
                        if !origins.contains(&indexed) {
                            origins.push(indexed);
                        }
                    }
                }
                Ok(origins)
            }
            TypedExprKind::Call {
                callee,
                receiver,
                args,
            } => {
                let receiver_origins = match receiver {
                    Some(receiver) => self.visit_expr(receiver, module, env)?,
                    None => Vec::new(),
                };
                let origins = self.visit_args(args, module, env)?;
                let spelling = callee.spelling();
                let name = call_base(&spelling);
                let positive_contract = |index: usize,
                                         label: &str,
                                         operation: &str|
                 -> Result<(), String> {
                    let Some(argument) =
                        args.get(index).and_then(|argument| argument.value.as_ref())
                    else {
                        return Ok(());
                    };
                    if super::legality::constant_number(self.programs, module, argument).is_some() {
                        return Ok(());
                    }
                    let Some(argument_origins) =
                        origins.get(index).filter(|origins| !origins.is_empty())
                    else {
                        return Err(format!(
                            "P004: `{operation}` `{label}` must be a positive comptime constant or a \
                             direct renderer parameter with a strictly positive `@range`"
                        ));
                    };
                    for origin in argument_origins {
                        if !origin.unmodified {
                            return Err(format!(
                                "P004: `{operation}` `{label}` must be a positive comptime constant or \
                                 direct renderer parameter with a strictly positive `@range`; \
                                 transformed parameter expressions are not admitted"
                            ));
                        }
                        let Some(range) = origin.contracts.range else {
                            return Err(format!(
                                "P005: parameter path {:?} influences rendering but has no `@range`",
                                origin.path
                            ));
                        };
                        if range.min <= 0.0 {
                            let code = if matches!(
                                operation,
                                "smooth_union" | "smooth_intersection" | "smooth_subtract"
                            ) {
                                "P011"
                            } else if matches!(
                                operation,
                                "finite_repeat_x" | "finite_repeat_y" | "finite_repeat_z"
                            ) {
                                "P012"
                            } else {
                                "P004"
                            };
                            return Err(format!(
                                "{code}: `{operation}` `{label}` range minimum {} must be strictly positive",
                                range.min
                            ));
                        }
                    }
                    Ok(())
                };
                let resolved = called_function(self.programs, module, &spelling);
                let canonical_name = resolved
                    .as_ref()
                    .filter(|located| super::is_core_field_function(located))
                    .map(|located| located.decl_name.as_str())
                    .unwrap_or(name);
                if matches!(
                    canonical_name,
                    "finite_repeat_x" | "finite_repeat_y" | "finite_repeat_z"
                ) {
                    positive_contract(3, "period", canonical_name)?;
                }
                if matches!(
                    canonical_name,
                    "smooth_union" | "smooth_intersection" | "smooth_subtract"
                ) {
                    positive_contract(2, "k", canonical_name)?;
                }
                if canonical_name == "uniform_scale" {
                    positive_contract(1, "scale", canonical_name)?;
                }
                if resolved.as_ref().is_some_and(super::is_core_field_function) {
                    self.record_origins(&receiver_origins)?;
                    for argument_origins in &origins {
                        self.record_origins(argument_origins)?;
                    }
                    return Ok(Vec::new());
                }
                if super::is_core_material_constructor(self.programs, module, &spelling) {
                    self.record_origins(&receiver_origins)?;
                    for argument_origins in &origins {
                        self.record_origins(argument_origins)?;
                    }
                    return Ok(Vec::new());
                }
                if resolved
                    .as_ref()
                    .is_some_and(super::is_core_field_vector_method)
                {
                    let mut origins =
                        Self::merge_origins(std::iter::once(receiver_origins).chain(origins));
                    for origin in &mut origins {
                        origin.unmodified = false;
                    }
                    return Ok(origins);
                }
                if let Some(function) = resolved {
                    let mut call_env: BTreeMap<String, Vec<Origin>> = function
                        .function
                        .params
                        .iter()
                        .zip(origins)
                        .map(|(param, origin)| (param.name.clone(), origin))
                        .collect();
                    if function.function.receiver.is_some() {
                        call_env.insert("self".to_string(), receiver_origins);
                    }
                    return self.visit_function(function, call_env);
                }
                Ok(Vec::new())
            }
            TypedExprKind::CallValue(callee, args) => {
                self.visit_expr(callee, module, env)?;
                self.visit_args(args, module, env)?;
                Ok(Vec::new())
            }
            TypedExprKind::Take(value)
            | TypedExprKind::Try(value, _)
            | TypedExprKind::Panic(value)
            | TypedExprKind::Await(value)
            | TypedExprKind::Send(value) => self.visit_expr(value, module, env),
            TypedExprKind::ToScalar(value)
            | TypedExprKind::Neg(value)
            | TypedExprKind::BitNot(value)
            | TypedExprKind::Not(value) => {
                let mut origins = self.visit_expr(value, module, env)?;
                for origin in &mut origins {
                    origin.unmodified = false;
                }
                Ok(origins)
            }
            TypedExprKind::Binary(_, left, right)
            | TypedExprKind::OpCall(_, left, right)
            | TypedExprKind::And(left, right)
            | TypedExprKind::Or(left, right) => {
                let left = self.visit_expr(left, module, env)?;
                let right = self.visit_expr(right, module, env)?;
                let mut origins = Self::merge_origins([left, right]);
                for origin in &mut origins {
                    origin.unmodified = false;
                }
                Ok(origins)
            }
            TypedExprKind::Is(value, _) => self.visit_expr(value, module, env),
            TypedExprKind::EnumConstruct { args, .. } => {
                let origins = self.visit_args(args, module, env)?;
                Ok(Self::merge_origins(origins))
            }
            TypedExprKind::Closure { body, .. } => {
                match body {
                    TypedClosureBody::Expr(value) => {
                        self.visit_expr(value, module, env)?;
                    }
                    TypedClosureBody::Suite(body) => {
                        self.visit_stmts(body, module, env.clone())?;
                    }
                }
                Ok(Vec::new())
            }
            TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
                let mut origins = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    origins.push(
                        self.visit_expr(item, module, env)?
                            .into_iter()
                            .map(|origin| Self::nest_origin(origin, index))
                            .collect(),
                    );
                }
                Ok(Self::merge_origins(origins))
            }
            TypedExprKind::StructLiteral { fields, .. } => {
                let mut origins = Vec::new();
                for (field, value) in fields {
                    let Some(index) = self.field_index(module, &expr.ty, field) else {
                        return Err(format!(
                            "P005: cannot resolve aggregate field `{field}` while tracing \
                             renderer parameters"
                        ));
                    };
                    origins.push(
                        self.visit_expr(value, module, env)?
                            .into_iter()
                            .map(|origin| Self::nest_origin(origin, index))
                            .collect(),
                    );
                }
                Ok(Self::merge_origins(origins))
            }
            TypedExprKind::Intrinsic { receiver, args, .. } => {
                let mut origins = Vec::new();
                if let Some(receiver) = receiver {
                    origins.push(self.visit_expr(receiver, module, env)?);
                }
                for (_, value) in args {
                    origins.push(self.visit_expr(value, module, env)?);
                }
                Ok(Self::merge_origins(origins))
            }
            TypedExprKind::Int(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::Str(_)
            | TypedExprKind::BStr(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Unit
            | TypedExprKind::Const(_)
            | TypedExprKind::Static(_)
            | TypedExprKind::FnRef(_)
            | TypedExprKind::PoolName(_)
            | TypedExprKind::GroupChild(_) => Ok(Vec::new()),
        }
    }

    fn visit_function(
        &mut self,
        located: LocatedFn,
        env: BTreeMap<String, Vec<Origin>>,
    ) -> Result<Vec<Origin>, String> {
        let identity = format!("{}::{}", located.module, located.key);
        if !self.active.insert(identity.clone()) {
            return Ok(Vec::new());
        }
        let result = self
            .visit_stmts(&located.function.body, &located.module, env)
            .map(|(returns, _)| returns);
        self.active.remove(&identity);
        result
    }

    fn visit_stmts(
        &mut self,
        stmts: &[TypedStmt],
        module: &str,
        mut env: BTreeMap<String, Vec<Origin>>,
    ) -> Result<(Vec<Origin>, BTreeMap<String, Vec<Origin>>), String> {
        let mut returns = Vec::new();
        for stmt in stmts {
            match &stmt.kind {
                TypedStmtKind::Let { name, value, .. } => {
                    let origin = self.visit_expr(value, module, &mut env)?;
                    env.insert(name.clone(), origin);
                }
                TypedStmtKind::Assign { target, value } => {
                    let origin = self.visit_expr(value, module, &mut env)?;
                    if let TypedExprKind::Local(name) = &target.kind {
                        env.insert(name.clone(), origin);
                    } else {
                        self.visit_expr(target, module, &mut env)?;
                    }
                }
                TypedStmtKind::If {
                    cond,
                    then_branch,
                    elifs,
                    else_branch,
                } => {
                    let cond_origins = self.visit_expr(cond, module, &mut env)?;
                    self.record_origins(&cond_origins)?;
                    let mut branch_envs = Vec::new();
                    let (branch_returns, branch_env) =
                        self.visit_stmts(then_branch, module, env.clone())?;
                    returns.extend(branch_returns);
                    branch_envs.push(branch_env);
                    for elif in elifs {
                        let cond_origins = self.visit_expr(&elif.cond, module, &mut env)?;
                        self.record_origins(&cond_origins)?;
                        let (branch_returns, branch_env) =
                            self.visit_stmts(&elif.body, module, env.clone())?;
                        returns.extend(branch_returns);
                        branch_envs.push(branch_env);
                    }
                    if let Some(body) = else_branch {
                        let (branch_returns, branch_env) =
                            self.visit_stmts(body, module, env.clone())?;
                        returns.extend(branch_returns);
                        branch_envs.push(branch_env);
                    }
                    env = Self::merge_envs(&env, branch_envs);
                }
                TypedStmtKind::Match { scrutinee, arms } => {
                    let scrutinee_origins = self.visit_expr(scrutinee, module, &mut env)?;
                    self.record_origins(&scrutinee_origins)?;
                    let mut branch_envs = Vec::new();
                    for arm in arms {
                        if let Some(guard) = &arm.guard {
                            let guard_origins = self.visit_expr(guard, module, &mut env)?;
                            self.record_origins(&guard_origins)?;
                        }
                        let (branch_returns, branch_env) =
                            self.visit_stmts(&arm.body, module, env.clone())?;
                        returns.extend(branch_returns);
                        branch_envs.push(branch_env);
                    }
                    env = Self::merge_envs(&env, branch_envs);
                }
                TypedStmtKind::For {
                    name, iter, body, ..
                } => {
                    let mut loop_env = env.clone();
                    let iterations = match iter {
                        TypedForIter::Range(start, end, inclusive) => {
                            self.visit_expr(start, module, &mut env)?;
                            self.visit_expr(end, module, &mut env)?;
                            let start_value =
                                super::legality::constant_integer(self.programs, module, start)
                                    .ok_or_else(|| {
                                        "P024: field loop bound is not comptime-known".to_string()
                                    })?;
                            let end_value =
                                super::legality::constant_integer(self.programs, module, end)
                                    .ok_or_else(|| {
                                        "P024: field loop bound is not comptime-known".to_string()
                                    })?;
                            let exclusive_end = if *inclusive {
                                end_value.checked_add(1).ok_or_else(|| {
                                    "P024: field loop bound is out of range".to_string()
                                })?
                            } else {
                                end_value
                            };
                            (start_value..exclusive_end)
                                .map(|value| {
                                    vec![Origin {
                                        path: Vec::new(),
                                        projection: Vec::new(),
                                        ty: start.ty.clone(),
                                        module: module.to_string(),
                                        unmodified: true,
                                        contracts: Default::default(),
                                        exact_comptime_integer: Some(value),
                                    }]
                                })
                                .collect::<Vec<_>>()
                        }
                        TypedForIter::Expr(value) => {
                            let bases = self.visit_expr(value, module, &mut env)?;
                            let mut elements: Vec<Vec<Origin>> = Vec::new();
                            for base in bases {
                                if let Some(index) = base.projection.first().copied() {
                                    if let Some(origin) = Self::projected_origin(base, index) {
                                        elements.push(vec![origin]);
                                    }
                                    continue;
                                }
                                let Type::Array(element, length) = &base.ty else {
                                    continue;
                                };
                                let Some(length) = crate::sema::bodies::literal_array_len(length)
                                    .and_then(|length| usize::try_from(length).ok())
                                else {
                                    continue;
                                };
                                for index in 0..length {
                                    let mut origin = base.clone();
                                    origin.path.push(index);
                                    origin.ty = (**element).clone();
                                    origin.contracts = Default::default();
                                    elements.push(vec![origin]);
                                }
                            }
                            elements
                        }
                    };
                    for iteration in iterations {
                        loop_env.insert(name.clone(), iteration);
                        let (loop_returns, next_loop_env) =
                            self.visit_stmts(body, module, loop_env)?;
                        returns.extend(loop_returns);
                        loop_env = next_loop_env;
                    }
                    env = loop_env;
                }
                TypedStmtKind::While { cond, body, .. } => {
                    self.visit_expr(cond, module, &mut env)?;
                    let (loop_returns, final_loop_env) =
                        self.visit_stmts(body, module, env.clone())?;
                    returns.extend(loop_returns);
                    env = Self::merge_envs(&env, [final_loop_env]);
                }
                TypedStmtKind::Return(Some(value)) => {
                    for origin in self.visit_expr(value, module, &mut env)? {
                        if !returns.contains(&origin) {
                            returns.push(origin);
                        }
                    }
                }
                TypedStmtKind::ExprStmt(value) | TypedStmtKind::BareSend { expr: value, .. } => {
                    self.visit_expr(value, module, &mut env)?;
                }
                TypedStmtKind::Assert { cond, message }
                | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
                    self.visit_expr(cond, module, &mut env)?;
                    if let Some(message) = message {
                        self.visit_expr(message, module, &mut env)?;
                    }
                }
                TypedStmtKind::Defer(TypedDeferBody::Expr(value)) => {
                    self.visit_expr(value, module, &mut env)?;
                }
                TypedStmtKind::Defer(TypedDeferBody::Suite(body))
                | TypedStmtKind::WithGroup { body, .. } => {
                    let (body_returns, body_env) = self.visit_stmts(body, module, env.clone())?;
                    returns.extend(body_returns);
                    env = Self::merge_envs(&env, [body_env]);
                }
                TypedStmtKind::Break
                | TypedStmtKind::Continue
                | TypedStmtKind::Pass
                | TypedStmtKind::Return(None) => {}
            }
        }
        Ok((returns, env))
    }
}

fn root_env(function: &TypedFn, module: &str) -> BTreeMap<String, Vec<Origin>> {
    function
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let origins = if index == 1 {
                vec![Origin {
                    path: Vec::new(),
                    projection: Vec::new(),
                    ty: param.ty.clone(),
                    module: module.to_string(),
                    unmodified: true,
                    contracts: Default::default(),
                    exact_comptime_integer: None,
                }]
            } else {
                Vec::new()
            };
            (param.name.clone(), origins)
        })
        .collect()
}

pub fn collect_parameter_contracts(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    _params_type: &Type,
    field: &str,
    material: &str,
) -> Result<Vec<ParameterContract>, String> {
    let field = root_function(owner, programs, field)?;
    let material = root_function(owner, programs, material)?;
    let field_env = root_env(&field.function, &field.module);
    let material_env = root_env(&material.function, &material.module);
    let mut collector = Collector {
        programs,
        active: BTreeSet::new(),
        found: BTreeMap::new(),
    };
    collector.visit_function(field, field_env)?;
    collector.visit_function(material, material_env)?;
    Ok(collector.found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_ranges_are_optional_but_retained_when_present() {
        for ty in [
            Type::U8,
            Type::U16,
            Type::U32,
            Type::U64,
            Type::Usize,
            Type::I8,
            Type::I16,
            Type::I32,
            Type::I64,
            Type::Isize,
        ] {
            assert!(is_optional_integer_range(&ty), "{ty:?}");
            assert!(!requires_parameter_range(&ty), "{ty:?}");
        }
        assert!(!is_optional_integer_range(&Type::Bool));
        assert!(requires_parameter_range(&Type::F32));
    }

    #[test]
    fn field_renames_change_source_digest_without_changing_path_order() {
        fn paths(fields: &[&str]) -> Vec<Vec<usize>> {
            let mut program = TypedProgram {
                module_path: "scene".to_string(),
                ..TypedProgram::default()
            };
            program
                .type_decl_modules
                .insert("Params".to_string(), "scene".to_string());
            program
                .type_decl_names
                .insert("Params".to_string(), "Params".to_string());
            let mut params = TypedStruct {
                name: "Params".to_string(),
                fields: fields.iter().map(|field| (*field).to_string()).collect(),
                ..TypedStruct::default()
            };
            for field in fields {
                params.field_types.insert((*field).to_string(), Type::F32);
            }
            program.structs.insert("Params".to_string(), params);
            let programs = BTreeMap::from([("scene".to_string(), program)]);
            let collector = Collector {
                programs: &programs,
                active: BTreeSet::new(),
                found: BTreeMap::new(),
            };
            let root = Origin {
                path: Vec::new(),
                projection: Vec::new(),
                ty: Type::Named("Params".to_string(), Vec::new()),
                module: "scene".to_string(),
                unmodified: true,
                contracts: Default::default(),
                exact_comptime_integer: None,
            };
            fields
                .iter()
                .map(|field| collector.field_origin(&root, field).unwrap().path)
                .collect()
        }

        let original = b"struct Params:\n    height: f32\n    tint: f32\n";
        let renamed = b"struct Params:\n    elevation: f32\n    tone: f32\n";
        assert_ne!(
            wrela_machine::sha256::sha256_hex(original),
            wrela_machine::sha256::sha256_hex(renamed)
        );
        assert_eq!(paths(&["height", "tint"]), paths(&["elevation", "tone"]));
    }
}
