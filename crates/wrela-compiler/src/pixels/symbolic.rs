//! Finite symbolic evaluator for accepted `@field` and `@material` bodies.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use crate::sema::typed::{
    PixelsFnKind, TypedCallArg, TypedExpr, TypedExprKind, TypedForIter, TypedMatchArm,
    TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::sema::types::Type;
use crate::syntax::ast::{BinOp, Span};

use super::arena::{NodeOrigin, OriginSite};
use super::config::RendererConfig;
use super::field_intrinsics::FieldIntrinsic;
use super::graph::{
    Axis, CanonicalIdentity, ClosedDeformDerivation, DerivedDeformContract, FieldArena, FieldKind,
    FieldNode, Primitive, TransformProgram,
};
use super::ids::{FieldId, MaterialId, ParamId, ScalarId};
use super::material_graph::{
    MaterialArena, MaterialKind, MaterialNode, MaterialSampleNode, NormalModel, TextureFilterV1,
    UvSourceV1,
};
use super::material_intrinsics::MaterialIntrinsic;
use super::quota::SymbolicQuota;
use super::scalar::{
    CompareOp, Dependency, ProofObligation, ScalarArena, ScalarIntrinsic, ScalarNode, ScalarOp,
    SemanticOpId,
};
#[cfg(test)]
use super::scalar::{source_max, source_min, source_smooth_min};
use super::{LocatedFn, call_base, called_function, located_constant, root_function};

#[derive(Clone, Debug, PartialEq)]
pub struct ParamRecord {
    pub id: ParamId,
    pub path: Vec<usize>,
    pub component: Option<u8>,
    pub spelling: String,
    pub ty: Type,
    pub range_min: f64,
    pub range_max: f64,
    pub exact_integer: Option<(i128, i128)>,
    pub rate: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PendingObligation {
    Scalar(ProofObligation),
    MaterialEvent { predicate: ScalarId },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolicGraph {
    pub renderer_index: usize,
    pub field_key: String,
    pub material_key: String,
    pub params_type: Type,
    pub material_type: Type,
    pub params: Vec<ParamRecord>,
    pub scalar: ScalarArena,
    pub fields: FieldArena,
    pub materials: MaterialArena,
    pub field_root: FieldId,
    pub material_root: MaterialId,
    pub obligations: Vec<PendingObligation>,
    pub quota: SymbolicQuota,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SymbolicFailure {
    pub message: String,
    pub primary: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SymBool {
    Const(bool),
    Runtime(ScalarId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SymInt {
    Const(i128),
    Runtime(ScalarId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParamProxy {
    ty: Type,
    path: Vec<usize>,
    component: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CoordTransform {
    Translate([ScalarId; 3]),
    Rotate {
        row_x: [ScalarId; 3],
        row_y: [ScalarId; 3],
        row_z: [ScalarId; 3],
    },
    Rigid {
        translation: [ScalarId; 3],
        row_x: [ScalarId; 3],
        row_y: [ScalarId; 3],
        row_z: [ScalarId; 3],
    },
    Repeat {
        axis: Axis,
        first: i32,
        count: u32,
        period: ScalarId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocatedCoordTransform {
    kind: CoordTransform,
    origin: NodeOrigin,
}

enum FieldTransformLayer {
    Program {
        transform: TransformProgram,
        origins: Vec<NodeOrigin>,
    },
    Repeat {
        axis: Axis,
        first: i32,
        count: u32,
        period: ScalarId,
        origin: NodeOrigin,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SymVec3 {
    values: [ScalarId; 3],
    transforms: Vec<LocatedCoordTransform>,
    /// True only when this value is the renderer coordinate or was produced
    /// from it by the closed coordinate-transform API. Scalar dependency
    /// taint is deliberately insufficient: arbitrary coordinate arithmetic
    /// is not a canonical geometric transform.
    coordinate_provenance: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SymValue {
    Unit,
    Bool(SymBool),
    Int(SymInt),
    F32(ScalarId),
    F64(ScalarId),
    Vec2([ScalarId; 2]),
    Vec3(SymVec3),
    Rgb([ScalarId; 3]),
    Field(FieldId),
    Material(MaterialId),
    Struct(Vec<SymValue>),
    Array(Vec<SymValue>),
    Enum(CanonicalIdentity, Vec<SymValue>),
    Param(ParamProxy),
    ParamAlternatives {
        values: Vec<ParamProxy>,
        index: ScalarId,
    },
    MaterialIdentity {
        enum_key: String,
    },
}

enum Flow {
    Continue,
    Return(SymValue),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum StructuralToken {
    Tag(u16),
    U32(u32),
    I32(i32),
    Bits32(u32),
    Bits64(u64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StructuralKeyNode {
    local: Vec<StructuralToken>,
    children: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum StructuralSource {
    Field(FieldId),
    Scalar(ScalarId),
}

struct Compiler<'a> {
    programs: &'a BTreeMap<String, TypedProgram>,
    owner_module: &'a str,
    config: &'a RendererConfig,
    kind: PixelsFnKind,
    call_stack: Vec<String>,
    call_sites: Vec<OriginSite>,
    scopes: Vec<BTreeMap<String, SymValue>>,
    scalar: ScalarArena,
    fields: FieldArena,
    materials: MaterialArena,
    scalar_cse: BTreeMap<ScalarOp, ScalarId>,
    field_cse: BTreeMap<(FieldKind, ScalarId), FieldId>,
    material_cse: BTreeMap<MaterialKind, MaterialId>,
    structural_keys: Vec<StructuralKeyNode>,
    structural_key_cse: BTreeMap<StructuralKeyNode, u32>,
    field_structural_keys: BTreeMap<FieldId, u32>,
    scalar_structural_keys: BTreeMap<ScalarId, u32>,
    params: Vec<ParamRecord>,
    param_ids: BTreeMap<(Vec<usize>, Option<u8>), ParamId>,
    obligations: Vec<PendingObligation>,
    quota: SymbolicQuota,
    last_span: Cell<Span>,
}

impl<'a> Compiler<'a> {
    fn new(
        programs: &'a BTreeMap<String, TypedProgram>,
        owner_module: &'a str,
        config: &'a RendererConfig,
        renderer_index: usize,
    ) -> Result<Self, String> {
        let arena_base = u32::try_from(renderer_index)
            .ok()
            .and_then(|index| index.checked_mul(3))
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| "P015: renderer arena identity capacity exhausted".to_string())?;
        let mut this = Self {
            programs,
            owner_module,
            config,
            kind: PixelsFnKind::Field,
            call_stack: Vec::new(),
            call_sites: Vec::new(),
            scopes: Vec::new(),
            scalar: ScalarArena::new(arena_base),
            fields: FieldArena::new(arena_base + 1),
            materials: MaterialArena::new(arena_base + 2),
            scalar_cse: BTreeMap::new(),
            field_cse: BTreeMap::new(),
            material_cse: BTreeMap::new(),
            structural_keys: Vec::new(),
            structural_key_cse: BTreeMap::new(),
            field_structural_keys: BTreeMap::new(),
            scalar_structural_keys: BTreeMap::new(),
            params: Vec::new(),
            param_ids: BTreeMap::new(),
            obligations: Vec::new(),
            quota: SymbolicQuota::default(),
            last_span: Cell::new(Span::default()),
        };
        this.predeclare_params()?;
        Ok(this)
    }

    fn origin(&self, module: &str, span: Span) -> NodeOrigin {
        NodeOrigin::new(module, span, self.call_sites.clone())
    }

    fn error(&self, code: &str, message: impl AsRef<str>) -> String {
        format!(
            "{code}: {}\n  renderer call chain: {}",
            message.as_ref(),
            self.call_stack.join(" -> ")
        )
    }

    fn predeclare_params(&mut self) -> Result<(), String> {
        let contracts = self.config.parameter_contracts.clone();
        for contract in contracts {
            let components = vector_components(self.programs, self.owner_module, &contract.ty);
            if let Some(count) = components {
                for component in 0..count {
                    self.declare_param(&contract, Some(component))?;
                }
            } else {
                self.declare_param(&contract, None)?;
            }
        }
        Ok(())
    }

    fn declare_param(
        &mut self,
        contract: &super::params::ParameterContract,
        component: Option<u8>,
    ) -> Result<(), String> {
        let id = ParamId(
            u32::try_from(self.params.len())
                .map_err(|_| "pixels::params: parameter ID overflow".to_string())?,
        );
        let spelling = param_spelling(
            self.programs,
            self.owner_module,
            &self.config.params_type,
            &contract.path,
            component,
        );
        self.param_ids
            .insert((contract.path.clone(), component), id);
        self.params.push(ParamRecord {
            id,
            path: contract.path.clone(),
            component,
            spelling,
            ty: contract.ty.clone(),
            range_min: contract.range.min,
            range_max: contract.range.max,
            exact_integer: contract.range.exact_integer,
            rate: contract
                .rate
                .map(|rate| (rate.max_delta, rate.max_second_delta)),
        });
        Ok(())
    }

    fn scalar_node(
        &mut self,
        op: ScalarOp,
        dependency: Dependency,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        let op = self.fold_scalar(&op).unwrap_or(op);
        let dependency = if matches!(op, ScalarOp::ConstF32(_) | ScalarOp::ConstF64(_)) {
            Dependency::Constant
        } else {
            dependency
        };
        if let Some(id) = self.scalar_cse.get(&op).copied() {
            let other = self.origin(module, span);
            self.scalar.origin_mut(id)?.merge(&other);
            return Ok(id);
        }
        self.quota.node(
            self.scalar.len() + self.fields.len() + self.materials.len(),
            &self.call_stack,
        )?;
        let id = self.scalar.push(
            ScalarNode {
                op: op.clone(),
                dependency,
            },
            self.origin(module, span),
        )?;
        self.scalar_cse.insert(op, id);
        Ok(id)
    }

    fn constant_value(&self, id: ScalarId) -> Option<f32> {
        super::scalar::constant_value(&self.scalar, id)
    }

    fn constant_value_f64(&self, id: ScalarId) -> Option<f64> {
        match self.scalar.get(id).ok()?.op {
            ScalarOp::ConstF64(bits) => Some(f64::from_bits(bits)),
            _ => None,
        }
    }

    fn is_const_f32(&self, id: ScalarId, value: f32) -> bool {
        self.constant_value(id)
            .is_some_and(|found| found.to_bits() == value.to_bits())
    }

    fn is_zero_vec3(&self, value: [ScalarId; 3]) -> bool {
        value
            .into_iter()
            .all(|component| self.is_const_f32(component, 0.0))
    }

    fn is_identity_rows(
        &self,
        row_x: [ScalarId; 3],
        row_y: [ScalarId; 3],
        row_z: [ScalarId; 3],
    ) -> bool {
        [row_x, row_y, row_z]
            .into_iter()
            .flatten()
            .zip([1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
            .all(|(id, expected)| self.is_const_f32(id, expected))
    }

    fn fold_scalar(&self, op: &ScalarOp) -> Option<ScalarOp> {
        super::scalar::fold_constant(&self.scalar, op)
    }

    fn field_node(
        &mut self,
        kind: FieldKind,
        scalar_value: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<FieldId, String> {
        let origin = self.origin(module, span);
        self.field_node_with_origin(kind, scalar_value, origin)
    }

    fn field_node_with_origin(
        &mut self,
        kind: FieldKind,
        scalar_value: ScalarId,
        origin: NodeOrigin,
    ) -> Result<FieldId, String> {
        let key = (kind.clone(), scalar_value);
        if let Some(id) = self.field_cse.get(&key).copied() {
            self.fields.origin_mut(id)?.merge(&origin);
            return Ok(id);
        }
        self.quota.node(
            self.scalar.len() + self.fields.len() + self.materials.len(),
            &self.call_stack,
        )?;
        let id = self.fields.push(
            FieldNode {
                kind: kind.clone(),
                scalar_value,
            },
            origin,
        )?;
        self.field_cse.insert(key, id);
        Ok(id)
    }

    fn material_node(
        &mut self,
        kind: MaterialKind,
        module: &str,
        span: Span,
    ) -> Result<MaterialId, String> {
        if let Some(id) = self.material_cse.get(&kind).copied() {
            let other = self.origin(module, span);
            self.materials.origin_mut(id)?.merge(&other);
            return Ok(id);
        }
        self.quota.node(
            self.scalar.len() + self.fields.len() + self.materials.len(),
            &self.call_stack,
        )?;
        let id = self.materials.push(
            MaterialNode { kind: kind.clone() },
            self.origin(module, span),
        )?;
        self.material_cse.insert(kind, id);
        Ok(id)
    }

    fn const_f32(&mut self, value: f32, module: &str, span: Span) -> Result<ScalarId, String> {
        self.scalar_node(
            ScalarOp::ConstF32(value.to_bits()),
            Dependency::Constant,
            module,
            span,
        )
    }

    fn const_f64(&mut self, value: f64, module: &str, span: Span) -> Result<ScalarId, String> {
        self.scalar_node(
            ScalarOp::ConstF64(value.to_bits()),
            Dependency::Constant,
            module,
            span,
        )
    }

    fn scalar_dependency(&self, id: ScalarId) -> Result<Dependency, String> {
        Ok(self.scalar.get(id)?.dependency)
    }

    fn binary_scalar(
        &mut self,
        op: BinOp,
        a: ScalarId,
        b: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        let dependency = self
            .scalar_dependency(a)?
            .combine(self.scalar_dependency(b)?);
        let scalar_op = match op {
            BinOp::Add | BinOp::AddW => ScalarOp::Add(a, b),
            BinOp::Sub | BinOp::SubW => ScalarOp::Sub(a, b),
            BinOp::Mul | BinOp::MulW => ScalarOp::Mul(a, b),
            BinOp::Div => {
                self.obligations.push(PendingObligation::Scalar(
                    ProofObligation::DenominatorNonZero { denominator: b },
                ));
                ScalarOp::Div(a, b)
            }
            BinOp::Lt => ScalarOp::Compare {
                op: CompareOp::Lt,
                a,
                b,
            },
            BinOp::Le => ScalarOp::Compare {
                op: CompareOp::Le,
                a,
                b,
            },
            BinOp::Gt => ScalarOp::Compare {
                op: CompareOp::Gt,
                a,
                b,
            },
            BinOp::Ge => ScalarOp::Compare {
                op: CompareOp::Ge,
                a,
                b,
            },
            BinOp::Eq => ScalarOp::Compare {
                op: CompareOp::Eq,
                a,
                b,
            },
            BinOp::Ne => ScalarOp::Compare {
                op: CompareOp::Ne,
                a,
                b,
            },
            _ => {
                return Err(self.error(
                    "P004",
                    format!("unsupported symbolic scalar operator `{}`", op.as_str()),
                ));
            }
        };
        let result = self.scalar_node(scalar_op, dependency, module, span)?;
        if matches!(
            op,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne
        ) {
            if let Some(value) = self.constant_value(result) {
                Ok(SymValue::Bool(SymBool::Const(value != 0.0)))
            } else {
                Ok(SymValue::Bool(SymBool::Runtime(result)))
            }
        } else {
            Ok(SymValue::F32(result))
        }
    }

    fn scalar_unary(
        &mut self,
        op: fn(ScalarId) -> ScalarOp,
        value: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        self.scalar_node(op(value), self.scalar_dependency(value)?, module, span)
    }

    fn add(
        &mut self,
        a: ScalarId,
        b: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        expect_f32(self.binary_scalar(BinOp::Add, a, b, module, span)?)
    }

    fn sub(
        &mut self,
        a: ScalarId,
        b: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        expect_f32(self.binary_scalar(BinOp::Sub, a, b, module, span)?)
    }

    fn mul(
        &mut self,
        a: ScalarId,
        b: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        expect_f32(self.binary_scalar(BinOp::Mul, a, b, module, span)?)
    }

    fn div(
        &mut self,
        a: ScalarId,
        b: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        expect_f32(self.binary_scalar(BinOp::Div, a, b, module, span)?)
    }

    fn neg(&mut self, a: ScalarId, module: &str, span: Span) -> Result<ScalarId, String> {
        self.scalar_unary(ScalarOp::Neg, a, module, span)
    }

    fn abs(&mut self, a: ScalarId, module: &str, span: Span) -> Result<ScalarId, String> {
        self.scalar_unary(ScalarOp::Abs, a, module, span)
    }

    fn min(
        &mut self,
        a: ScalarId,
        b: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        if a == b {
            return Ok(a);
        }
        let dependency = self
            .scalar_dependency(a)?
            .combine(self.scalar_dependency(b)?);
        self.scalar_node(ScalarOp::Min(a, b), dependency, module, span)
    }

    fn max(
        &mut self,
        a: ScalarId,
        b: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        if a == b {
            return Ok(a);
        }
        let dependency = self
            .scalar_dependency(a)?
            .combine(self.scalar_dependency(b)?);
        self.scalar_node(ScalarOp::Max(a, b), dependency, module, span)
    }

    fn length3(
        &mut self,
        value: [ScalarId; 3],
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        let dependency = value
            .iter()
            .try_fold(Dependency::Constant, |dependency, id| {
                Ok::<_, String>(dependency.combine(self.scalar_dependency(*id)?))
            })?;
        self.scalar_node(ScalarOp::Length3(value), dependency, module, span)
    }

    fn length2(
        &mut self,
        value: [ScalarId; 2],
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        let dependency = value
            .iter()
            .try_fold(Dependency::Constant, |dependency, id| {
                Ok::<_, String>(dependency.combine(self.scalar_dependency(*id)?))
            })?;
        self.scalar_node(ScalarOp::Length2(value), dependency, module, span)
    }

    fn dot3(
        &mut self,
        a: [ScalarId; 3],
        b: [ScalarId; 3],
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        let mut dependency = Dependency::Constant;
        for id in a.into_iter().chain(b) {
            dependency = dependency.combine(self.scalar_dependency(id)?);
        }
        self.scalar_node(ScalarOp::Dot3(a, b), dependency, module, span)
    }

    fn param_scalar(
        &mut self,
        proxy: &ParamProxy,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        let id = self
            .param_ids
            .get(&(proxy.path.clone(), proxy.component))
            .copied()
            .ok_or_else(|| {
                self.error(
                    "P005",
                    format!(
                        "parameter path {:?} component {:?} was not validated",
                        proxy.path, proxy.component
                    ),
                )
            })?;
        self.scalar_node(ScalarOp::Param(id), Dependency::Parameter, module, span)
    }

    fn as_scalar(&mut self, value: SymValue, module: &str, span: Span) -> Result<ScalarId, String> {
        match value {
            SymValue::F32(value) => Ok(value),
            SymValue::Int(SymInt::Const(value)) => self.const_f32(value as f32, module, span),
            SymValue::Int(SymInt::Runtime(value)) => Ok(value),
            SymValue::Param(proxy) if is_scalar_type(&proxy.ty) && proxy.ty != Type::F64 => {
                self.param_scalar(&proxy, module, span)
            }
            SymValue::ParamAlternatives { values, index }
                if values
                    .iter()
                    .all(|value| is_scalar_type(&value.ty) && value.ty != Type::F64) =>
            {
                let options = values
                    .iter()
                    .map(|value| self.param_scalar(value, module, span))
                    .collect::<Result<Vec<_>, _>>()?;
                self.obligations.push(PendingObligation::Scalar(
                    ProofObligation::DynamicIndexInBounds {
                        index,
                        extent: u32::try_from(options.len()).map_err(|_| {
                            self.error("P014", "dynamic parameter alternative count overflow")
                        })?,
                    },
                ));
                let dependency = options.iter().try_fold(
                    self.scalar_dependency(index)?,
                    |dependency, option| {
                        Ok::<_, String>(dependency.combine(self.scalar_dependency(*option)?))
                    },
                )?;
                self.scalar_node(
                    ScalarOp::SelectIndex { index, options },
                    dependency,
                    module,
                    span,
                )
            }
            other => Err(self.error(
                "P004",
                format!("expected a scalar symbolic value, found {other:?}"),
            )),
        }
    }

    fn as_vec3(&mut self, value: SymValue, module: &str, span: Span) -> Result<SymVec3, String> {
        match value {
            SymValue::Vec3(value) => Ok(value),
            SymValue::Param(proxy)
                if vector_components(self.programs, module, &proxy.ty) == Some(3) =>
            {
                Ok(SymVec3 {
                    values: self.param_vector_components(&proxy, module, span)?,
                    transforms: Vec::new(),
                    coordinate_provenance: false,
                })
            }
            SymValue::ParamAlternatives { values, index }
                if values.iter().all(|proxy| {
                    vector_components(self.programs, module, &proxy.ty) == Some(3)
                }) =>
            {
                Ok(SymVec3 {
                    values: self
                        .param_alternative_vector_components(&values, index, module, span)?,
                    transforms: Vec::new(),
                    coordinate_provenance: false,
                })
            }
            other => Err(self.error(
                "P004",
                format!("expected `Vec3`, found symbolic value {other:?}"),
            )),
        }
    }

    fn vec3_dependency(&self, value: &[ScalarId; 3]) -> Result<Dependency, String> {
        value
            .iter()
            .try_fold(Dependency::Constant, |dependency, component| {
                Ok(dependency.combine(self.scalar_dependency(*component)?))
            })
    }

    fn dependency_has_coordinate(dependency: Dependency) -> bool {
        matches!(
            dependency,
            Dependency::Coordinate
                | Dependency::CoordinateAndParameter
                | Dependency::CoordinateAndSurface
                | Dependency::CoordinateParameterAndSurface
        )
    }

    fn require_coordinate_free_scalar(
        &self,
        value: ScalarId,
        operation: &str,
        argument: &str,
    ) -> Result<(), String> {
        if Self::dependency_has_coordinate(self.scalar_dependency(value)?) {
            return Err(self.error(
                "P004",
                format!(
                    "field operation `{operation}` is not available in `AaaByteExact`: \
                     geometric coefficient `{argument}` must be coordinate-free"
                ),
            ));
        }
        Ok(())
    }

    fn require_coordinate_free_vec3(
        &self,
        value: [ScalarId; 3],
        operation: &str,
        argument: &str,
    ) -> Result<(), String> {
        for component in value {
            self.require_coordinate_free_scalar(component, operation, argument)?;
        }
        Ok(())
    }

    fn is_renderer_coordinate_triplet(&self, values: [ScalarId; 3]) -> bool {
        matches!(
            (
                self.scalar.get(values[0]).map(|node| &node.op),
                self.scalar.get(values[1]).map(|node| &node.op),
                self.scalar.get(values[2]).map(|node| &node.op),
            ),
            (
                Ok(ScalarOp::CoordX),
                Ok(ScalarOp::CoordY),
                Ok(ScalarOp::CoordZ)
            )
        )
    }

    fn as_vec2(
        &mut self,
        value: SymValue,
        module: &str,
        span: Span,
    ) -> Result<[ScalarId; 2], String> {
        match value {
            SymValue::Vec2(value) => Ok(value),
            SymValue::Param(proxy)
                if vector_components(self.programs, module, &proxy.ty) == Some(2) =>
            {
                self.param_vector_components(&proxy, module, span)
            }
            SymValue::ParamAlternatives { values, index }
                if values.iter().all(|proxy| {
                    vector_components(self.programs, module, &proxy.ty) == Some(2)
                }) =>
            {
                self.param_alternative_vector_components(&values, index, module, span)
            }
            other => Err(self.error(
                "P004",
                format!("expected `Vec2`, found symbolic value {other:?}"),
            )),
        }
    }

    fn as_rgb(
        &mut self,
        value: SymValue,
        module: &str,
        span: Span,
    ) -> Result<[ScalarId; 3], String> {
        match value {
            SymValue::Rgb(value) => Ok(value),
            SymValue::Param(proxy)
                if vector_components(self.programs, module, &proxy.ty) == Some(3) =>
            {
                self.param_vector_components(&proxy, module, span)
            }
            SymValue::ParamAlternatives { values, index }
                if values.iter().all(|proxy| {
                    vector_components(self.programs, module, &proxy.ty) == Some(3)
                }) =>
            {
                self.param_alternative_vector_components(&values, index, module, span)
            }
            other => Err(self.error(
                "P004",
                format!("expected `Rgb`, found symbolic value {other:?}"),
            )),
        }
    }

    fn param_vector_components<const N: usize>(
        &mut self,
        proxy: &ParamProxy,
        module: &str,
        span: Span,
    ) -> Result<[ScalarId; N], String> {
        let mut components = [ScalarId(0); N];
        for (component, slot) in components.iter_mut().enumerate() {
            let mut component_proxy = proxy.clone();
            component_proxy.component =
                Some(u8::try_from(component).map_err(|_| {
                    self.error("P014", "parameter vector component index overflow")
                })?);
            *slot = self.param_scalar(&component_proxy, module, span)?;
        }
        Ok(components)
    }

    fn param_alternative_vector_components<const N: usize>(
        &mut self,
        values: &[ParamProxy],
        index: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<[ScalarId; N], String> {
        let extent = u32::try_from(values.len())
            .map_err(|_| self.error("P014", "dynamic parameter alternative count overflow"))?;
        if extent == 0 {
            return Err(self.error("P004", "dynamic parameter vector has no alternatives"));
        }
        self.obligations.push(PendingObligation::Scalar(
            ProofObligation::DynamicIndexInBounds { index, extent },
        ));
        let mut components = [ScalarId(0); N];
        for (component, slot) in components.iter_mut().enumerate() {
            let component = u8::try_from(component)
                .map_err(|_| self.error("P014", "parameter vector component index overflow"))?;
            let options = values
                .iter()
                .map(|proxy| {
                    let mut component_proxy = proxy.clone();
                    component_proxy.component = Some(component);
                    self.param_scalar(&component_proxy, module, span)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let dependency =
                options
                    .iter()
                    .try_fold(self.scalar_dependency(index)?, |dependency, option| {
                        Ok::<_, String>(dependency.combine(self.scalar_dependency(*option)?))
                    })?;
            *slot = self.scalar_node(
                ScalarOp::SelectIndex { index, options },
                dependency,
                module,
                span,
            )?;
        }
        Ok(components)
    }

    fn project_param(
        &self,
        proxy: ParamProxy,
        module: &str,
        field: &str,
    ) -> Result<ParamProxy, String> {
        if let Some(component) = vector_field_component(self.programs, module, &proxy.ty, field) {
            let mut result = proxy;
            result.ty = Type::F32;
            result.component = Some(component);
            return Ok(result);
        }
        let (index, ty) =
            struct_field(self.programs, module, &proxy.ty, field).ok_or_else(|| {
                self.error(
                    "P004",
                    format!(
                        "cannot project field `{field}` from `{}`",
                        crate::sema::types::render_type(&proxy.ty)
                    ),
                )
            })?;
        let mut result = proxy;
        result.path.push(index);
        result.ty = ty;
        result.component = None;
        Ok(result)
    }

    fn eval_expr(&mut self, expr: &TypedExpr, module: &str) -> Result<SymValue, String> {
        let span = expr.span;
        self.last_span.set(span);
        let result = self.eval_expr_inner(expr, module);
        if result.is_ok() {
            self.last_span.set(span);
        }
        result
    }

    fn eval_expr_inner(&mut self, expr: &TypedExpr, module: &str) -> Result<SymValue, String> {
        self.quota.step(&self.call_stack)?;
        match &expr.kind {
            TypedExprKind::Float(text) => {
                if expr.ty == Type::F64 {
                    let value = text
                        .replace('_', "")
                        .parse::<f64>()
                        .map_err(|_| self.error("P004", format!("invalid f64 literal `{text}`")))?;
                    Ok(SymValue::F64(self.const_f64(value, module, expr.span)?))
                } else {
                    let value = text
                        .replace('_', "")
                        .parse::<f32>()
                        .map_err(|_| self.error("P004", format!("invalid f32 literal `{text}`")))?;
                    Ok(SymValue::F32(self.const_f32(value, module, expr.span)?))
                }
            }
            TypedExprKind::Int(text) => {
                let value = crate::eval::value::parse_int_literal(text).ok_or_else(|| {
                    self.error("P004", format!("invalid integer literal `{text}`"))
                })?;
                if expr.ty == Type::F64 {
                    Ok(SymValue::F64(self.const_f64(
                        value as f64,
                        module,
                        expr.span,
                    )?))
                } else if expr.ty == Type::F32 {
                    Ok(SymValue::F32(self.const_f32(
                        value as f32,
                        module,
                        expr.span,
                    )?))
                } else {
                    Ok(SymValue::Int(SymInt::Const(value)))
                }
            }
            TypedExprKind::Bool(value) => Ok(SymValue::Bool(SymBool::Const(*value))),
            TypedExprKind::Unit => Ok(SymValue::Unit),
            TypedExprKind::Local(name) => self.lookup(name),
            TypedExprKind::Const(name) => {
                let located = located_constant(self.programs, module, name)
                    .ok_or_else(|| self.error("P004", format!("missing constant `{name}`")))?;
                self.eval_expr(&located.constant.value, &located.module)
            }
            TypedExprKind::Field(base, field) => {
                let base_value = self.eval_expr(base, module)?;
                match base_value {
                    SymValue::Param(proxy) => {
                        Ok(SymValue::Param(self.project_param(proxy, module, field)?))
                    }
                    SymValue::ParamAlternatives { values, index } => {
                        let values = values
                            .into_iter()
                            .map(|proxy| self.project_param(proxy, module, field))
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(SymValue::ParamAlternatives { values, index })
                    }
                    SymValue::Vec3(value) => {
                        let component = vector_field_index(field).ok_or_else(|| {
                            self.error("P004", format!("unknown Vec3 field `{field}`"))
                        })?;
                        Ok(SymValue::F32(value.values[component]))
                    }
                    SymValue::Rgb(value) => {
                        let component = rgb_field_index(field).ok_or_else(|| {
                            self.error("P004", format!("unknown Rgb field `{field}`"))
                        })?;
                        Ok(SymValue::F32(value[component]))
                    }
                    SymValue::Struct(values) => {
                        let (index, _) = struct_field(self.programs, module, &base.ty, field)
                            .ok_or_else(|| {
                                self.error("P004", format!("unknown struct field `{field}`"))
                            })?;
                        values.get(index).cloned().ok_or_else(|| {
                            self.error("P004", format!("missing struct field index {index}"))
                        })
                    }
                    SymValue::MaterialIdentity { enum_key } if field == "material" => {
                        Ok(SymValue::MaterialIdentity { enum_key })
                    }
                    other => Err(self.error(
                        "P004",
                        format!("unsupported field projection `{field}` from {other:?}"),
                    )),
                }
            }
            TypedExprKind::Index(base, index) => {
                let base_value = self.eval_expr(base, module)?;
                let index_value = self.eval_expr(index, module)?;
                self.index_value(base_value, index_value, module, expr.span)
            }
            TypedExprKind::ToScalar(inner) => {
                let value = self.eval_expr(inner, module)?;
                if expr.ty == Type::F64 {
                    match value {
                        SymValue::F64(value) => Ok(SymValue::F64(value)),
                        SymValue::F32(value) => {
                            let value = self.constant_value(value).ok_or_else(|| {
                                self.error("P004", "runtime f32-to-f64 conversion is unsupported")
                            })?;
                            Ok(SymValue::F64(self.const_f64(
                                value as f64,
                                module,
                                expr.span,
                            )?))
                        }
                        other => {
                            let value = expect_const_int(other)? as f64;
                            Ok(SymValue::F64(self.const_f64(value, module, expr.span)?))
                        }
                    }
                } else if expr.ty == Type::F32 {
                    if let SymValue::F64(value) = value {
                        let value = self.constant_value_f64(value).ok_or_else(|| {
                            self.error("P004", "runtime f64-to-f32 conversion is unsupported")
                        })?;
                        Ok(SymValue::F32(self.const_f32(
                            value as f32,
                            module,
                            expr.span,
                        )?))
                    } else {
                        Ok(SymValue::F32(self.as_scalar(value, module, expr.span)?))
                    }
                } else {
                    Ok(match value {
                        SymValue::Int(value) => SymValue::Int(value),
                        SymValue::F32(value) => SymValue::Int(SymInt::Runtime(value)),
                        SymValue::Param(proxy) if is_integer_type(&proxy.ty) => SymValue::Int(
                            SymInt::Runtime(self.param_scalar(&proxy, module, expr.span)?),
                        ),
                        other => {
                            return Err(self.error(
                                "P004",
                                format!("unsupported scalar conversion from {other:?}"),
                            ));
                        }
                    })
                }
            }
            TypedExprKind::Neg(inner) => {
                let value = self.eval_expr(inner, module)?;
                match value {
                    SymValue::Int(SymInt::Const(value)) => Ok(SymValue::Int(SymInt::Const(
                        value.checked_neg().ok_or_else(|| {
                            self.error("P004", "integer negation overflow in symbolic body")
                        })?,
                    ))),
                    SymValue::F64(value) => {
                        let value = self.constant_value_f64(value).ok_or_else(|| {
                            self.error("P004", "runtime f64 negation is unsupported")
                        })?;
                        Ok(SymValue::F64(self.const_f64(-value, module, expr.span)?))
                    }
                    other => {
                        let value = self.as_scalar(other, module, expr.span)?;
                        Ok(SymValue::F32(self.neg(value, module, expr.span)?))
                    }
                }
            }
            TypedExprKind::Binary(op, left, right) => {
                let left = self.eval_expr(left, module)?;
                let right = self.eval_expr(right, module)?;
                self.eval_binary(*op, left, right, &expr.ty, module, expr.span)
            }
            TypedExprKind::And(left, right) => {
                let SymValue::Bool(SymBool::Const(left)) = self.eval_expr(left, module)? else {
                    return Err(self.error(
                        "P004",
                        "runtime boolean conjunction is not an explicit material predicate",
                    ));
                };
                if !left {
                    return Ok(SymValue::Bool(SymBool::Const(false)));
                }
                let SymValue::Bool(SymBool::Const(right)) = self.eval_expr(right, module)? else {
                    return Err(self.error(
                        "P004",
                        "runtime boolean conjunction is not an explicit material predicate",
                    ));
                };
                Ok(SymValue::Bool(SymBool::Const(right)))
            }
            TypedExprKind::Or(left, right) => {
                let SymValue::Bool(SymBool::Const(left)) = self.eval_expr(left, module)? else {
                    return Err(self.error(
                        "P004",
                        "runtime boolean conjunction is not an explicit material predicate",
                    ));
                };
                if left {
                    return Ok(SymValue::Bool(SymBool::Const(true)));
                }
                let SymValue::Bool(SymBool::Const(right)) = self.eval_expr(right, module)? else {
                    return Err(self.error(
                        "P004",
                        "runtime boolean conjunction is not an explicit material predicate",
                    ));
                };
                Ok(SymValue::Bool(SymBool::Const(right)))
            }
            TypedExprKind::Not(inner) => match self.eval_expr(inner, module)? {
                SymValue::Bool(SymBool::Const(value)) => Ok(SymValue::Bool(SymBool::Const(!value))),
                other => Err(self.error(
                    "P004",
                    format!("unsupported runtime boolean negation {other:?}"),
                )),
            },
            TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
                self.quota.aggregate(items.len(), &self.call_stack)?;
                let values = items
                    .iter()
                    .map(|item| self.eval_expr(item, module))
                    .collect::<Result<Vec<_>, _>>()?;
                if matches!(expr.kind, TypedExprKind::Tuple(_)) {
                    Ok(SymValue::Struct(values))
                } else {
                    Ok(SymValue::Array(values))
                }
            }
            TypedExprKind::StructLiteral { name, fields } => {
                let canonical = nominal_name(self.programs, module, name);
                if matches!(
                    canonical.as_deref(),
                    Some("core.field::Vec3")
                        | Some("field::Vec3")
                        | Some("core.field::Vec2")
                        | Some("field::Vec2")
                        | Some("core.field::Rgb")
                        | Some("field::Rgb")
                ) {
                    let values = fields
                        .iter()
                        .map(|(_, value)| self.eval_expr(value, module))
                        .collect::<Result<Vec<_>, _>>()?;
                    self.quota.aggregate(values.len(), &self.call_stack)?;
                    return match canonical.as_deref() {
                        Some("core.field::Vec3") | Some("field::Vec3") => {
                            let values: [ScalarId; 3] = values
                                .into_iter()
                                .map(|value| self.as_scalar(value, module, expr.span))
                                .collect::<Result<Vec<_>, _>>()?
                                .try_into()
                                .map_err(|_| self.error("P004", "Vec3 must have three fields"))?;
                            Ok(SymValue::Vec3(SymVec3 {
                                values,
                                transforms: Vec::new(),
                                coordinate_provenance: self.is_renderer_coordinate_triplet(values),
                            }))
                        }
                        Some("core.field::Vec2") | Some("field::Vec2") => {
                            let values: [ScalarId; 2] = values
                                .into_iter()
                                .map(|value| self.as_scalar(value, module, expr.span))
                                .collect::<Result<Vec<_>, _>>()?
                                .try_into()
                                .map_err(|_| self.error("P004", "Vec2 must have two fields"))?;
                            Ok(SymValue::Vec2(values))
                        }
                        Some("core.field::Rgb") | Some("field::Rgb") => {
                            let values: [ScalarId; 3] = values
                                .into_iter()
                                .map(|value| self.as_scalar(value, module, expr.span))
                                .collect::<Result<Vec<_>, _>>()?
                                .try_into()
                                .map_err(|_| self.error("P004", "Rgb must have three fields"))?;
                            Ok(SymValue::Rgb(values))
                        }
                        _ => unreachable!("closed field vector names"),
                    };
                }
                let (default_module, strukt) =
                    super::typed_struct_decl(self.programs, module, &expr.ty)
                        .ok_or_else(|| self.error("P004", format!("unknown struct `{name}`")))?;
                let default_module = default_module.to_string();
                let strukt = strukt.clone();
                let mut slots = vec![None; strukt.fields.len()];
                for (field, value) in fields {
                    let index = strukt
                        .fields
                        .iter()
                        .position(|candidate| candidate == field)
                        .ok_or_else(|| {
                            self.error("P004", format!("unknown struct field `{field}`"))
                        })?;
                    slots[index] = Some(self.eval_expr(value, module)?);
                }
                for (index, field) in strukt.fields.iter().enumerate() {
                    if slots[index].is_some() {
                        continue;
                    }
                    let default = strukt.field_defaults.get(field).ok_or_else(|| {
                        self.error(
                            "P004",
                            format!("struct field `{field}` has neither a value nor a default"),
                        )
                    })?;
                    let saved_scopes = std::mem::take(&mut self.scopes);
                    self.scopes.push(BTreeMap::new());
                    let value = self.eval_expr(default, &default_module);
                    self.scopes = saved_scopes;
                    slots[index] = Some(value?);
                }
                let values = slots
                    .into_iter()
                    .map(|value| {
                        value.ok_or_else(|| self.error("P004", "missing symbolic struct field"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                self.quota.aggregate(values.len(), &self.call_stack)?;
                Ok(SymValue::Struct(values))
            }
            TypedExprKind::EnumConstruct {
                enum_name,
                variant,
                args,
            } => {
                let identity = enum_identity(self.programs, module, enum_name, variant);
                let payload = required_args(self.eval_args(args, module)?, || {
                    self.error("P004", "enum construction has a defaulted payload")
                })?;
                Ok(SymValue::Enum(identity, payload))
            }
            TypedExprKind::Call {
                callee,
                receiver,
                args,
            } => {
                let spelling = callee.spelling();
                let receiver = receiver
                    .as_deref()
                    .map(|value| self.eval_expr(value, module))
                    .transpose()?;
                let args = self.eval_args(args, module)?;
                self.eval_call(&spelling, receiver, args, module, expr.span)
            }
            TypedExprKind::OpCall(callee, left, right) => {
                let left = self.eval_expr(left, module)?;
                let right = self.eval_expr(right, module)?;
                self.eval_call(
                    &callee.spelling(),
                    Some(left),
                    vec![Some(right)],
                    module,
                    expr.span,
                )
            }
            TypedExprKind::Intrinsic {
                key,
                receiver,
                args,
                ..
            } => {
                let receiver = receiver
                    .as_deref()
                    .map(|value| self.eval_expr(value, module))
                    .transpose()?;
                let args = args
                    .iter()
                    .map(|(_, value)| self.eval_expr(value, module))
                    .collect::<Result<Vec<_>, _>>()?;
                if key.ends_with(".to") || key == "to" {
                    let value = receiver
                        .or_else(|| args.first().cloned())
                        .ok_or_else(|| self.error("P004", "scalar conversion lacks a value"))?;
                    if expr.ty == Type::F64 {
                        match value {
                            SymValue::F64(value) => Ok(SymValue::F64(value)),
                            SymValue::F32(value) => {
                                let value = self.constant_value(value).ok_or_else(|| {
                                    self.error(
                                        "P004",
                                        "runtime f32-to-f64 conversion is unsupported",
                                    )
                                })?;
                                Ok(SymValue::F64(self.const_f64(
                                    value as f64,
                                    module,
                                    expr.span,
                                )?))
                            }
                            other => Ok(SymValue::F64(self.const_f64(
                                expect_const_int(other)? as f64,
                                module,
                                expr.span,
                            )?)),
                        }
                    } else if expr.ty == Type::F32 {
                        match value {
                            SymValue::F64(value) => {
                                let value = self.constant_value_f64(value).ok_or_else(|| {
                                    self.error(
                                        "P004",
                                        "runtime f64-to-f32 conversion is unsupported",
                                    )
                                })?;
                                Ok(SymValue::F32(self.const_f32(
                                    value as f32,
                                    module,
                                    expr.span,
                                )?))
                            }
                            other => Ok(SymValue::F32(self.as_scalar(other, module, expr.span)?)),
                        }
                    } else {
                        Ok(value)
                    }
                } else {
                    Err(self.error("P004", format!("unsupported renderer intrinsic `{key}`")))
                }
            }
            TypedExprKind::Take(inner) => self.eval_expr(inner, module),
            TypedExprKind::Is(value, pattern) => {
                let value = self.eval_expr(value, module)?;
                Ok(SymValue::Bool(SymBool::Const(
                    self.pattern_matches_comptime(&value, pattern, module)?,
                )))
            }
            TypedExprKind::FnRef(_)
            | TypedExprKind::Static(_)
            | TypedExprKind::Str(_)
            | TypedExprKind::BStr(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::BitNot(_)
            | TypedExprKind::Try(_, _)
            | TypedExprKind::CallValue(_, _)
            | TypedExprKind::Closure { .. }
            | TypedExprKind::Panic(_)
            | TypedExprKind::PoolName(_)
            | TypedExprKind::Await(_)
            | TypedExprKind::Send(_)
            | TypedExprKind::GroupChild(_) => Err(self.error(
                "P004",
                format!(
                    "unsupported renderer expression `{}`",
                    expr_kind_name(&expr.kind)
                ),
            )),
        }
    }

    fn eval_args(
        &mut self,
        args: &[TypedCallArg],
        module: &str,
    ) -> Result<Vec<Option<SymValue>>, String> {
        args.iter()
            .map(|arg| {
                arg.value
                    .as_ref()
                    .map(|value| self.eval_expr(value, module))
                    .transpose()
            })
            .collect()
    }

    fn index_value(
        &mut self,
        base: SymValue,
        index: SymValue,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        let index = match index {
            SymValue::Param(proxy) if is_integer_type(&proxy.ty) => {
                SymValue::Int(SymInt::Runtime(self.param_scalar(&proxy, module, span)?))
            }
            value => value,
        };
        match (base, index) {
            (SymValue::Array(values), SymValue::Int(SymInt::Const(index))) => values
                .get(usize::try_from(index).map_err(|_| {
                    self.error("P004", format!("negative renderer array index {index}"))
                })?)
                .cloned()
                .ok_or_else(|| {
                    self.error("P004", format!("renderer array index {index} is invalid"))
                }),
            (SymValue::Param(proxy), SymValue::Int(SymInt::Const(index))) => {
                let Type::Array(element, length) = proxy.ty.clone() else {
                    return Err(self.error("P004", "index base is not an array"));
                };
                let extent = crate::sema::bodies::literal_array_len(&length)
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| self.error("P004", "parameter array extent is not finite"))?;
                let index = usize::try_from(index)
                    .ok()
                    .filter(|index| *index < extent)
                    .ok_or_else(|| self.error("P004", "parameter array index is out of bounds"))?;
                let mut result = proxy;
                result.path.push(index);
                result.ty = (*element).clone();
                Ok(SymValue::Param(result))
            }
            (SymValue::Param(proxy), SymValue::Int(SymInt::Runtime(index))) => {
                let Type::Array(element, length) = proxy.ty.clone() else {
                    return Err(self.error("P004", "dynamic index base is not an array"));
                };
                let extent = crate::sema::bodies::literal_array_len(&length)
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| self.error("P004", "dynamic parameter extent is not finite"))?;
                self.quota.aggregate(extent, &self.call_stack)?;
                let values = (0..extent)
                    .map(|alternative| {
                        let mut value = proxy.clone();
                        value.path.push(alternative);
                        value.ty = (*element).clone();
                        value
                    })
                    .collect();
                Ok(SymValue::ParamAlternatives { values, index })
            }
            (other, index) => Err(self.error(
                "P004",
                format!("unsupported symbolic indexing base={other:?} index={index:?} at {span:?}"),
            )),
        }
    }

    fn eval_binary(
        &mut self,
        op: BinOp,
        left: SymValue,
        right: SymValue,
        result_type: &Type,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        let runtime_integer = |value: &SymValue| match value {
            SymValue::Int(SymInt::Runtime(_)) => true,
            SymValue::Param(proxy) => is_integer_type(&proxy.ty),
            SymValue::ParamAlternatives { values, .. } => {
                values.iter().any(|proxy| is_integer_type(&proxy.ty))
            }
            _ => false,
        };
        if runtime_integer(&left) || runtime_integer(&right) {
            return Err(self.error(
                "P004",
                "runtime integer operations must be rejected during renderer legality checking",
            ));
        }
        if let (SymValue::F64(a), SymValue::F64(b)) = (&left, &right) {
            let denominator = *b;
            let a = self
                .constant_value_f64(*a)
                .ok_or_else(|| self.error("P004", "runtime f64 arithmetic is unsupported"))?;
            let b = self
                .constant_value_f64(*b)
                .ok_or_else(|| self.error("P004", "runtime f64 arithmetic is unsupported"))?;
            if matches!(op, BinOp::Div) {
                self.obligations.push(PendingObligation::Scalar(
                    ProofObligation::DenominatorNonZero { denominator },
                ));
            }
            let value = match op {
                BinOp::Add | BinOp::AddW => a + b,
                BinOp::Sub | BinOp::SubW => a - b,
                BinOp::Mul | BinOp::MulW => a * b,
                BinOp::Div => a / b,
                BinOp::Rem => a % b,
                BinOp::Lt => return Ok(SymValue::Bool(SymBool::Const(a < b))),
                BinOp::Le => return Ok(SymValue::Bool(SymBool::Const(a <= b))),
                BinOp::Gt => return Ok(SymValue::Bool(SymBool::Const(a > b))),
                BinOp::Ge => return Ok(SymValue::Bool(SymBool::Const(a >= b))),
                BinOp::Eq => return Ok(SymValue::Bool(SymBool::Const(a == b))),
                BinOp::Ne => return Ok(SymValue::Bool(SymBool::Const(a != b))),
                _ => {
                    return Err(self.error(
                        "P004",
                        format!("unsupported f64 operator `{}`", op.as_str()),
                    ));
                }
            };
            return Ok(SymValue::F64(self.const_f64(value, module, span)?));
        }
        if let (SymValue::Int(SymInt::Const(a)), SymValue::Int(SymInt::Const(b))) = (&left, &right)
        {
            let result = match op {
                BinOp::Add => {
                    checked_int(*a, *b, result_type, i128::checked_add).map(SymInt::Const)
                }
                BinOp::Sub => {
                    checked_int(*a, *b, result_type, i128::checked_sub).map(SymInt::Const)
                }
                BinOp::Mul => {
                    checked_int(*a, *b, result_type, i128::checked_mul).map(SymInt::Const)
                }
                BinOp::AddW => {
                    wrapping_int(*a, *b, result_type, i128::wrapping_add).map(SymInt::Const)
                }
                BinOp::SubW => {
                    wrapping_int(*a, *b, result_type, i128::wrapping_sub).map(SymInt::Const)
                }
                BinOp::MulW => {
                    wrapping_int(*a, *b, result_type, i128::wrapping_mul).map(SymInt::Const)
                }
                BinOp::Div => a
                    .checked_div(*b)
                    .filter(|value| int_fits(*value, result_type))
                    .map(SymInt::Const),
                BinOp::Rem => a
                    .checked_rem(*b)
                    .filter(|value| int_fits(*value, result_type))
                    .map(SymInt::Const),
                BinOp::Lt => return Ok(SymValue::Bool(SymBool::Const(a < b))),
                BinOp::Le => return Ok(SymValue::Bool(SymBool::Const(a <= b))),
                BinOp::Gt => return Ok(SymValue::Bool(SymBool::Const(a > b))),
                BinOp::Ge => return Ok(SymValue::Bool(SymBool::Const(a >= b))),
                BinOp::Eq => return Ok(SymValue::Bool(SymBool::Const(a == b))),
                BinOp::Ne => return Ok(SymValue::Bool(SymBool::Const(a != b))),
                BinOp::BitAnd => Some(SymInt::Const(a & b)),
                BinOp::BitOr => Some(SymInt::Const(a | b)),
                BinOp::BitXor => Some(SymInt::Const(a ^ b)),
                BinOp::Shl => u32::try_from(*b)
                    .ok()
                    .and_then(|shift| a.checked_shl(shift))
                    .filter(|value| int_fits(*value, result_type))
                    .map(SymInt::Const),
                BinOp::Shr => u32::try_from(*b)
                    .ok()
                    .and_then(|shift| a.checked_shr(shift))
                    .map(SymInt::Const),
            };
            return result.map(SymValue::Int).ok_or_else(|| {
                self.error("P004", "checked integer arithmetic failed in renderer body")
            });
        }
        let left = self.as_scalar(left, module, span)?;
        let right = self.as_scalar(right, module, span)?;
        self.binary_scalar(op, left, right, module, span)
    }

    fn eval_call(
        &mut self,
        spelling: &str,
        receiver: Option<SymValue>,
        args: Vec<Option<SymValue>>,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        let resolved = called_function(self.programs, module, spelling);
        if let Some(function) = &resolved
            && super::is_core_scalar_function(function)
            && let Some(intrinsic) = super::scalar::classify_intrinsic(&function.decl_name)
        {
            return self.eval_scalar_intrinsic(
                intrinsic,
                required_args(args, || {
                    self.error("P004", "scalar intrinsic has a defaulted argument")
                })?,
                module,
                span,
            );
        }
        if let Some(function) = &resolved
            && super::is_core_field_function(function)
        {
            let intrinsic =
                super::field_intrinsics::classify(&function.decl_name).ok_or_else(|| {
                self.error(
                    "P004",
                    format!(
                        "field operation `{}` is in the typed surface but lacks a symbolic classifier",
                        function.decl_name
                    ),
                )
            })?;
            return self.eval_field_intrinsic(
                intrinsic,
                required_args(args, || {
                    self.error("P004", "field intrinsic has a defaulted argument")
                })?,
                module,
                span,
            );
        }
        if super::is_core_material_constructor(self.programs, module, spelling) {
            let member = call_base(spelling)
                .rsplit_once('.')
                .map(|(_, member)| member)
                .unwrap_or(call_base(spelling));
            let intrinsic = super::material_intrinsics::classify(member).ok_or_else(|| {
                self.error(
                    "P004",
                    format!("material constructor `{member}` lacks a symbolic classifier"),
                )
            })?;
            return self.eval_material_constructor(intrinsic, args, module, span);
        }
        let base = call_base(spelling);
        if resolved
            .as_ref()
            .is_some_and(super::is_core_field_vector_method)
        {
            let mut all = Vec::new();
            all.push(receiver.ok_or_else(|| {
                self.error("P004", format!("vector method `{base}` lacks a receiver"))
            })?);
            all.extend(required_args(args, || {
                self.error("P004", "vector method has a defaulted argument")
            })?);
            return self.eval_vec_method(base, all, module, span);
        }
        if resolved
            .as_ref()
            .is_some_and(super::is_core_field_value_method)
        {
            let left = expect_field(
                receiver.ok_or_else(|| self.error("P004", "`Field.union` lacks a receiver"))?,
            )?;
            let right = expect_field(
                required_args(args, || {
                    self.error("P004", "`Field.union` has a defaulted argument")
                })?
                .into_iter()
                .next()
                .ok_or_else(|| self.error("P004", "`Field.union` lacks an argument"))?,
            )?;
            return Ok(SymValue::Field(self.field_binary(
                FieldIntrinsic::Union,
                left,
                right,
                module,
                span,
            )?));
        }
        let function = resolved.ok_or_else(|| {
            self.error(
                "P004",
                format!("renderer operation `{spelling}` has no canonical checked callee"),
            )
        })?;
        let mut values = Vec::new();
        if let Some(receiver) = receiver {
            values.push(Some(receiver));
        }
        values.extend(args);
        self.eval_function(function, values, module, span)
    }

    fn eval_vec_method(
        &mut self,
        name: &str,
        args: Vec<SymValue>,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        let mut args = args.into_iter();
        let left = self.as_vec3(
            args.next()
                .ok_or_else(|| self.error("P004", "vector method lacks receiver"))?,
            module,
            span,
        )?;
        let right = self.as_vec3(
            args.next()
                .ok_or_else(|| self.error("P004", "vector method lacks argument"))?,
            module,
            span,
        )?;
        let right_dependency = self.vec3_dependency(&right.values)?;
        if matches!(
            right_dependency,
            Dependency::Coordinate
                | Dependency::CoordinateAndParameter
                | Dependency::CoordinateAndSurface
                | Dependency::CoordinateParameterAndSurface
        ) {
            return Err(self.error(
                "P004",
                "Vec3 coordinate methods require the coordinate vector as the receiver and a coordinate-free argument",
            ));
        }
        let subtract = name.ends_with(".subtract");
        let mut values = [ScalarId(0); 3];
        for component in 0..3 {
            values[component] = if subtract {
                self.sub(
                    left.values[component],
                    right.values[component],
                    module,
                    span,
                )?
            } else {
                self.add(
                    left.values[component],
                    right.values[component],
                    module,
                    span,
                )?
            };
        }
        let left_has_coordinate = left.coordinate_provenance;
        let mut transforms = left.transforms;
        if left_has_coordinate && !self.is_zero_vec3(right.values) {
            let translation = if subtract {
                right.values
            } else {
                [
                    self.neg(right.values[0], module, span)?,
                    self.neg(right.values[1], module, span)?,
                    self.neg(right.values[2], module, span)?,
                ]
            };
            transforms.push(LocatedCoordTransform {
                kind: CoordTransform::Translate(translation),
                origin: self.origin(module, span),
            });
        }
        Ok(SymValue::Vec3(SymVec3 {
            values,
            transforms,
            coordinate_provenance: left_has_coordinate,
        }))
    }

    fn eval_scalar_intrinsic(
        &mut self,
        intrinsic: ScalarIntrinsic,
        args: Vec<SymValue>,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        match intrinsic {
            ScalarIntrinsic::Sqrt
            | ScalarIntrinsic::Rsqrt
            | ScalarIntrinsic::Sin
            | ScalarIntrinsic::Cos => {
                let value = self.as_scalar(args[0].clone(), module, span)?;
                let op = match intrinsic {
                    ScalarIntrinsic::Sqrt => ScalarOp::Sqrt(value, SemanticOpId::SqrtF32V1),
                    ScalarIntrinsic::Rsqrt => ScalarOp::Rsqrt(value, SemanticOpId::RsqrtF32V1),
                    ScalarIntrinsic::Sin => {
                        ScalarOp::SinRestricted(value, SemanticOpId::SinRestrictedF32V1)
                    }
                    ScalarIntrinsic::Cos => {
                        ScalarOp::CosRestricted(value, SemanticOpId::CosRestrictedF32V1)
                    }
                    _ => unreachable!("closed unary scalar intrinsic"),
                };
                let result = self.scalar_node(op, self.scalar_dependency(value)?, module, span)?;
                if matches!(intrinsic, ScalarIntrinsic::Sin | ScalarIntrinsic::Cos)
                    && self.constant_value(result).is_none()
                {
                    self.obligations.push(PendingObligation::Scalar(
                        ProofObligation::RestrictedTrigDomain { argument: value },
                    ));
                }
                Ok(SymValue::F32(result))
            }
            ScalarIntrinsic::Dot3 => {
                let a = self.as_vec3(args[0].clone(), module, span)?.values;
                let b = self.as_vec3(args[1].clone(), module, span)?.values;
                Ok(SymValue::F32(self.dot3(a, b, module, span)?))
            }
            ScalarIntrinsic::Cross3 => {
                let a = self.as_vec3(args[0].clone(), module, span)?.values;
                let b = self.as_vec3(args[1].clone(), module, span)?.values;
                let dependency = a.into_iter().chain(b).try_fold(
                    Dependency::Constant,
                    |dependency, value| {
                        Ok::<_, String>(dependency.combine(self.scalar_dependency(value)?))
                    },
                )?;
                let mut values = [ScalarId(0); 3];
                for (component, slot) in values.iter_mut().enumerate() {
                    *slot = self.scalar_node(
                        ScalarOp::Cross3Component {
                            component: component as u8,
                            a,
                            b,
                        },
                        dependency,
                        module,
                        span,
                    )?;
                }
                Ok(SymValue::Vec3(SymVec3 {
                    values,
                    transforms: Vec::new(),
                    coordinate_provenance: false,
                }))
            }
            ScalarIntrinsic::Length2 => {
                let value = self.as_vec2(args[0].clone(), module, span)?;
                Ok(SymValue::F32(self.length2(value, module, span)?))
            }
            ScalarIntrinsic::Length3 => {
                let value = self.as_vec3(args[0].clone(), module, span)?.values;
                Ok(SymValue::F32(self.length3(value, module, span)?))
            }
            ScalarIntrinsic::Normalize3 => {
                let value = self.as_vec3(args[0].clone(), module, span)?.values;
                let dependency =
                    value
                        .into_iter()
                        .try_fold(Dependency::Constant, |dependency, value| {
                            Ok::<_, String>(dependency.combine(self.scalar_dependency(value)?))
                        })?;
                let mut values = [ScalarId(0); 3];
                for (component, slot) in values.iter_mut().enumerate() {
                    *slot = self.scalar_node(
                        ScalarOp::Normalize3Component {
                            component: component as u8,
                            value,
                            semantic: SemanticOpId::Normalize3F32V1,
                        },
                        dependency,
                        module,
                        span,
                    )?;
                }
                Ok(SymValue::Vec3(SymVec3 {
                    values,
                    transforms: Vec::new(),
                    coordinate_provenance: false,
                }))
            }
        }
    }

    fn eval_field_intrinsic(
        &mut self,
        intrinsic: FieldIntrinsic,
        args: Vec<SymValue>,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        match intrinsic {
            FieldIntrinsic::Translate => {
                let point = self.as_vec3(args[0].clone(), module, span)?;
                if !point.coordinate_provenance {
                    return Err(self.error(
                        "P004",
                        "coordinate transform input must be derived from the renderer coordinate",
                    ));
                }
                let by = self.as_vec3(args[1].clone(), module, span)?;
                self.require_coordinate_free_vec3(by.values, "translate", "by")?;
                let mut values = [ScalarId(0); 3];
                for component in 0..3 {
                    values[component] =
                        self.sub(point.values[component], by.values[component], module, span)?;
                }
                let mut transforms = point.transforms;
                if !self.is_zero_vec3(by.values) {
                    transforms.push(LocatedCoordTransform {
                        kind: CoordTransform::Translate(by.values),
                        origin: self.origin(module, span),
                    });
                }
                Ok(SymValue::Vec3(SymVec3 {
                    values,
                    transforms,
                    coordinate_provenance: true,
                }))
            }
            FieldIntrinsic::Rotate | FieldIntrinsic::RigidTransform => {
                let point = self.as_vec3(args[0].clone(), module, span)?;
                if !point.coordinate_provenance {
                    return Err(self.error(
                        "P004",
                        "coordinate transform input must be derived from the renderer coordinate",
                    ));
                }
                let (translation, row_offset) = if intrinsic == FieldIntrinsic::RigidTransform {
                    (Some(self.as_vec3(args[1].clone(), module, span)?.values), 2)
                } else {
                    (None, 1)
                };
                let row_x = self.as_vec3(args[row_offset].clone(), module, span)?.values;
                let row_y = self
                    .as_vec3(args[row_offset + 1].clone(), module, span)?
                    .values;
                let row_z = self
                    .as_vec3(args[row_offset + 2].clone(), module, span)?
                    .values;
                if let Some(translation) = translation {
                    self.require_coordinate_free_vec3(
                        translation,
                        "rigid_transform",
                        "translation",
                    )?;
                }
                self.require_coordinate_free_vec3(row_x, "rotate", "row_x")?;
                self.require_coordinate_free_vec3(row_y, "rotate", "row_y")?;
                self.require_coordinate_free_vec3(row_z, "rotate", "row_z")?;
                let source = if let Some(translation) = translation {
                    let mut shifted = [ScalarId(0); 3];
                    for component in 0..3 {
                        shifted[component] = self.sub(
                            point.values[component],
                            translation[component],
                            module,
                            span,
                        )?;
                    }
                    shifted
                } else {
                    point.values
                };
                let values = [
                    self.dot3(row_x, source, module, span)?,
                    self.dot3(row_y, source, module, span)?,
                    self.dot3(row_z, source, module, span)?,
                ];
                let mut transforms = point.transforms;
                let identity_rotation = self.is_identity_rows(row_x, row_y, row_z);
                match (translation, identity_rotation) {
                    (Some(translation), false) => transforms.push(LocatedCoordTransform {
                        kind: CoordTransform::Rigid {
                            translation,
                            row_x,
                            row_y,
                            row_z,
                        },
                        origin: self.origin(module, span),
                    }),
                    (Some(translation), true) if !self.is_zero_vec3(translation) => {
                        transforms.push(LocatedCoordTransform {
                            kind: CoordTransform::Translate(translation),
                            origin: self.origin(module, span),
                        });
                    }
                    (None, false) => transforms.push(LocatedCoordTransform {
                        kind: CoordTransform::Rotate {
                            row_x,
                            row_y,
                            row_z,
                        },
                        origin: self.origin(module, span),
                    }),
                    (Some(_), true) | (None, true) => {}
                }
                Ok(SymValue::Vec3(SymVec3 {
                    values,
                    transforms,
                    coordinate_provenance: true,
                }))
            }
            FieldIntrinsic::FiniteRepeatX
            | FieldIntrinsic::FiniteRepeatY
            | FieldIntrinsic::FiniteRepeatZ => self.eval_repeat(intrinsic, args, module, span),
            FieldIntrinsic::UniformScale => {
                let field = expect_field(args[0].clone())?;
                let scale = self.as_scalar(args[1].clone(), module, span)?;
                self.require_coordinate_free_scalar(scale, "uniform_scale", "scale")?;
                if self.is_const_f32(scale, 1.0) {
                    return Ok(SymValue::Field(field));
                }
                let id = self.uniform_scale_field(field, scale, module, span)?;
                Ok(SymValue::Field(id))
            }
            FieldIntrinsic::Union | FieldIntrinsic::Intersection | FieldIntrinsic::Subtract => {
                let left = expect_field(args[0].clone())?;
                let right = expect_field(args[1].clone())?;
                Ok(SymValue::Field(
                    self.field_binary(intrinsic, left, right, module, span)?,
                ))
            }
            FieldIntrinsic::SmoothUnion
            | FieldIntrinsic::SmoothIntersection
            | FieldIntrinsic::SmoothSubtract => {
                let left = expect_field(args[0].clone())?;
                let right = expect_field(args[1].clone())?;
                let k = self.as_scalar(args[2].clone(), module, span)?;
                Ok(SymValue::Field(
                    self.field_smooth(intrinsic, left, right, k, module, span)?,
                ))
            }
            FieldIntrinsic::Mark => {
                let child = expect_field(args[0].clone())?;
                let object_source = expect_identity(args[1].clone())?;
                let material_source = expect_identity(args[2].clone())?;
                let scalar = self.fields.get(child)?.scalar_value;
                Ok(SymValue::Field(self.field_node(
                    FieldKind::Mark {
                        child,
                        object_source,
                        material_source,
                    },
                    scalar,
                    module,
                    span,
                )?))
            }
            FieldIntrinsic::SinusoidalDisplace => self.eval_displace(args, module, span),
            FieldIntrinsic::Plane
            | FieldIntrinsic::Sphere
            | FieldIntrinsic::Box
            | FieldIntrinsic::RoundBox
            | FieldIntrinsic::Capsule
            | FieldIntrinsic::FiniteCylinder
            | FieldIntrinsic::FiniteCone
            | FieldIntrinsic::Torus => self.eval_primitive(intrinsic, args, module, span),
        }
    }

    fn eval_repeat(
        &mut self,
        intrinsic: FieldIntrinsic,
        args: Vec<SymValue>,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        let point = self.as_vec3(args[0].clone(), module, span)?;
        if !point.coordinate_provenance {
            return Err(self.error(
                "P004",
                "finite-repeat input must be derived from the renderer coordinate",
            ));
        }
        let first = expect_const_int(args[1].clone())?;
        let count = expect_const_int(args[2].clone())?;
        let first = i32::try_from(first)
            .map_err(|_| self.error("P004", "repeat first index does not fit i32"))?;
        let count = u32::try_from(count)
            .map_err(|_| self.error("P004", "repeat count does not fit u32"))?;
        if count == 0 {
            return Err(self.error("P004", "finite repeat count must be positive"));
        }
        let period = self.as_scalar(args[3].clone(), module, span)?;
        self.require_coordinate_free_scalar(period, "finite_repeat", "period")?;
        let axis = match intrinsic {
            FieldIntrinsic::FiniteRepeatX => Axis::X,
            FieldIntrinsic::FiniteRepeatY => Axis::Y,
            FieldIntrinsic::FiniteRepeatZ => Axis::Z,
            _ => unreachable!("closed repeat names"),
        };
        let component = match axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        };
        let first_scalar = self.const_f32(first as f32, module, span)?;
        let mut best = None;
        for index in 0..count {
            let instance = if index == 0 {
                first_scalar
            } else {
                let index = self.const_f32(index as f32, module, span)?;
                self.add(first_scalar, index, module, span)?
            };
            let delta = self.mul(instance, period, module, span)?;
            let candidate = self.sub(point.values[component], delta, module, span)?;
            best = Some(match best {
                None => candidate,
                Some(prior) => {
                    let abs_candidate = self.abs(candidate, module, span)?;
                    let abs_prior = self.abs(prior, module, span)?;
                    let predicate = expect_runtime_bool(self.binary_scalar(
                        BinOp::Lt,
                        abs_candidate,
                        abs_prior,
                        module,
                        span,
                    )?)?;
                    let dependency = self
                        .scalar_dependency(predicate)?
                        .combine(self.scalar_dependency(candidate)?)
                        .combine(self.scalar_dependency(prior)?);
                    self.scalar_node(
                        ScalarOp::Select {
                            predicate,
                            a: candidate,
                            b: prior,
                        },
                        dependency,
                        module,
                        span,
                    )?
                }
            });
        }
        let mut values = point.values;
        values[component] = best.expect("positive repeat count");
        let mut transforms = point.transforms;
        transforms.push(LocatedCoordTransform {
            kind: CoordTransform::Repeat {
                axis,
                first,
                count,
                period,
            },
            origin: self.origin(module, span),
        });
        Ok(SymValue::Vec3(SymVec3 {
            values,
            transforms,
            coordinate_provenance: true,
        }))
    }

    fn eval_primitive(
        &mut self,
        intrinsic: FieldIntrinsic,
        args: Vec<SymValue>,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        let point = self.as_vec3(args[0].clone(), module, span)?;
        if !point.coordinate_provenance {
            return Err(self.error(
                "P004",
                "field primitive point argument must be derived from the renderer coordinate",
            ));
        }
        let zero = self.const_f32(0.0, module, span)?;
        let axis_y = [zero, self.const_f32(1.0, module, span)?, zero];
        let (primitive, scalar) = match intrinsic {
            FieldIntrinsic::Plane => {
                let normal = self.as_vec3(args[1].clone(), module, span)?.values;
                let offset = self.as_scalar(args[2].clone(), module, span)?;
                self.require_coordinate_free_vec3(normal, "plane", "normal")?;
                self.require_coordinate_free_scalar(offset, "plane", "offset")?;
                let dot = self.dot3(point.values, normal, module, span)?;
                (
                    Primitive::Plane { normal, offset },
                    self.add(dot, offset, module, span)?,
                )
            }
            FieldIntrinsic::Sphere => {
                let center = self.as_vec3(args[1].clone(), module, span)?.values;
                let radius = self.as_scalar(args[2].clone(), module, span)?;
                self.require_coordinate_free_vec3(center, "sphere", "center")?;
                self.require_coordinate_free_scalar(radius, "sphere", "radius")?;
                let q = self.vec_sub(point.values, center, module, span)?;
                let length = self.length3(q, module, span)?;
                (
                    Primitive::Sphere { center, radius },
                    self.sub(length, radius, module, span)?,
                )
            }
            FieldIntrinsic::Box | FieldIntrinsic::RoundBox => {
                let half = self.as_vec3(args[1].clone(), module, span)?.values;
                let operation = if intrinsic == FieldIntrinsic::RoundBox {
                    "round_box"
                } else {
                    "box"
                };
                self.require_coordinate_free_vec3(half, operation, "half")?;
                let center = [zero; 3];
                let base = self.box_scalar(point.values, half, module, span)?;
                if intrinsic == FieldIntrinsic::RoundBox {
                    let radius = self.as_scalar(args[2].clone(), module, span)?;
                    self.require_coordinate_free_scalar(radius, operation, "radius")?;
                    (
                        Primitive::RoundBox {
                            center,
                            half,
                            radius,
                        },
                        self.sub(base, radius, module, span)?,
                    )
                } else {
                    (Primitive::Box { center, half }, base)
                }
            }
            FieldIntrinsic::Capsule => {
                let a = self.as_vec3(args[1].clone(), module, span)?.values;
                let b = self.as_vec3(args[2].clone(), module, span)?.values;
                let radius = self.as_scalar(args[3].clone(), module, span)?;
                self.require_coordinate_free_vec3(a, "capsule", "a")?;
                self.require_coordinate_free_vec3(b, "capsule", "b")?;
                self.require_coordinate_free_scalar(radius, "capsule", "radius")?;
                let scalar = self.capsule_scalar(point.values, a, b, radius, module, span)?;
                if a == b {
                    (Primitive::Sphere { center: a, radius }, scalar)
                } else {
                    (Primitive::Capsule { a, b, radius }, scalar)
                }
            }
            FieldIntrinsic::FiniteCylinder => {
                let radius = self.as_scalar(args[1].clone(), module, span)?;
                let half = self.as_scalar(args[2].clone(), module, span)?;
                self.require_coordinate_free_scalar(radius, "finite_cylinder", "radius")?;
                self.require_coordinate_free_scalar(half, "finite_cylinder", "half_height")?;
                let neg_half = self.neg(half, module, span)?;
                let a = [zero, neg_half, zero];
                let b = [zero, half, zero];
                let radial_vec = [point.values[0], point.values[2]];
                let radial_length = self.length2(radial_vec, module, span)?;
                let radial = self.sub(radial_length, radius, module, span)?;
                let abs_y = self.abs(point.values[1], module, span)?;
                let axial = self.sub(abs_y, half, module, span)?;
                let radial_outside = self.max(radial, zero, module, span)?;
                let axial_outside = self.max(axial, zero, module, span)?;
                let outside = self.length2([radial_outside, axial_outside], module, span)?;
                let inside_max = self.max(radial, axial, module, span)?;
                let inside = self.min(inside_max, zero, module, span)?;
                (
                    Primitive::FiniteCylinder { a, b, radius },
                    self.add(outside, inside, module, span)?,
                )
            }
            FieldIntrinsic::FiniteCone => {
                let radius = self.as_scalar(args[1].clone(), module, span)?;
                let half = self.as_scalar(args[2].clone(), module, span)?;
                self.require_coordinate_free_scalar(radius, "finite_cone", "radius")?;
                self.require_coordinate_free_scalar(half, "finite_cone", "half_height")?;
                let neg_half = self.neg(half, module, span)?;
                let a = [zero, neg_half, zero];
                let b = [zero, half, zero];
                let two_half = self.add(half, half, module, span)?;
                let shifted_y = self.add(point.values[1], half, module, span)?;
                let y = self.scalar_node(
                    ScalarOp::Clamp {
                        value: shifted_y,
                        lo: zero,
                        hi: two_half,
                    },
                    self.scalar_dependency(point.values[1])?
                        .combine(self.scalar_dependency(half)?),
                    module,
                    span,
                )?;
                let one = self.const_f32(1.0, module, span)?;
                let fraction = self.div(y, two_half, module, span)?;
                let remaining = self.sub(one, fraction, module, span)?;
                let allowed = self.mul(radius, remaining, module, span)?;
                let radial_length =
                    self.length2([point.values[0], point.values[2]], module, span)?;
                let radial = self.sub(radial_length, allowed, module, span)?;
                let abs_y = self.abs(point.values[1], module, span)?;
                let axial = self.sub(abs_y, half, module, span)?;
                (
                    Primitive::FiniteCone {
                        a,
                        b,
                        radius_a: radius,
                        radius_b: zero,
                    },
                    self.max(radial, axial, module, span)?,
                )
            }
            FieldIntrinsic::Torus => {
                let major = self.as_scalar(args[1].clone(), module, span)?;
                let minor = self.as_scalar(args[2].clone(), module, span)?;
                self.require_coordinate_free_scalar(major, "torus", "major")?;
                self.require_coordinate_free_scalar(minor, "torus", "minor")?;
                let radial_length =
                    self.length2([point.values[0], point.values[2]], module, span)?;
                let radial = self.sub(radial_length, major, module, span)?;
                let length = self.length2([radial, point.values[1]], module, span)?;
                (
                    Primitive::Torus {
                        center: [zero; 3],
                        axis: axis_y,
                        major,
                        minor,
                    },
                    self.sub(length, minor, module, span)?,
                )
            }
            _ => unreachable!("closed primitive names"),
        };
        let mut field = self.field_node(FieldKind::Primitive(primitive), scalar, module, span)?;
        let layers = self.field_transform_layers(&point.transforms)?;
        for layer in layers.into_iter().rev() {
            let (kind, mut origins) = match layer {
                FieldTransformLayer::Program { transform, origins } => (
                    FieldKind::Transform {
                        child: field,
                        transform,
                    },
                    origins,
                ),
                FieldTransformLayer::Repeat {
                    axis,
                    first,
                    count,
                    period,
                    origin,
                } => (
                    FieldKind::FiniteRepeat {
                        child: field,
                        axis,
                        first,
                        count,
                        period,
                    },
                    vec![origin],
                ),
            };
            let mut origin = origins
                .drain(..1)
                .next()
                .expect("every transform layer retains a source origin");
            for merged in origins {
                origin.merge(&merged);
            }
            field = self.field_node_with_origin(kind, scalar, origin)?;
        }
        Ok(SymValue::Field(field))
    }

    fn field_transform_layers(
        &mut self,
        transforms: &[LocatedCoordTransform],
    ) -> Result<Vec<FieldTransformLayer>, String> {
        let mut layers = Vec::new();
        let mut rigid_group = Vec::new();
        for transform in transforms {
            match &transform.kind {
                CoordTransform::Repeat {
                    axis,
                    first,
                    count,
                    period,
                } => {
                    self.flush_rigid_group(&mut rigid_group, &mut layers)?;
                    layers.push(FieldTransformLayer::Repeat {
                        axis: *axis,
                        first: *first,
                        count: *count,
                        period: *period,
                        origin: transform.origin.clone(),
                    });
                }
                _ => rigid_group.push(transform.clone()),
            }
        }
        self.flush_rigid_group(&mut rigid_group, &mut layers)?;
        Ok(layers)
    }

    fn flush_rigid_group(
        &mut self,
        group: &mut Vec<LocatedCoordTransform>,
        layers: &mut Vec<FieldTransformLayer>,
    ) -> Result<(), String> {
        if group.is_empty() {
            return Ok(());
        }
        let origins = group
            .iter()
            .map(|transform| transform.origin.clone())
            .collect();
        let transform = if group.len() == 1 {
            match group.pop().expect("nonempty rigid group").kind {
                CoordTransform::Translate(by) => TransformProgram::Translate { by },
                CoordTransform::Rotate {
                    row_x,
                    row_y,
                    row_z,
                } => TransformProgram::Rotate {
                    row_x,
                    row_y,
                    row_z,
                },
                CoordTransform::Rigid {
                    translation,
                    row_x,
                    row_y,
                    row_z,
                } => TransformProgram::Rigid {
                    translation,
                    row_x,
                    row_y,
                    row_z,
                },
                CoordTransform::Repeat { .. } => unreachable!("repeat groups flush separately"),
            }
        } else {
            let source = &group[0].origin.primary;
            let composed = self.compose_rigid_transforms(group, &source.module, source.span)?;
            let steps = group
                .iter()
                .map(|transform| match &transform.kind {
                    CoordTransform::Translate(by) => TransformProgram::Translate { by: *by },
                    CoordTransform::Rotate {
                        row_x,
                        row_y,
                        row_z,
                    } => TransformProgram::Rotate {
                        row_x: *row_x,
                        row_y: *row_y,
                        row_z: *row_z,
                    },
                    CoordTransform::Rigid {
                        translation,
                        row_x,
                        row_y,
                        row_z,
                    } => TransformProgram::Rigid {
                        translation: *translation,
                        row_x: *row_x,
                        row_y: *row_y,
                        row_z: *row_z,
                    },
                    CoordTransform::Repeat { .. } => {
                        unreachable!("repeat groups flush separately")
                    }
                })
                .collect();
            group.clear();
            TransformProgram::SourceRigidSequence {
                steps,
                composed: Box::new(composed),
            }
        };
        layers.push(FieldTransformLayer::Program { transform, origins });
        Ok(())
    }

    fn compose_rigid_transforms(
        &mut self,
        transforms: &[LocatedCoordTransform],
        module: &str,
        span: Span,
    ) -> Result<TransformProgram, String> {
        let zero = self.const_f32(0.0, module, span)?;
        let one = self.const_f32(1.0, module, span)?;
        let mut translation = [zero; 3];
        let mut rows = [[one, zero, zero], [zero, one, zero], [zero, zero, one]];
        for transform in transforms {
            let (by, next_rows) = match &transform.kind {
                CoordTransform::Translate(by) => (*by, None),
                CoordTransform::Rotate {
                    row_x,
                    row_y,
                    row_z,
                } => ([zero; 3], Some([*row_x, *row_y, *row_z])),
                CoordTransform::Rigid {
                    translation,
                    row_x,
                    row_y,
                    row_z,
                } => (*translation, Some([*row_x, *row_y, *row_z])),
                CoordTransform::Repeat { .. } => {
                    return Err(
                        "pixels::canonicalize: repeat cannot be composed as a rigid transform"
                            .to_string(),
                    );
                }
            };

            let world_by = [
                self.dot3([rows[0][0], rows[1][0], rows[2][0]], by, module, span)?,
                self.dot3([rows[0][1], rows[1][1], rows[2][1]], by, module, span)?,
                self.dot3([rows[0][2], rows[1][2], rows[2][2]], by, module, span)?,
            ];
            translation = [
                self.add(translation[0], world_by[0], module, span)?,
                self.add(translation[1], world_by[1], module, span)?,
                self.add(translation[2], world_by[2], module, span)?,
            ];

            if let Some(next_rows) = next_rows {
                rows = [
                    [
                        self.dot3(
                            next_rows[0],
                            [rows[0][0], rows[1][0], rows[2][0]],
                            module,
                            span,
                        )?,
                        self.dot3(
                            next_rows[0],
                            [rows[0][1], rows[1][1], rows[2][1]],
                            module,
                            span,
                        )?,
                        self.dot3(
                            next_rows[0],
                            [rows[0][2], rows[1][2], rows[2][2]],
                            module,
                            span,
                        )?,
                    ],
                    [
                        self.dot3(
                            next_rows[1],
                            [rows[0][0], rows[1][0], rows[2][0]],
                            module,
                            span,
                        )?,
                        self.dot3(
                            next_rows[1],
                            [rows[0][1], rows[1][1], rows[2][1]],
                            module,
                            span,
                        )?,
                        self.dot3(
                            next_rows[1],
                            [rows[0][2], rows[1][2], rows[2][2]],
                            module,
                            span,
                        )?,
                    ],
                    [
                        self.dot3(
                            next_rows[2],
                            [rows[0][0], rows[1][0], rows[2][0]],
                            module,
                            span,
                        )?,
                        self.dot3(
                            next_rows[2],
                            [rows[0][1], rows[1][1], rows[2][1]],
                            module,
                            span,
                        )?,
                        self.dot3(
                            next_rows[2],
                            [rows[0][2], rows[1][2], rows[2][2]],
                            module,
                            span,
                        )?,
                    ],
                ];
            }
        }

        let identity = self.is_identity_rows(rows[0], rows[1], rows[2]);
        if identity {
            Ok(TransformProgram::Translate { by: translation })
        } else if self.is_zero_vec3(translation) {
            Ok(TransformProgram::Rotate {
                row_x: rows[0],
                row_y: rows[1],
                row_z: rows[2],
            })
        } else {
            Ok(TransformProgram::Rigid {
                translation,
                row_x: rows[0],
                row_y: rows[1],
                row_z: rows[2],
            })
        }
    }

    fn vec_sub(
        &mut self,
        a: [ScalarId; 3],
        b: [ScalarId; 3],
        module: &str,
        span: Span,
    ) -> Result<[ScalarId; 3], String> {
        Ok([
            self.sub(a[0], b[0], module, span)?,
            self.sub(a[1], b[1], module, span)?,
            self.sub(a[2], b[2], module, span)?,
        ])
    }

    fn box_scalar(
        &mut self,
        point: [ScalarId; 3],
        half: [ScalarId; 3],
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        let zero = self.const_f32(0.0, module, span)?;
        let abs_x = self.abs(point[0], module, span)?;
        let abs_y = self.abs(point[1], module, span)?;
        let abs_z = self.abs(point[2], module, span)?;
        let q = [
            self.sub(abs_x, half[0], module, span)?,
            self.sub(abs_y, half[1], module, span)?,
            self.sub(abs_z, half[2], module, span)?,
        ];
        let outside = [
            self.max(q[0], zero, module, span)?,
            self.max(q[1], zero, module, span)?,
            self.max(q[2], zero, module, span)?,
        ];
        let length = self.length3(outside, module, span)?;
        let yz = self.max(q[1], q[2], module, span)?;
        let xyz = self.max(q[0], yz, module, span)?;
        let inside = self.min(xyz, zero, module, span)?;
        self.add(length, inside, module, span)
    }

    fn capsule_scalar(
        &mut self,
        point: [ScalarId; 3],
        a: [ScalarId; 3],
        b: [ScalarId; 3],
        radius: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        let pa = self.vec_sub(point, a, module, span)?;
        let ba = self.vec_sub(b, a, module, span)?;
        let denom = self.dot3(ba, ba, module, span)?;
        let numerator = self.dot3(pa, ba, module, span)?;
        let zero = self.const_f32(0.0, module, span)?;
        let one = self.const_f32(1.0, module, span)?;
        let h = match self.constant_value(denom) {
            Some(denom) if denom <= 0.0 => zero,
            constant_denom => {
                let predicate = if constant_denom.is_none() {
                    Some(expect_runtime_bool(self.binary_scalar(
                        BinOp::Gt,
                        denom,
                        zero,
                        module,
                        span,
                    )?)?)
                } else {
                    None
                };
                let quotient = if let Some(predicate) = predicate {
                    let dependency = self
                        .scalar_dependency(numerator)?
                        .combine(self.scalar_dependency(denom)?);
                    let quotient = self.scalar_node(
                        ScalarOp::Div(numerator, denom),
                        dependency,
                        module,
                        span,
                    )?;
                    self.obligations.push(PendingObligation::Scalar(
                        ProofObligation::GuardedDenominatorNonZero {
                            denominator: denom,
                            predicate,
                        },
                    ));
                    quotient
                } else {
                    self.div(numerator, denom, module, span)?
                };
                let clamped = self.scalar_node(
                    ScalarOp::Clamp {
                        value: quotient,
                        lo: zero,
                        hi: one,
                    },
                    self.scalar_dependency(quotient)?,
                    module,
                    span,
                )?;
                if let Some(predicate) = predicate {
                    self.scalar_node(
                        ScalarOp::Select {
                            predicate,
                            a: clamped,
                            b: zero,
                        },
                        self.scalar_dependency(clamped)?,
                        module,
                        span,
                    )?
                } else {
                    clamped
                }
            }
        };
        let bah = [
            self.mul(ba[0], h, module, span)?,
            self.mul(ba[1], h, module, span)?,
            self.mul(ba[2], h, module, span)?,
        ];
        let q = [
            self.sub(pa[0], bah[0], module, span)?,
            self.sub(pa[1], bah[1], module, span)?,
            self.sub(pa[2], bah[2], module, span)?,
        ];
        let length = self.length3(q, module, span)?;
        self.sub(length, radius, module, span)
    }

    fn compare_field_structural(
        &mut self,
        left: FieldId,
        right: FieldId,
    ) -> Result<std::cmp::Ordering, String> {
        let left = self.build_structural_key(StructuralSource::Field(left))?;
        let right = self.build_structural_key(StructuralSource::Field(right))?;
        let mut comparisons = BTreeMap::new();
        compare_structural_keys(&self.structural_keys, left, right, &mut comparisons, 0)
    }

    fn build_structural_key(&mut self, source: StructuralSource) -> Result<u32, String> {
        enum Visit {
            Enter(StructuralSource),
            Finish(
                StructuralSource,
                Vec<StructuralToken>,
                Vec<StructuralSource>,
            ),
        }

        let mut stack = vec![Visit::Enter(source)];
        let mut pending = BTreeSet::new();
        while let Some(visit) = stack.pop() {
            match visit {
                Visit::Enter(current) => {
                    if self.cached_structural_key(current).is_some() {
                        continue;
                    }
                    if !pending.insert(current) {
                        return Err(self.error("P014", "structural-key cycle detected"));
                    }
                    if self.structural_keys.len() + pending.len()
                        > self.quota.max_aggregate_elements
                    {
                        return Err(self.error("P014", "structural-key node quota exhausted"));
                    }
                    let (local, children) = match current {
                        StructuralSource::Field(id) => self.field_key_parts(id)?,
                        StructuralSource::Scalar(id) => self.scalar_key_parts(id)?,
                    };
                    stack.push(Visit::Finish(current, local, children.clone()));
                    for child in children.into_iter().rev() {
                        if self.cached_structural_key(child).is_none() {
                            stack.push(Visit::Enter(child));
                        }
                    }
                }
                Visit::Finish(current, local, sources) => {
                    pending.remove(&current);
                    if self.cached_structural_key(current).is_some() {
                        continue;
                    }
                    let children = sources
                        .into_iter()
                        .map(|child| {
                            self.cached_structural_key(child).ok_or_else(|| {
                                self.error(
                                    "P014",
                                    "structural-key child was not completed before its parent",
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let node = StructuralKeyNode { local, children };
                    let id = if let Some(id) = self.structural_key_cse.get(&node) {
                        *id
                    } else {
                        if self.structural_keys.len() >= self.quota.max_aggregate_elements {
                            return Err(self.error("P014", "structural-key node quota exhausted"));
                        }
                        let id = u32::try_from(self.structural_keys.len()).map_err(|_| {
                            self.error("P014", "structural-key ID capacity exhausted")
                        })?;
                        self.structural_keys.push(node.clone());
                        self.structural_key_cse.insert(node, id);
                        id
                    };
                    self.cache_structural_key(current, id);
                }
            }
        }
        self.cached_structural_key(source)
            .ok_or_else(|| self.error("P014", "structural-key construction did not finish"))
    }

    fn cached_structural_key(&self, source: StructuralSource) -> Option<u32> {
        match source {
            StructuralSource::Field(id) => self.field_structural_keys.get(&id).copied(),
            StructuralSource::Scalar(id) => self.scalar_structural_keys.get(&id).copied(),
        }
    }

    fn cache_structural_key(&mut self, source: StructuralSource, key: u32) {
        match source {
            StructuralSource::Field(id) => {
                self.field_structural_keys.insert(id, key);
            }
            StructuralSource::Scalar(id) => {
                self.scalar_structural_keys.insert(id, key);
            }
        }
    }

    fn field_key_parts(
        &self,
        id: FieldId,
    ) -> Result<(Vec<StructuralToken>, Vec<StructuralSource>), String> {
        let node = self.fields.get(id)?;
        let mut local = Vec::new();
        let mut children = Vec::new();
        match &node.kind {
            FieldKind::Primitive(primitive) => {
                local.push(StructuralToken::Tag(100));
                let (tag, scalars): (u16, Vec<ScalarId>) = match primitive {
                    Primitive::Plane { normal, offset } => {
                        (120, normal.iter().copied().chain([*offset]).collect())
                    }
                    Primitive::Sphere { center, radius } => {
                        (121, center.iter().copied().chain([*radius]).collect())
                    }
                    Primitive::Box { center, half } => (
                        122,
                        center.iter().copied().chain(half.iter().copied()).collect(),
                    ),
                    Primitive::RoundBox {
                        center,
                        half,
                        radius,
                    } => (
                        123,
                        center
                            .iter()
                            .copied()
                            .chain(half.iter().copied())
                            .chain([*radius])
                            .collect(),
                    ),
                    Primitive::Capsule { a, b, radius } => (
                        124,
                        a.iter()
                            .copied()
                            .chain(b.iter().copied())
                            .chain([*radius])
                            .collect(),
                    ),
                    Primitive::FiniteCylinder { a, b, radius } => (
                        125,
                        a.iter()
                            .copied()
                            .chain(b.iter().copied())
                            .chain([*radius])
                            .collect(),
                    ),
                    Primitive::FiniteCone {
                        a,
                        b,
                        radius_a,
                        radius_b,
                    } => (
                        126,
                        a.iter()
                            .copied()
                            .chain(b.iter().copied())
                            .chain([*radius_a, *radius_b])
                            .collect(),
                    ),
                    Primitive::Torus {
                        center,
                        axis,
                        major,
                        minor,
                    } => (
                        127,
                        center
                            .iter()
                            .copied()
                            .chain(axis.iter().copied())
                            .chain([*major, *minor])
                            .collect(),
                    ),
                };
                local.push(StructuralToken::Tag(tag));
                children.extend(scalars.into_iter().map(StructuralSource::Scalar));
            }
            FieldKind::HardUnion { a, b }
            | FieldKind::HardIntersection { a, b }
            | FieldKind::HardSubtract { a, b } => {
                local.push(StructuralToken::Tag(match &node.kind {
                    FieldKind::HardUnion { .. } => 101,
                    FieldKind::HardIntersection { .. } => 102,
                    FieldKind::HardSubtract { .. } => 103,
                    _ => unreachable!(),
                }));
                children.extend([StructuralSource::Field(*a), StructuralSource::Field(*b)]);
            }
            FieldKind::SmoothUnion { a, b, k }
            | FieldKind::SmoothIntersection { a, b, k }
            | FieldKind::SmoothSubtract { a, b, k } => {
                local.push(StructuralToken::Tag(match &node.kind {
                    FieldKind::SmoothUnion { .. } => 104,
                    FieldKind::SmoothIntersection { .. } => 105,
                    FieldKind::SmoothSubtract { .. } => 106,
                    _ => unreachable!(),
                }));
                children.extend([
                    StructuralSource::Field(*a),
                    StructuralSource::Field(*b),
                    StructuralSource::Scalar(*k),
                ]);
            }
            FieldKind::Neg { child } => {
                local.push(StructuralToken::Tag(107));
                children.push(StructuralSource::Field(*child));
            }
            FieldKind::Transform { child, transform } => {
                local.push(StructuralToken::Tag(108));
                children.push(StructuralSource::Field(*child));
                let steps = match transform {
                    TransformProgram::SourceRigidSequence { steps, .. }
                    | TransformProgram::RigidSequence { steps, .. } => {
                        local.extend([
                            StructuralToken::Tag(134),
                            StructuralToken::U32(u32::try_from(steps.len()).map_err(|_| {
                                self.error("P014", "rigid transform sequence length overflow")
                            })?),
                        ]);
                        steps.as_slice()
                    }
                    transform => std::slice::from_ref(transform),
                };
                let mut add_atomic = |transform: &TransformProgram| -> Result<(), String> {
                    let (tag, scalars): (u16, Vec<ScalarId>) = match transform {
                        TransformProgram::Translate { by } => (130, by.to_vec()),
                        TransformProgram::Rotate {
                            row_x,
                            row_y,
                            row_z,
                        } => (
                            131,
                            row_x
                                .iter()
                                .copied()
                                .chain(row_y.iter().copied())
                                .chain(row_z.iter().copied())
                                .collect(),
                        ),
                        TransformProgram::Rigid {
                            translation,
                            row_x,
                            row_y,
                            row_z,
                        } => (
                            132,
                            translation
                                .iter()
                                .copied()
                                .chain(row_x.iter().copied())
                                .chain(row_y.iter().copied())
                                .chain(row_z.iter().copied())
                                .collect(),
                        ),
                        TransformProgram::UniformScale { scale } => (133, vec![*scale]),
                        TransformProgram::SourceRigidSequence { .. }
                        | TransformProgram::RigidSequence { .. } => {
                            return Err(
                                "pixels::symbolic: nested rigid transform sequence".to_string()
                            );
                        }
                    };
                    local.push(StructuralToken::Tag(tag));
                    children.extend(scalars.into_iter().map(StructuralSource::Scalar));
                    Ok(())
                };
                for step in steps {
                    add_atomic(step)?;
                }
            }
            FieldKind::FiniteRepeat {
                child,
                axis,
                first,
                count,
                period,
            } => {
                local.extend([
                    StructuralToken::Tag(109),
                    StructuralToken::Tag(match axis {
                        Axis::X => 0,
                        Axis::Y => 1,
                        Axis::Z => 2,
                    }),
                    StructuralToken::I32(*first),
                    StructuralToken::U32(*count),
                ]);
                children.extend([
                    StructuralSource::Field(*child),
                    StructuralSource::Scalar(*period),
                ]);
            }
            FieldKind::BoundedDisplace {
                base,
                displacement,
                contract,
            } => {
                local.extend([
                    StructuralToken::Tag(110),
                    StructuralToken::Tag(match contract.derivation {
                        ClosedDeformDerivation::SinusoidalX => 0,
                    }),
                ]);
                children.extend([
                    StructuralSource::Field(*base),
                    StructuralSource::Scalar(*displacement),
                    StructuralSource::Scalar(contract.amplitude_bound),
                    StructuralSource::Scalar(contract.gradient_bound),
                    StructuralSource::Scalar(contract.hessian_bound),
                    StructuralSource::Scalar(contract.third_derivative_bound),
                ]);
            }
            FieldKind::Mark {
                child,
                object_source,
                material_source,
            } => {
                local.extend([
                    StructuralToken::Tag(111),
                    StructuralToken::Text(object_source.enum_key.clone()),
                    StructuralToken::Text(object_source.variant.clone()),
                    StructuralToken::Text(material_source.enum_key.clone()),
                    StructuralToken::Text(material_source.variant.clone()),
                ]);
                children.push(StructuralSource::Field(*child));
            }
        }
        children.push(StructuralSource::Scalar(node.scalar_value));
        Ok((local, children))
    }

    fn scalar_key_parts(
        &self,
        id: ScalarId,
    ) -> Result<(Vec<StructuralToken>, Vec<StructuralSource>), String> {
        let op = &self.scalar.get(id)?.op;
        let mut local = Vec::new();
        let mut children = Vec::new();
        match op {
            ScalarOp::ConstF32(bits) => {
                local.extend([StructuralToken::Tag(0), StructuralToken::Bits32(*bits)]);
            }
            ScalarOp::ConstF64(bits) => {
                local.extend([StructuralToken::Tag(1), StructuralToken::Bits64(*bits)]);
            }
            ScalarOp::CoordX => local.push(StructuralToken::Tag(2)),
            ScalarOp::CoordY => local.push(StructuralToken::Tag(3)),
            ScalarOp::CoordZ => local.push(StructuralToken::Tag(4)),
            ScalarOp::SurfacePosition(component) | ScalarOp::SurfaceNormal(component) => {
                local.extend([
                    StructuralToken::Tag(if matches!(op, ScalarOp::SurfacePosition(_)) {
                        5
                    } else {
                        6
                    }),
                    StructuralToken::U32(u32::from(*component)),
                ]);
            }
            ScalarOp::Param(param) => {
                local.push(StructuralToken::Tag(7));
                let record = self
                    .params
                    .get(param.index())
                    .ok_or_else(|| "pixels::symbolic: missing parameter key".to_string())?;
                local.push(StructuralToken::U32(
                    u32::try_from(record.path.len())
                        .map_err(|_| "pixels::symbolic: parameter path is too long".to_string())?,
                ));
                for index in &record.path {
                    local.push(StructuralToken::U32(u32::try_from(*index).map_err(
                        |_| "pixels::symbolic: parameter path index overflow".to_string(),
                    )?));
                }
                local.push(StructuralToken::U32(
                    record.component.map(u32::from).unwrap_or(u32::MAX),
                ));
            }
            ScalarOp::Add(a, b)
            | ScalarOp::Sub(a, b)
            | ScalarOp::Mul(a, b)
            | ScalarOp::Div(a, b)
            | ScalarOp::Min(a, b)
            | ScalarOp::Max(a, b) => {
                local.push(StructuralToken::Tag(match op {
                    ScalarOp::Add(..) => 8,
                    ScalarOp::Sub(..) => 9,
                    ScalarOp::Mul(..) => 10,
                    ScalarOp::Div(..) => 11,
                    ScalarOp::Min(..) => 14,
                    ScalarOp::Max(..) => 15,
                    _ => unreachable!(),
                }));
                children.extend([*a, *b]);
            }
            ScalarOp::Neg(value) | ScalarOp::Abs(value) => {
                local.push(StructuralToken::Tag(if matches!(op, ScalarOp::Neg(..)) {
                    12
                } else {
                    13
                }));
                children.push(*value);
            }
            ScalarOp::Clamp { value, lo, hi } => {
                local.push(StructuralToken::Tag(16));
                children.extend([*value, *lo, *hi]);
            }
            ScalarOp::Sqrt(value, semantic)
            | ScalarOp::Rsqrt(value, semantic)
            | ScalarOp::SinRestricted(value, semantic)
            | ScalarOp::CosRestricted(value, semantic) => {
                local.extend([
                    StructuralToken::Tag(match op {
                        ScalarOp::Sqrt(..) => 17,
                        ScalarOp::Rsqrt(..) => 18,
                        ScalarOp::SinRestricted(..) => 19,
                        ScalarOp::CosRestricted(..) => 20,
                        _ => unreachable!(),
                    }),
                    StructuralToken::Tag(semantic_tag(*semantic)),
                ]);
                children.push(*value);
            }
            ScalarOp::Dot3(a, b) => {
                local.push(StructuralToken::Tag(21));
                children.extend(*a);
                children.extend(*b);
            }
            ScalarOp::Cross3Component { component, a, b } => {
                local.extend([
                    StructuralToken::Tag(22),
                    StructuralToken::U32(u32::from(*component)),
                ]);
                children.extend(*a);
                children.extend(*b);
            }
            ScalarOp::Length2(values) => {
                local.push(StructuralToken::Tag(23));
                children.extend(*values);
            }
            ScalarOp::Length3(values) => {
                local.push(StructuralToken::Tag(24));
                children.extend(*values);
            }
            ScalarOp::Normalize3Component {
                component,
                value,
                semantic,
            } => {
                local.extend([
                    StructuralToken::Tag(25),
                    StructuralToken::U32(u32::from(*component)),
                    StructuralToken::Tag(semantic_tag(*semantic)),
                ]);
                children.extend(*value);
            }
            ScalarOp::Compare { op, a, b } => {
                local.extend([
                    StructuralToken::Tag(26),
                    StructuralToken::Tag(match op {
                        CompareOp::Lt => 0,
                        CompareOp::Le => 1,
                        CompareOp::Gt => 2,
                        CompareOp::Ge => 3,
                        CompareOp::Eq => 4,
                        CompareOp::Ne => 5,
                    }),
                ]);
                children.extend([*a, *b]);
            }
            ScalarOp::Select { predicate, a, b } => {
                local.push(StructuralToken::Tag(27));
                children.extend([*predicate, *a, *b]);
            }
            ScalarOp::SelectIndex { index, options } => {
                local.extend([
                    StructuralToken::Tag(28),
                    StructuralToken::U32(u32::try_from(options.len()).map_err(|_| {
                        "pixels::symbolic: select option count overflow".to_string()
                    })?),
                ]);
                children.push(*index);
                children.extend(options);
            }
            ScalarOp::SmoothMin { a, b, k, semantic } => {
                local.extend([
                    StructuralToken::Tag(29),
                    StructuralToken::Tag(semantic_tag(*semantic)),
                ]);
                children.extend([*a, *b, *k]);
            }
            ScalarOp::FiniteOr {
                value,
                fallback,
                semantic,
            } => {
                local.extend([
                    StructuralToken::Tag(30),
                    StructuralToken::Tag(semantic_tag(*semantic)),
                ]);
                children.extend([*value, *fallback]);
            }
            ScalarOp::MaterialRoughness { value, semantic } => {
                local.extend([
                    StructuralToken::Tag(31),
                    StructuralToken::Tag(semantic_tag(*semantic)),
                ]);
                children.push(*value);
            }
        }
        Ok((
            local,
            children.into_iter().map(StructuralSource::Scalar).collect(),
        ))
    }

    fn field_binary(
        &mut self,
        intrinsic: FieldIntrinsic,
        left: FieldId,
        right: FieldId,
        module: &str,
        span: Span,
    ) -> Result<FieldId, String> {
        let a = self.fields.get(left)?.scalar_value;
        let b = self.fields.get(right)?.scalar_value;
        // Hard union/intersection are structurally commutative, but their
        // fallback scalar expression retains source operand order below.
        let (canonical_left, canonical_right) =
            if self.compare_field_structural(right, left)?.is_lt() {
                (right, left)
            } else {
                (left, right)
            };
        let (kind, scalar) = match intrinsic {
            FieldIntrinsic::Union => (
                FieldKind::HardUnion {
                    a: canonical_left,
                    b: canonical_right,
                },
                self.min(a, b, module, span)?,
            ),
            FieldIntrinsic::Intersection => (
                FieldKind::HardIntersection {
                    a: canonical_left,
                    b: canonical_right,
                },
                self.max(a, b, module, span)?,
            ),
            FieldIntrinsic::Subtract => {
                let neg_b = self.neg(b, module, span)?;
                (
                    FieldKind::HardSubtract { a: left, b: right },
                    self.max(a, neg_b, module, span)?,
                )
            }
            _ => unreachable!("closed hard CSG names"),
        };
        self.field_node(kind, scalar, module, span)
    }

    fn smooth_min_scalar(
        &mut self,
        a: ScalarId,
        b: ScalarId,
        k: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<ScalarId, String> {
        let dependency = self
            .scalar_dependency(a)?
            .combine(self.scalar_dependency(b)?)
            .combine(self.scalar_dependency(k)?);
        self.scalar_node(
            ScalarOp::SmoothMin {
                a,
                b,
                k,
                semantic: SemanticOpId::SmoothMinF32V1,
            },
            dependency,
            module,
            span,
        )
    }

    fn field_smooth(
        &mut self,
        intrinsic: FieldIntrinsic,
        left: FieldId,
        right: FieldId,
        k: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<FieldId, String> {
        if self.field_contains_hard_boundary(left)? || self.field_contains_hard_boundary(right)? {
            return Err(self.error(
                "P004",
                "smooth CSG cannot enclose hard union, intersection, subtraction, or negation; \
                 keep hard operations outside maximal smooth subtrees",
            ));
        }
        let a = self.fields.get(left)?.scalar_value;
        let b = self.fields.get(right)?.scalar_value;
        let (kind, scalar) = match intrinsic {
            FieldIntrinsic::SmoothUnion => (
                FieldKind::SmoothUnion {
                    a: left,
                    b: right,
                    k,
                },
                self.smooth_min_scalar(a, b, k, module, span)?,
            ),
            FieldIntrinsic::SmoothIntersection => {
                let na = self.neg(a, module, span)?;
                let nb = self.neg(b, module, span)?;
                let smooth = self.smooth_min_scalar(na, nb, k, module, span)?;
                (
                    FieldKind::SmoothIntersection {
                        a: left,
                        b: right,
                        k,
                    },
                    self.neg(smooth, module, span)?,
                )
            }
            FieldIntrinsic::SmoothSubtract => {
                let na = self.neg(a, module, span)?;
                let inner = self.smooth_min_scalar(na, b, k, module, span)?;
                let scalar = self.neg(inner, module, span)?;
                (
                    FieldKind::SmoothSubtract {
                        a: left,
                        b: right,
                        k,
                    },
                    scalar,
                )
            }
            _ => unreachable!("closed smooth CSG names"),
        };
        self.field_node(kind, scalar, module, span)
    }

    fn field_contains_hard_boundary(&self, root: FieldId) -> Result<bool, String> {
        let mut stack = vec![root];
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match &self.fields.get(id)?.kind {
                FieldKind::HardUnion { .. }
                | FieldKind::HardIntersection { .. }
                | FieldKind::HardSubtract { .. }
                | FieldKind::Neg { .. } => return Ok(true),
                FieldKind::Primitive(_) => {}
                FieldKind::SmoothUnion { a, b, .. }
                | FieldKind::SmoothIntersection { a, b, .. }
                | FieldKind::SmoothSubtract { a, b, .. } => stack.extend([*a, *b]),
                FieldKind::Transform { child, .. }
                | FieldKind::FiniteRepeat { child, .. }
                | FieldKind::Mark { child, .. } => stack.push(*child),
                FieldKind::BoundedDisplace { base, .. } => stack.push(*base),
            }
        }
        Ok(false)
    }

    fn uniform_scale_field(
        &mut self,
        field: FieldId,
        scale: ScalarId,
        module: &str,
        span: Span,
    ) -> Result<FieldId, String> {
        let kind = self.fields.get(field)?.kind.clone();
        match kind {
            FieldKind::HardUnion { a, b } => {
                let a = self.uniform_scale_field(a, scale, module, span)?;
                let b = self.uniform_scale_field(b, scale, module, span)?;
                self.field_binary(FieldIntrinsic::Union, a, b, module, span)
            }
            FieldKind::HardIntersection { a, b } => {
                let a = self.uniform_scale_field(a, scale, module, span)?;
                let b = self.uniform_scale_field(b, scale, module, span)?;
                self.field_binary(FieldIntrinsic::Intersection, a, b, module, span)
            }
            FieldKind::HardSubtract { a, b } => {
                let a = self.uniform_scale_field(a, scale, module, span)?;
                let b = self.uniform_scale_field(b, scale, module, span)?;
                self.field_binary(FieldIntrinsic::Subtract, a, b, module, span)
            }
            FieldKind::Neg { child } => {
                let child = self.uniform_scale_field(child, scale, module, span)?;
                let scalar = self.neg(self.fields.get(child)?.scalar_value, module, span)?;
                self.field_node(FieldKind::Neg { child }, scalar, module, span)
            }
            FieldKind::Mark {
                child,
                object_source,
                material_source,
            } => {
                let child = self.uniform_scale_field(child, scale, module, span)?;
                let scalar = self.fields.get(child)?.scalar_value;
                self.field_node(
                    FieldKind::Mark {
                        child,
                        object_source,
                        material_source,
                    },
                    scalar,
                    module,
                    span,
                )
            }
            _ => {
                let scalar = self.mul(self.fields.get(field)?.scalar_value, scale, module, span)?;
                self.field_node(
                    FieldKind::Transform {
                        child: field,
                        transform: TransformProgram::UniformScale { scale },
                    },
                    scalar,
                    module,
                    span,
                )
            }
        }
    }

    fn displace_hard_partitioned(
        &mut self,
        field: FieldId,
        displacement: ScalarId,
        contract: &DerivedDeformContract,
        module: &str,
        span: Span,
    ) -> Result<FieldId, String> {
        let kind = self.fields.get(field)?.kind.clone();
        match kind {
            FieldKind::HardUnion { a, b } => {
                let a = self.displace_hard_partitioned(a, displacement, contract, module, span)?;
                let b = self.displace_hard_partitioned(b, displacement, contract, module, span)?;
                self.field_binary(FieldIntrinsic::Union, a, b, module, span)
            }
            FieldKind::HardIntersection { a, b } => {
                let a = self.displace_hard_partitioned(a, displacement, contract, module, span)?;
                let b = self.displace_hard_partitioned(b, displacement, contract, module, span)?;
                self.field_binary(FieldIntrinsic::Intersection, a, b, module, span)
            }
            FieldKind::HardSubtract { a, b } => {
                let a = self.displace_hard_partitioned(a, displacement, contract, module, span)?;
                let neg_displacement = self.neg(displacement, module, span)?;
                let b =
                    self.displace_hard_partitioned(b, neg_displacement, contract, module, span)?;
                self.field_binary(FieldIntrinsic::Subtract, a, b, module, span)
            }
            FieldKind::Neg { child } => {
                let neg_displacement = self.neg(displacement, module, span)?;
                let child = self.displace_hard_partitioned(
                    child,
                    neg_displacement,
                    contract,
                    module,
                    span,
                )?;
                let scalar = self.neg(self.fields.get(child)?.scalar_value, module, span)?;
                self.field_node(FieldKind::Neg { child }, scalar, module, span)
            }
            FieldKind::Mark {
                child,
                object_source,
                material_source,
            } => {
                let child =
                    self.displace_hard_partitioned(child, displacement, contract, module, span)?;
                let scalar = self.fields.get(child)?.scalar_value;
                self.field_node(
                    FieldKind::Mark {
                        child,
                        object_source,
                        material_source,
                    },
                    scalar,
                    module,
                    span,
                )
            }
            _ => {
                let scalar = self.add(
                    self.fields.get(field)?.scalar_value,
                    displacement,
                    module,
                    span,
                )?;
                self.field_node(
                    FieldKind::BoundedDisplace {
                        base: field,
                        displacement,
                        contract: contract.clone(),
                    },
                    scalar,
                    module,
                    span,
                )
            }
        }
    }

    fn eval_displace(
        &mut self,
        args: Vec<SymValue>,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        let base = expect_field(args[0].clone())?;
        let point = self.as_vec3(args[1].clone(), module, span)?;
        if !point.coordinate_provenance {
            return Err(self.error(
                "P004",
                "displacement point argument must be derived from the renderer coordinate",
            ));
        }
        if has_discontinuous_transform(&point.transforms) {
            return Err(self.error(
                "P004",
                "`sinusoidal_displace` requires continuous coordinates; finite-repeat coordinates have discontinuous cell boundaries",
            ));
        }
        let amplitude = self.as_scalar(args[2].clone(), module, span)?;
        let frequency = self.as_scalar(args[3].clone(), module, span)?;
        let phase = self.as_scalar(args[4].clone(), module, span)?;
        self.require_coordinate_free_scalar(amplitude, "sinusoidal_displace", "amplitude")?;
        self.require_coordinate_free_scalar(frequency, "sinusoidal_displace", "frequency")?;
        self.require_coordinate_free_scalar(phase, "sinusoidal_displace", "phase")?;
        let frequency_x = self.mul(frequency, point.values[0], module, span)?;
        let angle = self.add(frequency_x, phase, module, span)?;
        self.obligations.push(PendingObligation::Scalar(
            ProofObligation::RestrictedTrigDomain { argument: angle },
        ));
        let wave = self.scalar_node(
            ScalarOp::SinRestricted(angle, SemanticOpId::SinRestrictedF32V1),
            self.scalar_dependency(angle)?,
            module,
            span,
        )?;
        let displacement = self.mul(amplitude, wave, module, span)?;
        let value_factor =
            self.const_f32(super::scalar::SOURCE_TRIG_VALUE_FACTOR_V2, module, span)?;
        let gradient_factor =
            self.const_f32(super::scalar::SOURCE_TRIG_GRADIENT_FACTOR_V2, module, span)?;
        let hessian_factor =
            self.const_f32(super::scalar::SOURCE_TRIG_HESSIAN_FACTOR_V2, module, span)?;
        let third_factor =
            self.const_f32(super::scalar::SOURCE_TRIG_THIRD_FACTOR_V2, module, span)?;
        let amplitude_abs = self.abs(amplitude, module, span)?;
        let amplitude_bound = self.mul(amplitude_abs, value_factor, module, span)?;
        let amplitude_frequency = self.mul(amplitude, frequency, module, span)?;
        let gradient_abs = self.abs(amplitude_frequency, module, span)?;
        let gradient_bound = self.mul(gradient_abs, gradient_factor, module, span)?;
        let frequency2 = self.mul(frequency, frequency, module, span)?;
        let amplitude_frequency2 = self.mul(amplitude, frequency2, module, span)?;
        let hessian_abs = self.abs(amplitude_frequency2, module, span)?;
        let hessian_bound = self.mul(hessian_abs, hessian_factor, module, span)?;
        let frequency3 = self.mul(frequency2, frequency, module, span)?;
        let amplitude_frequency3 = self.mul(amplitude, frequency3, module, span)?;
        let third_abs = self.abs(amplitude_frequency3, module, span)?;
        let third_derivative_bound = self.mul(third_abs, third_factor, module, span)?;
        let contract = DerivedDeformContract {
            amplitude_bound,
            gradient_bound,
            hessian_bound,
            third_derivative_bound,
            coordinate_x: point.values[0],
            frequency,
            phase,
            derivation: ClosedDeformDerivation::SinusoidalX,
        };
        // The common additive deformation commutes with min/max under the
        // pinned f32 ordering. Push it through hard CSG (flipping the offset
        // under negation) so every emitted local object remains a maximal
        // smooth subtree while the authored scalar bits are preserved.
        let partitioned =
            self.displace_hard_partitioned(base, displacement, &contract, module, span)?;
        Ok(SymValue::Field(partitioned))
    }

    fn eval_material_constructor(
        &mut self,
        intrinsic: MaterialIntrinsic,
        args: Vec<Option<SymValue>>,
        module: &str,
        span: Span,
    ) -> Result<SymValue, String> {
        // `clay` and `porcelain` are source-level aliases of `standard` in
        // render.wr. Keep this exhaustive match so adding distinct preset
        // semantics cannot silently fall through the symbolic classifier.
        match intrinsic {
            MaterialIntrinsic::Standard
            | MaterialIntrinsic::Clay
            | MaterialIntrinsic::Porcelain
            | MaterialIntrinsic::Textured => {}
        }
        let color = self.as_rgb(
            args.first()
                .cloned()
                .flatten()
                .ok_or_else(|| self.error("P004", "MaterialSample lacks color"))?,
            module,
            span,
        )?;
        let roughness = self.as_scalar(
            args.get(1)
                .cloned()
                .flatten()
                .ok_or_else(|| self.error("P004", "MaterialSample lacks roughness"))?,
            module,
            span,
        )?;
        let zero = self.const_f32(0.0, module, span)?;
        let one = self.const_f32(1.0, module, span)?;
        let half = self.const_f32(0.5, module, span)?;
        let ior = self.const_f32(1.5, module, span)?;
        let pattern = if intrinsic == MaterialIntrinsic::Textured {
            let stable_id = u32::try_from(expect_const_int(
                args.get(2)
                    .cloned()
                    .flatten()
                    .ok_or_else(|| self.error("P004", "textured material lacks texture id"))?,
            )?)
            .map_err(|_| self.error("P004", "texture id must fit u32"))?;
            let authored_width = u32::try_from(expect_const_int(
                args.get(3)
                    .cloned()
                    .flatten()
                    .ok_or_else(|| self.error("P004", "textured material lacks width"))?,
            )?)
            .map_err(|_| self.error("P004", "texture width must fit u32"))?;
            let authored_height = u32::try_from(expect_const_int(
                args.get(4)
                    .cloned()
                    .flatten()
                    .ok_or_else(|| self.error("P004", "textured material lacks height"))?,
            )?)
            .map_err(|_| self.error("P004", "texture height must fit u32"))?;
            let filter = expect_identity(
                args.get(5)
                    .cloned()
                    .flatten()
                    .ok_or_else(|| self.error("P004", "textured material lacks filter"))?,
            )?;
            let filter = match filter.variant.as_str() {
                "Nearest" => TextureFilterV1::Nearest,
                "Bilinear" => TextureFilterV1::Bilinear,
                "Trilinear" => TextureFilterV1::Trilinear,
                "Anisotropic4" => TextureFilterV1::Anisotropic4,
                other => {
                    return Err(self.error(
                        "P004",
                        format!("unsupported immutable texture filter `{other}`"),
                    ));
                }
            };
            let uv_source = match args.get(6).cloned().flatten() {
                Some(value) => match expect_identity(value)?.variant.as_str() {
                    "Plane" => UvSourceV1::Plane,
                    "Sphere" => UvSourceV1::Sphere,
                    "Cylinder" => UvSourceV1::Cylinder,
                    "Torus" => UvSourceV1::Torus,
                    "BoxFeature" => UvSourceV1::BoxFeature,
                    "RoundBoxFeature" => UvSourceV1::RoundBoxFeature,
                    "ObjectTriplanar" => UvSourceV1::ObjectTriplanar,
                    "WorldTriplanar" => UvSourceV1::WorldTriplanar,
                    other => {
                        return Err(self.error(
                            "P004",
                            format!("unsupported immutable texture UV source `{other}`"),
                        ));
                    }
                },
                None => UvSourceV1::WorldTriplanar,
            };
            let texture = super::material_graph::compiler_texture(stable_id, filter, uv_source)
                .map_err(|message| self.error("P004", message))?;
            if (authored_width, authored_height) != (texture.width, texture.height) {
                return Err(self.error(
                    "P004",
                    format!(
                        "field operation `texture_lookup` is not available in `AaaByteExact`: \
                         compiler-owned asset {stable_id} has sealed dimensions {}x{}, not {}x{}",
                        texture.width, texture.height, authored_width, authored_height
                    ),
                ));
            }
            Some(texture)
        } else {
            None
        };
        let (metallic, specular_level, emissive, opacity, normal) = if intrinsic
            == MaterialIntrinsic::Standard
        {
            let metallic = match args.get(2).cloned().flatten() {
                Some(value) => self.as_scalar(value, module, span)?,
                None => zero,
            };
            let specular = match args.get(3).cloned().flatten() {
                Some(value) => self.as_scalar(value, module, span)?,
                None => half,
            };
            let emissive = match args.get(4).cloned().flatten() {
                Some(value) => self.as_rgb(value, module, span)?,
                None => [zero; 3],
            };
            let opacity = match args.get(5).cloned().flatten() {
                Some(value) => self.as_scalar(value, module, span)?,
                None => one,
            };
            let normal = match args.get(6).cloned().flatten() {
                None => NormalModel::Geometric,
                Some(SymValue::Enum(identity, payload))
                    if identity.variant == "Geometric" && payload.is_empty() =>
                {
                    NormalModel::Geometric
                }
                Some(SymValue::Enum(identity, payload))
                    if matches!(identity.variant.as_str(), "AnalyticSlope" | "ObjectSlope")
                        && payload.len() == 2 =>
                {
                    NormalModel::AnalyticSlope {
                        x: self.as_scalar(payload[0].clone(), module, span)?,
                        y: self.as_scalar(payload[1].clone(), module, span)?,
                    }
                }
                Some(SymValue::Enum(identity, payload))
                    if matches!(identity.variant.as_str(), "TextureSlope" | "TextureSlopeUv")
                        && (payload.len() == 4 || payload.len() == 5) =>
                {
                    let stable_id = u32::try_from(expect_const_int(payload[0].clone())?)
                        .map_err(|_| self.error("P004", "normal texture id must fit u32"))?;
                    let width = u32::try_from(expect_const_int(payload[1].clone())?)
                        .map_err(|_| self.error("P004", "normal texture width must fit u32"))?;
                    let height = u32::try_from(expect_const_int(payload[2].clone())?)
                        .map_err(|_| self.error("P004", "normal texture height must fit u32"))?;
                    let filter = expect_identity(payload[3].clone())?;
                    let filter = match filter.variant.as_str() {
                        "Nearest" => TextureFilterV1::Nearest,
                        "Bilinear" => TextureFilterV1::Bilinear,
                        "Trilinear" => TextureFilterV1::Trilinear,
                        "Anisotropic4" => TextureFilterV1::Anisotropic4,
                        other => {
                            return Err(self.error(
                                "P004",
                                format!("unsupported normal texture filter `{other}`"),
                            ));
                        }
                    };
                    let uv_source = if payload.len() == 5 {
                        match expect_identity(payload[4].clone())?.variant.as_str() {
                            "Plane" => UvSourceV1::Plane,
                            "Sphere" => UvSourceV1::Sphere,
                            "Cylinder" => UvSourceV1::Cylinder,
                            "Torus" => UvSourceV1::Torus,
                            "BoxFeature" => UvSourceV1::BoxFeature,
                            "RoundBoxFeature" => UvSourceV1::RoundBoxFeature,
                            "ObjectTriplanar" => UvSourceV1::ObjectTriplanar,
                            "WorldTriplanar" => UvSourceV1::WorldTriplanar,
                            other => {
                                return Err(self.error(
                                    "P004",
                                    format!("unsupported normal texture UV source `{other}`"),
                                ));
                            }
                        }
                    } else {
                        UvSourceV1::WorldTriplanar
                    };
                    let texture =
                        super::material_graph::compiler_texture(stable_id, filter, uv_source)
                            .map_err(|message| self.error("P004", message))?;
                    if texture.format_tag != 3 || (width, height) != (texture.width, texture.height)
                    {
                        return Err(self.error(
                                "P004",
                                "normal detail texture must be a sealed Rg8Snorm asset with exact dimensions",
                            ));
                    }
                    NormalModel::TextureSlope { texture }
                }
                Some(_) => {
                    return Err(self.error(
                        "P004",
                        "normal detail is not a closed v1 geometric/analytic/object slope",
                    ));
                }
            };
            (metallic, specular, emissive, opacity, normal)
        } else {
            (zero, half, [zero; 3], one, NormalModel::Geometric)
        };
        let sample = MaterialSampleNode {
            base_color: color,
            opacity,
            emissive,
            roughness,
            metallic,
            specular_level,
            ior,
            normal,
            pattern,
        };
        Ok(SymValue::Material(self.material_node(
            MaterialKind::Sample(sample),
            module,
            span,
        )?))
    }

    fn eval_function(
        &mut self,
        function: LocatedFn,
        values: Vec<Option<SymValue>>,
        call_module: &str,
        call_span: Span,
    ) -> Result<SymValue, String> {
        let identity = format!("{}::{}", function.module, function.key);
        self.call_stack.push(identity);
        self.call_sites.push(OriginSite {
            module: call_module.to_string(),
            span: call_span,
        });
        self.quota.call(&self.call_stack)?;
        let result = (|| {
            let mut scope = BTreeMap::new();
            if values.len()
                != function.function.params.len()
                    + usize::from(function.function.receiver.is_some())
            {
                return Err(self.error(
                    "P004",
                    format!(
                        "renderer helper argument count mismatch: expected {}, found {}",
                        function.function.params.len()
                            + usize::from(function.function.receiver.is_some()),
                        values.len()
                    ),
                ));
            }
            let mut values = values.into_iter();
            if function.function.receiver.is_some() {
                let receiver = values
                    .next()
                    .flatten()
                    .ok_or_else(|| self.error("P004", "renderer receiver is missing"))?;
                scope.insert("self".to_string(), receiver);
            }
            self.scopes.push(scope);
            for (parameter, value) in function.function.params.iter().zip(values) {
                let value = match value {
                    Some(value) => value,
                    None => {
                        let default = parameter.default.as_ref().ok_or_else(|| {
                            self.error(
                                "P004",
                                format!(
                                    "renderer helper argument `{}` is absent without a default",
                                    parameter.name
                                ),
                            )
                        })?;
                        self.eval_expr(default, &function.module)?
                    }
                };
                self.bind(parameter.name.clone(), value)?;
            }
            let flow = self.exec_stmts(&function.function.body, &function.module)?;
            self.scopes.pop();
            match flow {
                Flow::Return(value) => Ok(value),
                Flow::Continue if matches!(function.function.ret, Type::Unit) => Ok(SymValue::Unit),
                Flow::Continue => Err(self.error(
                    "P004",
                    "renderer helper completed without returning its declared value",
                )),
            }
        })();
        self.call_sites.pop();
        self.call_stack.pop();
        result
    }

    fn exec_stmts(&mut self, stmts: &[TypedStmt], module: &str) -> Result<Flow, String> {
        for (stmt_index, stmt) in stmts.iter().enumerate() {
            self.quota.step(&self.call_stack)?;
            let flow = match &stmt.kind {
                TypedStmtKind::Let { name, value, .. } => {
                    let value = self.eval_expr(value, module)?;
                    self.bind(name.clone(), value)?;
                    Flow::Continue
                }
                TypedStmtKind::Assign { target, value } => {
                    let TypedExprKind::Local(name) = &target.kind else {
                        return Err(
                            self.error("P004", "symbolic assignment target must be a typed local")
                        );
                    };
                    let value = self.eval_expr(value, module)?;
                    self.assign(name, value)?;
                    Flow::Continue
                }
                TypedStmtKind::Return(value) => Flow::Return(match value {
                    Some(value) => self.eval_expr(value, module)?,
                    None => SymValue::Unit,
                }),
                TypedStmtKind::ExprStmt(value) => {
                    self.eval_expr(value, module)?;
                    Flow::Continue
                }
                TypedStmtKind::Pass => Flow::Continue,
                TypedStmtKind::If {
                    cond,
                    then_branch,
                    elifs,
                    else_branch,
                } => {
                    let condition = self.eval_expr(cond, module)?;
                    match condition {
                        SymValue::Bool(SymBool::Const(true)) => {
                            self.exec_scoped_stmts(then_branch, module)?
                        }
                        SymValue::Bool(SymBool::Const(false)) => {
                            let mut selected = None;
                            for elif in elifs {
                                match self.eval_expr(&elif.cond, module)? {
                                    SymValue::Bool(SymBool::Const(true)) => {
                                        selected =
                                            Some(self.exec_scoped_stmts(&elif.body, module)?);
                                        break;
                                    }
                                    SymValue::Bool(SymBool::Const(false)) => {}
                                    _ => {
                                        return Err(self.error(
                                            "P004",
                                            "runtime elif requires explicit material selection",
                                        ));
                                    }
                                }
                            }
                            match selected {
                                Some(flow) => flow,
                                None => {
                                    if let Some(branch) = else_branch {
                                        self.exec_scoped_stmts(branch, module)?
                                    } else {
                                        Flow::Continue
                                    }
                                }
                            }
                        }
                        SymValue::Bool(SymBool::Runtime(predicate))
                            if self.kind == PixelsFnKind::Material =>
                        {
                            let tail = &stmts[stmt_index + 1..];
                            let then_value =
                                self.branch_return_with_tail(then_branch, tail, module)?;
                            let a = expect_material(then_value)?;
                            let b = self.material_if_fallback(
                                elifs,
                                else_branch.as_deref().unwrap_or(&[]),
                                tail,
                                module,
                                stmt.span,
                            )?;
                            if a == b {
                                Flow::Return(SymValue::Material(a))
                            } else {
                                self.obligations
                                    .push(PendingObligation::MaterialEvent { predicate });
                                Flow::Return(SymValue::Material(self.material_node(
                                    MaterialKind::Select { predicate, a, b },
                                    module,
                                    stmt.span,
                                )?))
                            }
                        }
                        _ => {
                            return Err(self.error(
                                "P003",
                                "field runtime branch reached symbolic evaluation",
                            ));
                        }
                    }
                }
                TypedStmtKind::For {
                    name, iter, body, ..
                } => {
                    let values = match iter {
                        TypedForIter::Range(start, end, inclusive) => {
                            let start = expect_const_int(self.eval_expr(start, module)?)?;
                            let end = expect_const_int(self.eval_expr(end, module)?)?;
                            let exclusive_end = if *inclusive {
                                end.checked_add(1)
                                    .ok_or_else(|| self.error("P014", "loop endpoint overflow"))?
                            } else {
                                end
                            };
                            (start..exclusive_end)
                                .map(|value| SymValue::Int(SymInt::Const(value)))
                                .collect::<Vec<_>>()
                        }
                        TypedForIter::Expr(value) => {
                            let Type::Array(_, length) = &value.ty else {
                                return Err(self.error(
                                    "P004",
                                    "symbolic for expression is not an exact array",
                                ));
                            };
                            let extent = crate::sema::bodies::literal_array_len(length)
                                .and_then(|value| usize::try_from(value).ok())
                                .ok_or_else(|| {
                                    self.error(
                                        "P014",
                                        "symbolic for array extent is not representable",
                                    )
                                })?;
                            let collection = self.eval_expr(value, module)?;
                            (0..extent)
                                .map(|index| {
                                    self.index_value(
                                        collection.clone(),
                                        SymValue::Int(SymInt::Const(index as i128)),
                                        module,
                                        stmt.span,
                                    )
                                })
                                .collect::<Result<Vec<_>, _>>()?
                        }
                    };
                    self.quota.unroll(
                        u64::try_from(values.len())
                            .map_err(|_| self.error("P014", "loop expansion is too large"))?,
                        &self.call_stack,
                    )?;
                    for value in values {
                        self.scopes.push(BTreeMap::new());
                        self.bind(name.clone(), value)?;
                        let flow = self.exec_stmts(body, module);
                        self.scopes.pop();
                        match flow? {
                            Flow::Continue => {}
                            result @ Flow::Return(_) => return Ok(result),
                        }
                    }
                    Flow::Continue
                }
                TypedStmtKind::Match { scrutinee, arms } => {
                    self.exec_match(scrutinee, arms, &stmts[stmt_index + 1..], module, stmt.span)?
                }
                TypedStmtKind::ComptimeAssert { cond, .. } | TypedStmtKind::Assert { cond, .. } => {
                    match self.eval_expr(cond, module)? {
                        SymValue::Bool(SymBool::Const(true)) => Flow::Continue,
                        SymValue::Bool(SymBool::Const(false)) => {
                            return Err(
                                self.error("P004", "renderer compile-time assertion failed")
                            );
                        }
                        _ => {
                            return Err(self.error(
                                "P004",
                                "runtime assertion is not part of symbolic renderer semantics",
                            ));
                        }
                    }
                }
                TypedStmtKind::While { .. }
                | TypedStmtKind::Break
                | TypedStmtKind::Continue
                | TypedStmtKind::Defer(_)
                | TypedStmtKind::BareSend { .. }
                | TypedStmtKind::WithGroup { .. } => {
                    return Err(self.error(
                        "P003",
                        "illegal control/effect form reached symbolic evaluation",
                    ));
                }
            };
            if matches!(flow, Flow::Return(_)) {
                return Ok(flow);
            }
        }
        Ok(Flow::Continue)
    }

    fn exec_scoped_stmts(&mut self, stmts: &[TypedStmt], module: &str) -> Result<Flow, String> {
        self.scopes.push(BTreeMap::new());
        let result = self.exec_stmts(stmts, module);
        self.scopes.pop();
        result
    }

    fn material_if_fallback(
        &mut self,
        elifs: &[crate::sema::typed::TypedElif],
        else_branch: &[TypedStmt],
        tail: &[TypedStmt],
        module: &str,
        span: Span,
    ) -> Result<MaterialId, String> {
        let mut runtime_branches = Vec::new();
        let mut terminal = None;
        for elif in elifs {
            let condition = self.eval_expr(&elif.cond, module)?;
            match condition {
                SymValue::Bool(SymBool::Const(false)) => {}
                SymValue::Bool(SymBool::Const(true)) => {
                    terminal = Some(expect_material(
                        self.branch_return_with_tail(&elif.body, tail, module)?,
                    )?);
                    break;
                }
                SymValue::Bool(SymBool::Runtime(predicate)) => {
                    let branch =
                        expect_material(self.branch_return_with_tail(&elif.body, tail, module)?)?;
                    runtime_branches.push((predicate, branch));
                }
                other => {
                    return Err(self.error(
                        "P004",
                        format!("material elif condition is not boolean: {other:?}"),
                    ));
                }
            }
        }
        let mut fallback = match terminal {
            Some(terminal) => terminal,
            None => expect_material(self.branch_return_with_tail(else_branch, tail, module)?)?,
        };
        for (predicate, branch) in runtime_branches.into_iter().rev() {
            if branch == fallback {
                continue;
            }
            self.obligations
                .push(PendingObligation::MaterialEvent { predicate });
            fallback = self.material_node(
                MaterialKind::Select {
                    predicate,
                    a: branch,
                    b: fallback,
                },
                module,
                span,
            )?;
        }
        Ok(fallback)
    }

    fn branch_return_with_tail(
        &mut self,
        body: &[TypedStmt],
        tail: &[TypedStmt],
        module: &str,
    ) -> Result<SymValue, String> {
        let saved = self.scopes.clone();
        let result = self
            .exec_scoped_stmts(body, module)
            .and_then(|flow| match flow {
                Flow::Return(value) => Ok(Flow::Return(value)),
                Flow::Continue => self.exec_stmts(tail, module),
            });
        self.scopes = saved;
        match result? {
            Flow::Return(value) => Ok(value),
            Flow::Continue => Err(self.error(
                "P004",
                "runtime material branch must return a MaterialSample",
            )),
        }
    }

    fn exec_match(
        &mut self,
        scrutinee: &TypedExpr,
        arms: &[TypedMatchArm],
        tail: &[TypedStmt],
        module: &str,
        span: Span,
    ) -> Result<Flow, String> {
        let scrutinee = self.eval_expr(scrutinee, module)?;
        match &scrutinee {
            SymValue::Enum(_, _) => {
                for arm in arms {
                    if self.pattern_matches_comptime(&scrutinee, &arm.pattern, module)? {
                        if arm.guard.is_some() {
                            return Err(self.error(
                                "P004",
                                "guarded renderer match is outside the closed symbolic subset",
                            ));
                        }
                        return self.exec_pattern_arm(arm, &scrutinee, module);
                    }
                }
                Err(self.error("P004", "compile-time enum match has no matching arm"))
            }
            SymValue::MaterialIdentity { enum_key } => {
                let mut cases = Vec::new();
                for arm in arms {
                    if arm.guard.is_some() {
                        return Err(self.error(
                            "P004",
                            "material identity tables do not permit guarded arms",
                        ));
                    }
                    let identity = pattern_identity(self.programs, module, &arm.pattern, enum_key)
                        .ok_or_else(|| {
                            self.error(
                                "P004",
                                "material identity match requires one nominal variant per arm",
                            )
                        })?;
                    let value = self.branch_return_with_tail(&arm.body, tail, module)?;
                    cases.push((identity, expect_material(value)?));
                }
                cases.sort_by(|a, b| a.0.cmp(&b.0));
                let root = self.material_node(
                    MaterialKind::IdentityTable {
                        enum_key: enum_key.clone(),
                        cases,
                    },
                    module,
                    span,
                )?;
                Ok(Flow::Return(SymValue::Material(root)))
            }
            other => Err(self.error(
                "P004",
                format!("unsupported symbolic match scrutinee {other:?}"),
            )),
        }
    }

    fn exec_pattern_arm(
        &mut self,
        arm: &TypedMatchArm,
        value: &SymValue,
        module: &str,
    ) -> Result<Flow, String> {
        self.scopes.push(BTreeMap::new());
        let result = (|| {
            self.bind_pattern(&arm.pattern, value, module)?;
            self.exec_stmts(&arm.body, module)
        })();
        self.scopes.pop();
        result
    }

    fn bind_pattern(
        &mut self,
        pattern: &TypedPattern,
        value: &SymValue,
        module: &str,
    ) -> Result<(), String> {
        match &pattern.kind {
            TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => Ok(()),
            TypedPatternKind::Binding(name) => self.bind(name.clone(), value.clone()),
            TypedPatternKind::Take(pattern) => self.bind_pattern(pattern, value, module),
            TypedPatternKind::Variant { payload, .. } => {
                let SymValue::Enum(_, values) = value else {
                    return Err(self.error("P004", "variant pattern received a non-enum value"));
                };
                if payload.len() != values.len() {
                    return Err(self.error("P004", "variant pattern payload arity mismatch"));
                }
                for (pattern, value) in payload.iter().zip(values) {
                    self.bind_pattern(pattern, value, module)?;
                }
                Ok(())
            }
            TypedPatternKind::Tuple(patterns) | TypedPatternKind::Array(patterns) => {
                let values = match value {
                    SymValue::Struct(values) | SymValue::Array(values) => values,
                    _ => {
                        return Err(
                            self.error("P004", "aggregate pattern received a non-aggregate value")
                        );
                    }
                };
                if patterns.len() != values.len() {
                    return Err(self.error("P004", "aggregate pattern arity mismatch"));
                }
                for (pattern, value) in patterns.iter().zip(values) {
                    self.bind_pattern(pattern, value, module)?;
                }
                Ok(())
            }
            TypedPatternKind::Or(patterns) => {
                for pattern in patterns {
                    if self.pattern_matches_comptime(value, pattern, module)? {
                        return self.bind_pattern(pattern, value, module);
                    }
                }
                Err(self.error("P004", "or-pattern has no matching alternative"))
            }
        }
    }

    fn pattern_matches_comptime(
        &mut self,
        value: &SymValue,
        pattern: &TypedPattern,
        module: &str,
    ) -> Result<bool, String> {
        Ok(match &pattern.kind {
            TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => true,
            TypedPatternKind::Literal(literal) => {
                let literal = self.eval_expr(literal, module)?;
                self.comptime_values_equal(value, &literal)?
            }
            TypedPatternKind::Take(pattern) => {
                self.pattern_matches_comptime(value, pattern, module)?
            }
            TypedPatternKind::Variant {
                enum_name,
                variant,
                payload,
            } => {
                let SymValue::Enum(identity, values) = value else {
                    return Ok(false);
                };
                let enum_key = nominal_name(self.programs, module, enum_name)
                    .unwrap_or_else(|| enum_name.clone());
                if identity.enum_key != enum_key
                    || identity.variant != *variant
                    || payload.len() != values.len()
                {
                    false
                } else {
                    let mut matches = true;
                    for (pattern, value) in payload.iter().zip(values) {
                        matches &= self.pattern_matches_comptime(value, pattern, module)?;
                    }
                    matches
                }
            }
            TypedPatternKind::Tuple(patterns) | TypedPatternKind::Array(patterns) => {
                let values = match value {
                    SymValue::Struct(values) | SymValue::Array(values) => values,
                    _ => return Ok(false),
                };
                if patterns.len() != values.len() {
                    false
                } else {
                    let mut matches = true;
                    for (pattern, value) in patterns.iter().zip(values) {
                        matches &= self.pattern_matches_comptime(value, pattern, module)?;
                    }
                    matches
                }
            }
            TypedPatternKind::Or(patterns) => {
                let mut matches = false;
                for pattern in patterns {
                    matches |= self.pattern_matches_comptime(value, pattern, module)?;
                }
                matches
            }
        })
    }

    fn comptime_values_equal(&self, left: &SymValue, right: &SymValue) -> Result<bool, String> {
        Ok(match (left, right) {
            (SymValue::Unit, SymValue::Unit) => true,
            (SymValue::Bool(SymBool::Const(left)), SymValue::Bool(SymBool::Const(right))) => {
                left == right
            }
            (SymValue::Int(SymInt::Const(left)), SymValue::Int(SymInt::Const(right))) => {
                left == right
            }
            (SymValue::F32(left), SymValue::F32(right)) => {
                match (self.constant_value(*left), self.constant_value(*right)) {
                    (Some(left), Some(right)) => left.to_bits() == right.to_bits(),
                    _ => false,
                }
            }
            (SymValue::F64(left), SymValue::F64(right)) => {
                match (
                    self.constant_value_f64(*left),
                    self.constant_value_f64(*right),
                ) {
                    (Some(left), Some(right)) => left.to_bits() == right.to_bits(),
                    _ => false,
                }
            }
            (SymValue::Enum(left, left_values), SymValue::Enum(right, right_values)) => {
                if left != right || left_values.len() != right_values.len() {
                    false
                } else {
                    let mut equal = true;
                    for (left, right) in left_values.iter().zip(right_values) {
                        equal &= self.comptime_values_equal(left, right)?;
                    }
                    equal
                }
            }
            (SymValue::Struct(left), SymValue::Struct(right))
            | (SymValue::Array(left), SymValue::Array(right)) => {
                if left.len() != right.len() {
                    false
                } else {
                    let mut equal = true;
                    for (left, right) in left.iter().zip(right) {
                        equal &= self.comptime_values_equal(left, right)?;
                    }
                    equal
                }
            }
            _ => false,
        })
    }

    fn lookup(&self, name: &str) -> Result<SymValue, String> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .ok_or_else(|| self.error("P004", format!("unbound symbolic local `{name}`")))
    }

    fn bind(&mut self, name: String, value: SymValue) -> Result<(), String> {
        self.scopes
            .last_mut()
            .ok_or_else(|| "pixels::symbolic: missing lexical scope".to_string())?
            .insert(name, value);
        Ok(())
    }

    fn assign(&mut self, name: &str, value: SymValue) -> Result<(), String> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return Ok(());
            }
        }
        Err(self.error(
            "P004",
            format!("assignment to unbound symbolic local `{name}`"),
        ))
    }

    fn compile_root(&mut self, key: &str, kind: PixelsFnKind) -> Result<SymValue, String> {
        self.kind = kind.clone();
        let owner = self.programs.get(self.owner_module).ok_or_else(|| {
            format!(
                "pixels::symbolic: owner module `{}` is absent",
                self.owner_module
            )
        })?;
        let root = root_function(owner, self.programs, key)?;
        let mut args = Vec::new();
        match kind {
            PixelsFnKind::Field => {
                let origin = self.origin(&root.module, Span::default());
                let x = self.scalar_node(
                    ScalarOp::CoordX,
                    Dependency::Coordinate,
                    &root.module,
                    origin.primary.span,
                )?;
                let y = self.scalar_node(
                    ScalarOp::CoordY,
                    Dependency::Coordinate,
                    &root.module,
                    origin.primary.span,
                )?;
                let z = self.scalar_node(
                    ScalarOp::CoordZ,
                    Dependency::Coordinate,
                    &root.module,
                    origin.primary.span,
                )?;
                args.push(SymValue::Vec3(SymVec3 {
                    values: [x, y, z],
                    transforms: Vec::new(),
                    coordinate_provenance: true,
                }));
            }
            PixelsFnKind::Material => {
                let enum_key =
                    canonical_type_key(self.programs, &root.module, &self.config.material_type);
                let px = self.scalar_node(
                    ScalarOp::SurfacePosition(0),
                    Dependency::Surface,
                    &root.module,
                    Span::default(),
                )?;
                let py = self.scalar_node(
                    ScalarOp::SurfacePosition(1),
                    Dependency::Surface,
                    &root.module,
                    Span::default(),
                )?;
                let pz = self.scalar_node(
                    ScalarOp::SurfacePosition(2),
                    Dependency::Surface,
                    &root.module,
                    Span::default(),
                )?;
                let nx = self.scalar_node(
                    ScalarOp::SurfaceNormal(0),
                    Dependency::Surface,
                    &root.module,
                    Span::default(),
                )?;
                let ny = self.scalar_node(
                    ScalarOp::SurfaceNormal(1),
                    Dependency::Surface,
                    &root.module,
                    Span::default(),
                )?;
                let nz = self.scalar_node(
                    ScalarOp::SurfaceNormal(2),
                    Dependency::Surface,
                    &root.module,
                    Span::default(),
                )?;
                args.push(SymValue::Struct(vec![
                    SymValue::MaterialIdentity { enum_key },
                    SymValue::Vec3(SymVec3 {
                        values: [px, py, pz],
                        transforms: Vec::new(),
                        coordinate_provenance: false,
                    }),
                    SymValue::Vec3(SymVec3 {
                        values: [nx, ny, nz],
                        transforms: Vec::new(),
                        coordinate_provenance: false,
                    }),
                ]));
            }
        }
        if root.function.params.len() > 1 {
            let params_type = root
                .function
                .params
                .get(1)
                .map(|param| param.ty.clone())
                .ok_or_else(|| self.error("P004", "renderer root lacks parameter type"))?;
            args.push(SymValue::Param(ParamProxy {
                ty: params_type,
                path: Vec::new(),
                component: None,
            }));
        }
        let root_module = root.module.clone();
        self.eval_function(
            root,
            args.into_iter().map(Some).collect(),
            &root_module,
            Span::default(),
        )
    }
}

pub(crate) fn compile(
    programs: &BTreeMap<String, TypedProgram>,
    owner_module: &str,
    config: &RendererConfig,
    renderer_index: usize,
) -> Result<SymbolicGraph, SymbolicFailure> {
    let mut compiler =
        Compiler::new(programs, owner_module, config, renderer_index).map_err(|message| {
            SymbolicFailure {
                message,
                primary: Span::default(),
            }
        })?;
    let field_root = compiler
        .compile_root(&config.field, PixelsFnKind::Field)
        .and_then(expect_field)
        .map_err(|message| SymbolicFailure {
            message,
            primary: compiler.last_span.get(),
        })?;
    let material_root = compiler
        .compile_root(&config.material, PixelsFnKind::Material)
        .and_then(expect_material)
        .map_err(|message| SymbolicFailure {
            message,
            primary: compiler.last_span.get(),
        })?;
    let owner = programs.get(owner_module).ok_or_else(|| SymbolicFailure {
        message: format!("pixels::symbolic: owner module `{owner_module}` is absent"),
        primary: compiler.last_span.get(),
    })?;
    let field =
        root_function(owner, programs, &config.field).map_err(|message| SymbolicFailure {
            message,
            primary: compiler.last_span.get(),
        })?;
    let material =
        root_function(owner, programs, &config.material).map_err(|message| SymbolicFailure {
            message,
            primary: compiler.last_span.get(),
        })?;
    Ok(SymbolicGraph {
        renderer_index,
        field_key: format!("{}::{}", field.module, field.decl_name),
        material_key: format!("{}::{}", material.module, material.decl_name),
        params_type: config.params_type.clone(),
        material_type: config.material_type.clone(),
        params: compiler.params,
        scalar: compiler.scalar,
        fields: compiler.fields,
        materials: compiler.materials,
        field_root,
        material_root,
        obligations: compiler.obligations,
        quota: compiler.quota,
    })
}

fn expect_f32(value: SymValue) -> Result<ScalarId, String> {
    match value {
        SymValue::F32(value) => Ok(value),
        other => Err(format!("pixels::symbolic: expected f32, found {other:?}")),
    }
}

fn expect_field(value: SymValue) -> Result<FieldId, String> {
    match value {
        SymValue::Field(value) => Ok(value),
        other => Err(format!("pixels::symbolic: expected Field, found {other:?}")),
    }
}

fn expect_material(value: SymValue) -> Result<MaterialId, String> {
    match value {
        SymValue::Material(value) => Ok(value),
        other => Err(format!(
            "pixels::symbolic: expected MaterialSample, found {other:?}"
        )),
    }
}

fn expect_identity(value: SymValue) -> Result<CanonicalIdentity, String> {
    match value {
        SymValue::Enum(identity, payload) if payload.is_empty() => Ok(identity),
        other => Err(format!(
            "pixels::symbolic: expected payload-free nominal identity, found {other:?}"
        )),
    }
}

fn expect_const_int(value: SymValue) -> Result<i128, String> {
    match value {
        SymValue::Int(SymInt::Const(value)) => Ok(value),
        other => Err(format!(
            "pixels::symbolic: expected compile-time integer, found {other:?}"
        )),
    }
}

fn expect_runtime_bool(value: SymValue) -> Result<ScalarId, String> {
    match value {
        SymValue::Bool(SymBool::Runtime(value)) => Ok(value),
        other => Err(format!(
            "pixels::symbolic: expected runtime predicate, found {other:?}"
        )),
    }
}

fn required_args(
    values: Vec<Option<SymValue>>,
    missing: impl Fn() -> String,
) -> Result<Vec<SymValue>, String> {
    values
        .into_iter()
        .map(|value| value.ok_or_else(&missing))
        .collect()
}

fn checked_int(a: i128, b: i128, ty: &Type, op: fn(i128, i128) -> Option<i128>) -> Option<i128> {
    let value = op(a, b)?;
    int_fits(value, ty).then_some(value)
}

fn int_fits(value: i128, ty: &Type) -> bool {
    crate::eval::value::int_bounds(ty).is_some_and(|(min, max)| (min..=max).contains(&value))
}

fn wrapping_int(a: i128, b: i128, ty: &Type, op: fn(i128, i128) -> i128) -> Option<i128> {
    let (bits, signed) = match ty {
        Type::U8 => (8, false),
        Type::U16 => (16, false),
        Type::U32 => (32, false),
        Type::U64 | Type::Usize => (64, false),
        Type::I8 => (8, true),
        Type::I16 => (16, true),
        Type::I32 => (32, true),
        Type::I64 | Type::Isize => (64, true),
        _ => return None,
    };
    let mask = (1_u128 << bits) - 1;
    let pattern = (op(a, b) as u128) & mask;
    if signed && pattern & (1_u128 << (bits - 1)) != 0 {
        Some((pattern as i128) - (1_i128 << bits))
    } else {
        Some(pattern as i128)
    }
}

fn semantic_tag(semantic: SemanticOpId) -> u16 {
    match semantic {
        SemanticOpId::SqrtF32V1 => 0,
        SemanticOpId::RsqrtF32V1 => 1,
        SemanticOpId::SinRestrictedF32V1 => 2,
        SemanticOpId::CosRestrictedF32V1 => 3,
        SemanticOpId::Normalize3F32V1 => 4,
        SemanticOpId::SmoothMinF32V1 => 5,
        SemanticOpId::FiniteColorF32V1 => 6,
        SemanticOpId::MaterialRoughnessF32V1 => 7,
    }
}

fn compare_structural_keys(
    arena: &[StructuralKeyNode],
    left: u32,
    right: u32,
    memo: &mut BTreeMap<(u32, u32), std::cmp::Ordering>,
    depth: usize,
) -> Result<std::cmp::Ordering, String> {
    use std::cmp::Ordering;

    if left == right {
        return Ok(Ordering::Equal);
    }
    if let Some(ordering) = memo.get(&(left, right)) {
        return Ok(*ordering);
    }
    if depth >= super::capacities::PixelsCeilings::MACHINE_V1.structural_depth as usize {
        return Err("P014: structural-key comparison depth quota exhausted".to_string());
    }
    let left_node = arena
        .get(left as usize)
        .ok_or_else(|| "pixels::symbolic: missing structural-key node".to_string())?;
    let right_node = arena
        .get(right as usize)
        .ok_or_else(|| "pixels::symbolic: missing structural-key node".to_string())?;
    let mut ordering = left_node.local.cmp(&right_node.local);
    if ordering == Ordering::Equal {
        for (left_child, right_child) in left_node.children.iter().zip(&right_node.children) {
            ordering = compare_structural_keys(arena, *left_child, *right_child, memo, depth + 1)?;
            if ordering != Ordering::Equal {
                break;
            }
        }
        if ordering == Ordering::Equal {
            ordering = left_node.children.len().cmp(&right_node.children.len());
        }
    }
    memo.insert((left, right), ordering);
    memo.insert((right, left), ordering.reverse());
    Ok(ordering)
}

fn has_discontinuous_transform(transforms: &[LocatedCoordTransform]) -> bool {
    transforms
        .iter()
        .any(|transform| matches!(transform.kind, CoordTransform::Repeat { .. }))
}

fn is_scalar_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::F32
            | Type::F64
            | Type::U8
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

fn is_integer_type(ty: &Type) -> bool {
    is_scalar_type(ty) && !matches!(ty, Type::F32 | Type::F64)
}

fn nominal_name(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    visible: &str,
) -> Option<String> {
    let program = super::program_for_decl_module(programs, module)?;
    let (declaring_module, name) = super::nominal_decl(program, visible)?;
    Some(format!("{declaring_module}::{name}"))
}

fn canonical_type_key(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    ty: &Type,
) -> String {
    let Type::Named(name, _) = ty else {
        return crate::sema::types::render_type(ty);
    };
    nominal_name(programs, module, name).unwrap_or_else(|| name.clone())
}

fn enum_identity(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    enum_name: &str,
    variant: &str,
) -> CanonicalIdentity {
    CanonicalIdentity {
        enum_key: nominal_name(programs, module, enum_name)
            .unwrap_or_else(|| enum_name.to_string()),
        variant: variant.to_string(),
    }
}

fn find_struct<'a>(
    programs: &'a BTreeMap<String, TypedProgram>,
    module: &str,
    ty: &Type,
) -> Option<&'a crate::sema::typed::TypedStruct> {
    super::typed_struct_decl(programs, module, ty).map(|(_, strukt)| strukt)
}

fn struct_field(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    ty: &Type,
    field: &str,
) -> Option<(usize, Type)> {
    let strukt = find_struct(programs, module, ty)?;
    let index = strukt.fields.iter().position(|name| name == field)?;
    Some((index, strukt.field_types.get(field)?.clone()))
}

fn vector_components(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    ty: &Type,
) -> Option<u8> {
    let Type::Named(name, args) = ty else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let program = super::program_for_decl_module(programs, module)?;
    let (declaring_module, declaration) = super::nominal_decl(program, name)?;
    if !matches!(declaring_module, "field" | "core.field") {
        return None;
    }
    match declaration {
        "Vec2" => Some(2),
        "Vec3" | "Rgb" => Some(3),
        "Vec4" => Some(4),
        _ => None,
    }
}

fn vector_field_component(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    ty: &Type,
    field: &str,
) -> Option<u8> {
    let count = vector_components(programs, module, ty)?;
    let component = match field {
        "x" | "r" => 0,
        "y" | "g" => 1,
        "z" | "b" => 2,
        "w" => 3,
        _ => return None,
    };
    (component < count).then_some(component)
}

fn vector_field_index(field: &str) -> Option<usize> {
    match field {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        _ => None,
    }
}

fn rgb_field_index(field: &str) -> Option<usize> {
    match field {
        "r" => Some(0),
        "g" => Some(1),
        "b" => Some(2),
        _ => None,
    }
}

fn param_spelling(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    root_ty: &Type,
    path: &[usize],
    component: Option<u8>,
) -> String {
    let mut ty = root_ty.clone();
    let mut current_module = module.to_string();
    let mut spelling = crate::sema::types::render_type(root_ty);
    for index in path {
        match &ty {
            Type::Array(element, _) => {
                spelling.push_str(&format!("[{index}]"));
                ty = (**element).clone();
            }
            Type::Tuple(items) => {
                spelling.push_str(&format!(".{index}"));
                if let Some(next) = items.get(*index) {
                    ty = next.clone();
                }
            }
            Type::Named(_, _) => {
                if let Some(strukt) = find_struct(programs, &current_module, &ty)
                    && let Some(name) = strukt.fields.get(*index)
                {
                    spelling.push('.');
                    spelling.push_str(name);
                    if let Some(next) = strukt.field_types.get(name) {
                        ty = next.clone();
                    }
                }
                if let Type::Named(name, _) = &ty
                    && let Some(program) = super::program_for_decl_module(programs, &current_module)
                    && let Some((declaring_module, _)) = super::nominal_decl(program, name)
                {
                    current_module = declaring_module.to_string();
                }
            }
            _ => spelling.push_str(&format!(".#{index}")),
        }
    }
    if let Some(component) = component {
        spelling.push('.');
        let is_rgb = match &ty {
            Type::Named(name, args) if args.is_empty() => {
                super::program_for_decl_module(programs, &current_module)
                    .and_then(|program| super::nominal_decl(program, name))
                    .is_some_and(|(module, declaration)| {
                        matches!(module, "field" | "core.field") && declaration == "Rgb"
                    })
            }
            _ => false,
        };
        spelling.push(if is_rgb {
            match component {
                0 => 'r',
                1 => 'g',
                _ => 'b',
            }
        } else {
            match component {
                0 => 'x',
                1 => 'y',
                2 => 'z',
                _ => 'w',
            }
        });
    }
    spelling
}

fn pattern_identity(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    pattern: &TypedPattern,
    fallback_enum_key: &str,
) -> Option<CanonicalIdentity> {
    match &pattern.kind {
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } if payload.is_empty() => Some(CanonicalIdentity {
            enum_key: nominal_name(programs, module, enum_name)
                .unwrap_or_else(|| fallback_enum_key.to_string()),
            variant: variant.clone(),
        }),
        TypedPatternKind::Take(pattern) => {
            pattern_identity(programs, module, pattern, fallback_enum_key)
        }
        _ => None,
    }
}

fn expr_kind_name(kind: &TypedExprKind) -> &'static str {
    match kind {
        TypedExprKind::FnRef(_) => "function reference",
        TypedExprKind::Static(_) => "static",
        TypedExprKind::Str(_) | TypedExprKind::BStr(_) | TypedExprKind::Char(_) => "text literal",
        TypedExprKind::BitNot(_) => "bitwise not",
        TypedExprKind::Try(_, _) => "try",
        TypedExprKind::CallValue(_, _) => "indirect call",
        TypedExprKind::Closure { .. } => "closure",
        TypedExprKind::Panic(_) => "panic",
        TypedExprKind::PoolName(_) => "pool",
        TypedExprKind::Await(_) => "await",
        TypedExprKind::Send(_) => "send",
        TypedExprKind::GroupChild(_) => "group child",
        _ => "expression",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::config::{RgbRangeConfig, ScalarRangeConfig, Vec3Config};

    fn test_config() -> RendererConfig {
        RendererConfig {
            declaration_index: 0,
            worker_count: 1,
            params_type: Type::Unit,
            field: "world".to_string(),
            material: "shade".to_string(),
            material_type: Type::Unit,
            display_index: 0,
            display_doorbell_addr: wrela_machine::pixels::DOORBELL_ADDR,
            width: 1,
            height: 1,
            refresh_hz: 60,
            shade_hz: 60,
            profile: "RenderProfile.AaaByteExact".to_string(),
            tone_curve: "ToneCurve.Linear".to_string(),
            near: 0.1,
            far: 10.0,
            world_min: Vec3Config {
                x: -1.0,
                y: -1.0,
                z: -1.0,
            },
            world_max: Vec3Config {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
            camera_pose: None,
            camera_max_motion: 0.0,
            light_capacity: 0,
            light_kinds: Vec::new(),
            light_ranges: super::super::config::default_light_ranges(),
            exposure: ScalarRangeConfig { min: 0.0, max: 1.0 },
            environment: RgbRangeConfig {
                min: [0.0; 3],
                max: [1.0; 3],
            },
            ao_enabled: false,
            ao_radius: 1.0,
            ao_strength: 1.0,
            probes_enabled: false,
            probes_static_preinitialized: false,
            probe_levels: 0,
            probe_dims: [0; 3],
            probe_base_spacing: 1.0,
            probe_initialization_worst_case_ms: 0,
            initialization_deadline_ms: 1,
            parameter_contracts: Vec::new(),
        }
    }

    fn test_compiler<'a>(
        programs: &'a BTreeMap<String, TypedProgram>,
        config: &'a RendererConfig,
    ) -> Compiler<'a> {
        Compiler {
            programs,
            owner_module: "scene",
            config,
            kind: PixelsFnKind::Field,
            call_stack: Vec::new(),
            call_sites: Vec::new(),
            scopes: Vec::new(),
            scalar: ScalarArena::new(1),
            fields: FieldArena::new(2),
            materials: MaterialArena::new(3),
            scalar_cse: BTreeMap::new(),
            field_cse: BTreeMap::new(),
            material_cse: BTreeMap::new(),
            structural_keys: Vec::new(),
            structural_key_cse: BTreeMap::new(),
            field_structural_keys: BTreeMap::new(),
            scalar_structural_keys: BTreeMap::new(),
            params: Vec::new(),
            param_ids: BTreeMap::new(),
            obligations: Vec::new(),
            quota: SymbolicQuota::default(),
            last_span: Cell::new(Span::default()),
        }
    }

    #[test]
    fn stable_identity_is_nominal_not_a_discriminant() {
        let identity = CanonicalIdentity {
            enum_key: "game.scene::MaterialId".to_string(),
            variant: "Clay".to_string(),
        };
        assert_eq!(identity.enum_key, "game.scene::MaterialId");
        assert_eq!(identity.variant, "Clay");
    }

    #[test]
    fn renderer_compilers_use_distinct_debug_arena_identities() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut first = Compiler::new(&programs, "scene", &config, 0).unwrap();
        let mut second = Compiler::new(&programs, "scene", &config, 1).unwrap();
        let first_id = first
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("first"),
            )
            .unwrap();
        second
            .scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32(0),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("second"),
            )
            .unwrap();
        let debug_id = first.scalar.debug_id(first_id).unwrap();
        assert!(second.scalar.debug_get(debug_id).is_err());
    }

    #[test]
    fn nonlinear_coordinate_taint_is_not_canonical_coordinate_provenance() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let span = Span::default();
        let x = compiler
            .scalar_node(ScalarOp::CoordX, Dependency::Coordinate, "scene", span)
            .unwrap();
        let half = compiler.const_f32(0.5, "scene", span).unwrap();
        let nonlinear_x = compiler.mul(x, half, "scene", span).unwrap();
        let y = compiler
            .scalar_node(ScalarOp::CoordY, Dependency::Coordinate, "scene", span)
            .unwrap();
        let z = compiler
            .scalar_node(ScalarOp::CoordZ, Dependency::Coordinate, "scene", span)
            .unwrap();
        let zero = compiler.const_f32(0.0, "scene", span).unwrap();
        let one = compiler.const_f32(1.0, "scene", span).unwrap();
        let error = compiler
            .eval_primitive(
                FieldIntrinsic::Sphere,
                vec![
                    SymValue::Vec3(SymVec3 {
                        values: [nonlinear_x, y, z],
                        transforms: Vec::new(),
                        coordinate_provenance: false,
                    }),
                    SymValue::Vec3(SymVec3 {
                        values: [zero; 3],
                        transforms: Vec::new(),
                        coordinate_provenance: false,
                    }),
                    SymValue::F32(one),
                ],
                "scene",
                span,
            )
            .unwrap_err();
        assert!(error.contains("point argument must be derived"));
    }

    #[test]
    fn scale_and_deformation_are_partitioned_through_hard_union() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let span = Span::default();
        let zero = compiler.const_f32(0.0, "scene", span).unwrap();
        let one = compiler.const_f32(1.0, "scene", span).unwrap();
        let left = compiler
            .field_node(
                FieldKind::Primitive(Primitive::Sphere {
                    center: [zero; 3],
                    radius: one,
                }),
                zero,
                "scene",
                span,
            )
            .unwrap();
        let right = compiler
            .field_node(
                FieldKind::Primitive(Primitive::Sphere {
                    center: [one, zero, zero],
                    radius: one,
                }),
                one,
                "scene",
                span,
            )
            .unwrap();
        let hard = compiler
            .field_binary(FieldIntrinsic::Union, left, right, "scene", span)
            .unwrap();
        let scale = compiler.const_f32(2.0, "scene", span).unwrap();
        let scaled = compiler
            .uniform_scale_field(hard, scale, "scene", span)
            .unwrap();
        let FieldKind::HardUnion { a, b } = compiler.fields.get(scaled).unwrap().kind else {
            panic!("uniform scale must expose the hard frontier");
        };
        assert!(matches!(
            compiler.fields.get(a).unwrap().kind,
            FieldKind::Transform { .. }
        ));
        assert!(matches!(
            compiler.fields.get(b).unwrap().kind,
            FieldKind::Transform { .. }
        ));
        let contract = DerivedDeformContract {
            amplitude_bound: one,
            gradient_bound: one,
            hessian_bound: one,
            third_derivative_bound: one,
            coordinate_x: zero,
            frequency: one,
            phase: zero,
            derivation: ClosedDeformDerivation::SinusoidalX,
        };
        let displaced = compiler
            .displace_hard_partitioned(hard, one, &contract, "scene", span)
            .unwrap();
        let FieldKind::HardUnion { a, b } = compiler.fields.get(displaced).unwrap().kind else {
            panic!("deformation must expose the hard frontier");
        };
        assert!(matches!(
            compiler.fields.get(a).unwrap().kind,
            FieldKind::BoundedDisplace { .. }
        ));
        assert!(matches!(
            compiler.fields.get(b).unwrap().kind,
            FieldKind::BoundedDisplace { .. }
        ));
    }

    #[test]
    fn quota_failure_is_a_pixels_error_with_call_chain() {
        let mut quota = SymbolicQuota {
            max_steps: 0,
            ..Default::default()
        };
        let error = quota.step(&["scene::world".to_string()]).unwrap_err();
        assert!(error.starts_with("P014:"));
        assert!(error.contains("scene::world"));
    }

    #[test]
    fn dynamic_parameter_indexing_charges_each_materialized_alternative() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        compiler.quota.max_aggregate_elements = 1;
        let base = SymValue::Param(ParamProxy {
            ty: Type::Array(
                Box::new(Type::F32),
                Box::new(crate::syntax::ast::Expr::Int(
                    Span::default(),
                    "2".to_string(),
                )),
            ),
            path: Vec::new(),
            component: None,
        });
        let error = compiler
            .index_value(
                base,
                SymValue::Int(SymInt::Runtime(ScalarId(0))),
                "scene",
                Span::default(),
            )
            .unwrap_err();
        assert!(error.starts_with("P014:"));
        assert!(error.contains("symbolic-memory"));
    }

    #[test]
    fn structural_keys_are_linear_for_shared_subtree_dags() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        compiler.quota.max_aggregate_elements = 64;
        let zero = compiler.const_f32(0.0, "scene", Span::default()).unwrap();
        let one = compiler.const_f32(1.0, "scene", Span::default()).unwrap();
        let length = compiler
            .length3([zero; 3], "scene", Span::default())
            .unwrap();
        let scalar = compiler.sub(length, one, "scene", Span::default()).unwrap();
        let mut root = compiler
            .field_node(
                FieldKind::Primitive(Primitive::Sphere {
                    center: [zero; 3],
                    radius: one,
                }),
                scalar,
                "scene",
                Span::default(),
            )
            .unwrap();
        for _ in 0..24 {
            root = compiler
                .field_binary(FieldIntrinsic::Union, root, root, "scene", Span::default())
                .unwrap();
        }
        assert!(compiler.fields.get(root).is_ok());
    }

    #[test]
    fn structural_key_construction_is_iterative_for_deep_field_chains() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let zero = compiler.const_f32(0.0, "scene", Span::default()).unwrap();
        let mut root = compiler
            .fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(Primitive::Plane {
                        normal: [zero; 3],
                        offset: zero,
                    }),
                    scalar_value: zero,
                },
                NodeOrigin::synthetic("deep-root"),
            )
            .unwrap();
        for _ in 0..65_536 {
            root = compiler
                .fields
                .push(
                    FieldNode {
                        kind: FieldKind::Transform {
                            child: root,
                            transform: TransformProgram::Translate { by: [zero; 3] },
                        },
                        scalar_value: zero,
                    },
                    NodeOrigin::synthetic("deep-transform"),
                )
                .unwrap();
        }
        let key = compiler
            .build_structural_key(StructuralSource::Field(root))
            .unwrap();
        assert_eq!(
            compiler.field_structural_keys.get(&root),
            Some(&key),
            "the deepest field receives a cached key without recursive descent"
        );
    }

    #[test]
    fn total_guarded_intrinsics_do_not_emit_unconditional_obligations() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let runtime = compiler
            .scalar_node(
                ScalarOp::CoordX,
                Dependency::Coordinate,
                "scene",
                Span::default(),
            )
            .unwrap();
        let zero = compiler.const_f32(0.0, "scene", Span::default()).unwrap();
        compiler
            .eval_scalar_intrinsic(
                ScalarIntrinsic::Rsqrt,
                vec![SymValue::F32(runtime)],
                "core.field",
                Span::default(),
            )
            .unwrap();
        compiler
            .eval_scalar_intrinsic(
                ScalarIntrinsic::Normalize3,
                vec![SymValue::Vec3(SymVec3 {
                    values: [runtime, zero, zero],
                    transforms: Vec::new(),
                    coordinate_provenance: false,
                })],
                "core.field",
                Span::default(),
            )
            .unwrap();
        assert!(
            compiler.obligations.is_empty(),
            "source guards make rsqrt and normalize total at zero"
        );
    }

    #[test]
    fn transformed_field_nodes_retain_transform_origin_and_expansion_chain() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let zero = compiler.const_f32(0.0, "scene", Span::default()).unwrap();
        let one = compiler.const_f32(1.0, "scene", Span::default()).unwrap();
        let coordinates = [
            compiler
                .scalar_node(
                    ScalarOp::CoordX,
                    Dependency::Coordinate,
                    "scene",
                    Span::default(),
                )
                .unwrap(),
            compiler
                .scalar_node(
                    ScalarOp::CoordY,
                    Dependency::Coordinate,
                    "scene",
                    Span::default(),
                )
                .unwrap(),
            compiler
                .scalar_node(
                    ScalarOp::CoordZ,
                    Dependency::Coordinate,
                    "scene",
                    Span::default(),
                )
                .unwrap(),
        ];
        let call_span = Span {
            line: 7,
            col: 12,
            byte_start: 83,
            byte_end: 92,
        };
        let caller = OriginSite {
            module: "scene".to_string(),
            span: Span {
                line: 15,
                col: 8,
                byte_start: 210,
                byte_end: 225,
            },
        };
        let transform_origin =
            NodeOrigin::new("helpers.transforms", call_span, vec![caller.clone()]);
        let point = SymValue::Vec3(SymVec3 {
            values: coordinates,
            transforms: vec![LocatedCoordTransform {
                kind: CoordTransform::Translate([one, zero, zero]),
                origin: transform_origin,
            }],
            coordinate_provenance: true,
        });
        let center = SymValue::Vec3(SymVec3 {
            values: [zero; 3],
            transforms: Vec::new(),
            coordinate_provenance: false,
        });
        let SymValue::Field(root) = compiler
            .eval_primitive(
                FieldIntrinsic::Sphere,
                vec![point, center, SymValue::F32(one)],
                "core.field",
                Span::default(),
            )
            .unwrap()
        else {
            panic!("sphere lowering returns a field");
        };
        let origin = compiler.fields.origin(root).unwrap();
        assert_eq!(origin.primary.module, "helpers.transforms");
        assert_eq!(origin.primary.span, call_span);
        assert_eq!(origin.expansion_chain, vec![caller]);
    }

    #[test]
    fn pre_canonical_field_graph_preserves_adjacent_source_transforms() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let zero = compiler.const_f32(0.0, "scene", Span::default()).unwrap();
        let one = compiler.const_f32(1.0, "scene", Span::default()).unwrap();
        let transforms = vec![
            LocatedCoordTransform {
                kind: CoordTransform::Translate([one, zero, zero]),
                origin: NodeOrigin::synthetic("first"),
            },
            LocatedCoordTransform {
                kind: CoordTransform::Translate([zero, one, zero]),
                origin: NodeOrigin::synthetic("second"),
            },
        ];
        let layers = compiler.field_transform_layers(&transforms).unwrap();
        let [FieldTransformLayer::Program { transform, origins }] = layers.as_slice() else {
            panic!("adjacent rigid transforms must remain one source-sequence layer");
        };
        let TransformProgram::SourceRigidSequence { steps, .. } = transform else {
            panic!("composition must remain deferred until canonicalization");
        };
        assert_eq!(steps.len(), 2);
        assert_eq!(origins.len(), 2);
    }

    #[test]
    fn exact_scalar_helpers_preserve_signed_zero_and_smooth_saturation() {
        assert_eq!(
            source_min(-0.0, 0.0).to_bits(),
            0.0f32.to_bits(),
            "source min returns its right operand when values compare equal"
        );
        assert_eq!(
            source_max(0.0, -0.0).to_bits(),
            (-0.0f32).to_bits(),
            "source max returns its right operand when values compare equal"
        );
        let selected = -3.25f32;
        assert_eq!(
            source_smooth_min(selected, 2.0, 0.5).to_bits(),
            selected.to_bits()
        );
        assert_eq!(source_smooth_min(1.0, 1.0, 4.0), 0.0);

        let boundary = 1.5_f32;
        let one_ulp_inside = f32::from_bits(boundary.to_bits() - 1);
        assert_eq!(
            source_smooth_min(1.0, boundary, 0.5).to_bits(),
            1.0_f32.to_bits(),
            "the saturated branch returns the selected operand verbatim"
        );
        assert_eq!(
            source_smooth_min(1.0, one_ulp_inside, 0.5).to_bits(),
            0.999_999_94_f32.to_bits(),
            "one ulp inside the blend must retain the interior polynomial result"
        );
    }

    #[test]
    fn material_roughness_preserves_negative_zero_like_source_comparisons() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let zero = compiler.const_f32(0.0, "scene", Span::default()).unwrap();
        let negative_zero = compiler.const_f32(-0.0, "scene", Span::default()).unwrap();
        let SymValue::Material(material) = compiler
            .eval_material_constructor(
                MaterialIntrinsic::Standard,
                vec![
                    Some(SymValue::Rgb([zero; 3])),
                    Some(SymValue::F32(negative_zero)),
                ],
                "scene",
                Span::default(),
            )
            .unwrap()
        else {
            panic!("material constructor must return a material node");
        };
        let MaterialKind::Sample(sample) = &compiler.materials.get(material).unwrap().kind else {
            panic!("standard material constructor must produce a sample");
        };
        assert_eq!(
            compiler.constant_value(sample.roughness).unwrap().to_bits(),
            (-0.0f32).to_bits()
        );
    }

    #[test]
    fn compile_time_boolean_operators_short_circuit_dead_rhs() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let int = |value: &str| TypedExpr {
            ty: Type::U32,
            span: Default::default(),
            kind: TypedExprKind::Int(value.to_string()),
        };
        let dead_division = TypedExpr {
            ty: Type::U32,
            span: Default::default(),
            kind: TypedExprKind::Binary(BinOp::Div, Box::new(int("1")), Box::new(int("0"))),
        };
        let dead_predicate = TypedExpr {
            ty: Type::Bool,
            span: Default::default(),
            kind: TypedExprKind::Binary(BinOp::Eq, Box::new(dead_division), Box::new(int("0"))),
        };
        let boolean = |value| TypedExpr {
            ty: Type::Bool,
            span: Default::default(),
            kind: TypedExprKind::Bool(value),
        };
        let and = TypedExpr {
            ty: Type::Bool,
            span: Default::default(),
            kind: TypedExprKind::And(Box::new(boolean(false)), Box::new(dead_predicate.clone())),
        };
        let or = TypedExpr {
            ty: Type::Bool,
            span: Default::default(),
            kind: TypedExprKind::Or(Box::new(boolean(true)), Box::new(dead_predicate)),
        };
        assert_eq!(
            compiler.eval_expr(&and, "scene").unwrap(),
            SymValue::Bool(SymBool::Const(false))
        );
        assert_eq!(
            compiler.eval_expr(&or, "scene").unwrap(),
            SymValue::Bool(SymBool::Const(true))
        );
    }

    #[test]
    fn f64_division_records_its_exact_denominator_obligation() {
        let programs = BTreeMap::new();
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let numerator = compiler.const_f64(1.0, "scene", Span::default()).unwrap();
        let denominator = compiler.const_f64(-0.0, "scene", Span::default()).unwrap();
        compiler
            .eval_binary(
                BinOp::Div,
                SymValue::F64(numerator),
                SymValue::F64(denominator),
                &Type::F64,
                "scene",
                Span::default(),
            )
            .unwrap();
        assert!(compiler.obligations.contains(&PendingObligation::Scalar(
            ProofObligation::DenominatorNonZero { denominator }
        )));
    }

    #[test]
    fn comptime_enum_match_checks_literal_payloads_and_or_alternatives() {
        let source = r#"module scene
enum Pick:
    K(u32)
fn choose() -> f32:
    match Pick.K(2):
        case Pick.K(1):
            return 0.25
        case Pick.K(0 | 2):
            return 0.75
        case _:
            return 1.0
"#;
        let tokens = crate::syntax::lexer::lex(source).unwrap();
        let module = crate::syntax::parser::parse(tokens).unwrap();
        let program = crate::sema::check_typed(&module, "<literal-enum-match>").unwrap();
        let programs = BTreeMap::from([("scene".to_string(), program)]);
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let choose = called_function(&programs, "scene", "choose").unwrap();
        let value = compiler
            .eval_function(choose, Vec::new(), "scene", Span::default())
            .unwrap();
        let value = expect_f32(value).unwrap();
        assert_eq!(compiler.constant_value(value), Some(0.75));
    }

    #[test]
    fn user_type_names_ending_in_core_type_names_are_not_hijacked() {
        let source = r#"module scene
struct MyVec3:
    x: f32
    y: f32
    z: f32

    fn subtract(read self, other: MyVec3) -> MyVec3:
        return MyVec3(x=self.x - other.x, y=self.y - other.y, z=self.z - other.z)

fn choose() -> f32:
    a = MyVec3(x=3.0, y=4.0, z=5.0)
    b = MyVec3(x=1.0, y=1.0, z=1.0)
    return a.subtract(b).x
"#;
        let tokens = crate::syntax::lexer::lex(source).unwrap();
        let module = crate::syntax::parser::parse(tokens).unwrap();
        let program = crate::sema::check_typed(&module, "<suffix-counterfeit>").unwrap();
        let programs = BTreeMap::from([("scene".to_string(), program)]);
        let config = test_config();
        let mut compiler = test_compiler(&programs, &config);
        let choose = called_function(&programs, "scene", "choose").unwrap();
        let value = compiler
            .eval_function(choose, Vec::new(), "scene", Span::default())
            .unwrap();
        let value = expect_f32(value).unwrap();
        assert_eq!(compiler.constant_value(value), Some(2.0));
    }

    #[test]
    fn integer_arithmetic_honors_checked_and_wrapping_result_widths() {
        assert_eq!(
            wrapping_int(250, 10, &Type::U8, i128::wrapping_add),
            Some(4)
        );
        assert_eq!(
            wrapping_int(i8::MAX.into(), 1, &Type::I8, i128::wrapping_add),
            Some(i8::MIN.into())
        );
        assert_eq!(checked_int(250, 10, &Type::U8, i128::checked_add), None);
        assert_eq!(checked_int(250, 5, &Type::U8, i128::checked_add), Some(255));
    }

    #[test]
    fn finite_repeat_instance_conversion_preserves_source_rounding_order() {
        let first = 16_777_217_i32;
        let index = 1_u32;
        let source = first as f32 + index as f32;
        let incorrectly_fused = (i64::from(first) + i64::from(index)) as f32;
        assert_ne!(source.to_bits(), incorrectly_fused.to_bits());
        assert_eq!(source.to_bits(), 16_777_216_f32.to_bits());
    }

    #[test]
    fn displacement_contract_rejects_repeat_discontinuities() {
        let located = |kind| LocatedCoordTransform {
            kind,
            origin: NodeOrigin::synthetic("<test-transform>"),
        };
        assert!(has_discontinuous_transform(&[located(
            CoordTransform::Repeat {
                axis: Axis::X,
                first: -1,
                count: 3,
                period: ScalarId(9),
            }
        )]));
        assert!(!has_discontinuous_transform(&[
            located(CoordTransform::Translate([ScalarId(0); 3])),
            located(CoordTransform::Rotate {
                row_x: [ScalarId(1); 3],
                row_y: [ScalarId(2); 3],
                row_z: [ScalarId(3); 3],
            }),
        ]));
    }

    #[test]
    fn helper_declaration_reordering_preserves_compiled_symbolic_node_order() {
        fn check(source: &str) -> TypedProgram {
            let tokens = crate::syntax::lexer::lex(source).unwrap();
            let module = crate::syntax::parser::parse(tokens).unwrap();
            crate::sema::check_typed(&module, "<helper-order>").unwrap()
        }

        let alpha_then_beta = check(
            r#"module scene
fn alpha(x: f32) -> f32:
    return x + 1.0
fn beta(x: f32) -> f32:
    return x * 2.0
fn combined(x: f32) -> f32:
    return beta(x) + alpha(x)
"#,
        );
        let beta_then_alpha = check(
            r#"module scene
fn beta(x: f32) -> f32:
    return x * 2.0
fn alpha(x: f32) -> f32:
    return x + 1.0
fn combined(x: f32) -> f32:
    return beta(x) + alpha(x)
"#,
        );

        fn compile(program: TypedProgram) -> SymbolicGraph {
            let programs = BTreeMap::from([("scene".to_string(), program)]);
            let config = test_config();
            let mut compiler = test_compiler(&programs, &config);
            let combined = called_function(&programs, "scene", "combined").unwrap();
            let coordinate = compiler
                .scalar_node(
                    ScalarOp::CoordX,
                    Dependency::Coordinate,
                    "scene",
                    Span::default(),
                )
                .unwrap();
            let value = compiler
                .eval_function(
                    combined,
                    vec![Some(SymValue::F32(coordinate))],
                    "scene",
                    Span::default(),
                )
                .unwrap();
            let radius = expect_f32(value).unwrap();
            let zero = compiler.const_f32(0.0, "scene", Span::default()).unwrap();
            let one = compiler.const_f32(1.0, "scene", Span::default()).unwrap();
            let field_root = compiler
                .field_node(
                    FieldKind::Primitive(Primitive::Sphere {
                        center: [zero; 3],
                        radius,
                    }),
                    radius,
                    "scene",
                    Span::default(),
                )
                .unwrap();
            let material_root = compiler
                .material_node(
                    MaterialKind::Sample(MaterialSampleNode {
                        base_color: [zero; 3],
                        opacity: one,
                        emissive: [zero; 3],
                        roughness: zero,
                        metallic: zero,
                        specular_level: zero,
                        ior: one,
                        normal: NormalModel::Geometric,
                        pattern: None,
                    }),
                    "scene",
                    Span::default(),
                )
                .unwrap();
            let mut graph = SymbolicGraph {
                renderer_index: 0,
                field_key: "scene::world".to_string(),
                material_key: "scene::shade".to_string(),
                params_type: Type::Unit,
                material_type: Type::Unit,
                params: compiler.params,
                scalar: compiler.scalar,
                fields: compiler.fields,
                materials: compiler.materials,
                field_root,
                material_root,
                obligations: compiler.obligations,
                quota: compiler.quota,
            };
            crate::pixels::canonicalize::run(&mut graph).unwrap();
            graph
        }

        let first = compile(alpha_then_beta);
        let reordered = compile(beta_then_alpha);
        let scalar_ops = |graph: &SymbolicGraph| {
            graph
                .scalar
                .iter()
                .map(|(_, node)| node.op.clone())
                .collect::<Vec<_>>()
        };
        let field_nodes = |graph: &SymbolicGraph| {
            graph
                .fields
                .iter()
                .map(|(_, node)| node.clone())
                .collect::<Vec<_>>()
        };
        let material_nodes = |graph: &SymbolicGraph| {
            graph
                .materials
                .iter()
                .map(|(_, node)| node.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(scalar_ops(&first), scalar_ops(&reordered));
        assert_eq!(field_nodes(&first), field_nodes(&reordered));
        assert_eq!(material_nodes(&first), material_nodes(&reordered));
        assert!(
            first
                .scalar
                .iter()
                .any(|(_, node)| matches!(node.op, ScalarOp::Add(_, _))),
            "the compared graph must contain the compiled helper calls"
        );
    }

    #[test]
    fn structural_canonicalization_is_allocation_order_independent() {
        fn push_scalar(compiler: &mut Compiler<'_>, value: f32, label: &str) -> ScalarId {
            compiler
                .scalar
                .push(
                    ScalarNode {
                        op: ScalarOp::ConstF32(value.to_bits()),
                        dependency: Dependency::Constant,
                    },
                    NodeOrigin::synthetic(label),
                )
                .unwrap()
        }
        fn push_sphere(compiler: &mut Compiler<'_>, zero: ScalarId, one: ScalarId) -> FieldId {
            compiler
                .fields
                .push(
                    FieldNode {
                        kind: FieldKind::Primitive(Primitive::Sphere {
                            center: [zero; 3],
                            radius: one,
                        }),
                        scalar_value: zero,
                    },
                    NodeOrigin::synthetic("sphere"),
                )
                .unwrap()
        }
        fn push_plane(compiler: &mut Compiler<'_>, zero: ScalarId, one: ScalarId) -> FieldId {
            compiler
                .fields
                .push(
                    FieldNode {
                        kind: FieldKind::Primitive(Primitive::Plane {
                            normal: [zero, one, zero],
                            offset: zero,
                        }),
                        scalar_value: zero,
                    },
                    NodeOrigin::synthetic("plane"),
                )
                .unwrap()
        }
        fn push_material(compiler: &mut Compiler<'_>, zero: ScalarId, one: ScalarId) -> MaterialId {
            compiler
                .materials
                .push(
                    MaterialNode {
                        kind: MaterialKind::Sample(MaterialSampleNode {
                            base_color: [zero; 3],
                            opacity: one,
                            emissive: [zero; 3],
                            roughness: zero,
                            metallic: zero,
                            specular_level: zero,
                            ior: one,
                            normal: NormalModel::Geometric,
                            pattern: None,
                        }),
                    },
                    NodeOrigin::synthetic("material"),
                )
                .unwrap()
        }
        fn finish(
            compiler: Compiler<'_>,
            field_root: FieldId,
            material_root: MaterialId,
        ) -> SymbolicGraph {
            SymbolicGraph {
                renderer_index: 0,
                field_key: "scene::world".to_string(),
                material_key: "scene::shade".to_string(),
                params_type: Type::Unit,
                material_type: Type::Unit,
                params: compiler.params,
                scalar: compiler.scalar,
                fields: compiler.fields,
                materials: compiler.materials,
                field_root,
                material_root,
                obligations: compiler.obligations,
                quota: compiler.quota,
            }
        }

        let programs = BTreeMap::new();
        let config = test_config();
        let mut first = test_compiler(&programs, &config);
        let first_zero = push_scalar(&mut first, 0.0, "zero");
        let first_one = push_scalar(&mut first, 1.0, "one");
        let first_sphere = push_sphere(&mut first, first_zero, first_one);
        let first_plane = push_plane(&mut first, first_zero, first_one);
        let first_root = first
            .field_binary(
                FieldIntrinsic::Union,
                first_sphere,
                first_plane,
                "scene",
                Span::default(),
            )
            .unwrap();
        let first_material = push_material(&mut first, first_zero, first_one);

        let mut reordered = test_compiler(&programs, &config);
        let reordered_one = push_scalar(&mut reordered, 1.0, "one");
        let reordered_zero = push_scalar(&mut reordered, 0.0, "zero");
        let reordered_plane = push_plane(&mut reordered, reordered_zero, reordered_one);
        let reordered_sphere = push_sphere(&mut reordered, reordered_zero, reordered_one);
        let reordered_root = reordered
            .field_binary(
                FieldIntrinsic::Union,
                reordered_sphere,
                reordered_plane,
                "scene",
                Span::default(),
            )
            .unwrap();
        let reordered_material = push_material(&mut reordered, reordered_zero, reordered_one);

        assert_eq!(
            first
                .compare_field_structural(first_sphere, first_plane)
                .unwrap(),
            reordered
                .compare_field_structural(reordered_sphere, reordered_plane)
                .unwrap()
        );

        let mut first_graph = finish(first, first_root, first_material);
        let mut reordered_graph = finish(reordered, reordered_root, reordered_material);
        crate::pixels::canonicalize::run(&mut first_graph).unwrap();
        crate::pixels::canonicalize::run(&mut reordered_graph).unwrap();
        assert_eq!(first_graph, reordered_graph);

        let configs = crate::pixels::config::RendererConfigs {
            renderers: vec![config],
        };
        assert_eq!(
            crate::pixels::dump_symbolic_graphs(&[(0, first_graph)], &configs),
            crate::pixels::dump_symbolic_graphs(&[(0, reordered_graph)], &configs),
        );
    }
}
