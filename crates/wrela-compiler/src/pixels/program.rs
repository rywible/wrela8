//! Canonical coefficient and predicate programs used by projective lowering.
//!
//! These records are compiler data. They deliberately describe coefficient
//! dependencies instead of forming another executable Wrela IR.

use std::collections::BTreeMap;

use super::ids::{CoeffId, ParamId, PolyProgramId, PredicateProgramId, ScalarId};
use super::polynomial::PolyProgram;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CameraCoeff {
    Eye(u8),
    Forward(u8),
    Right(u8),
    Up(u8),
    EyeRate(u8),
    ForwardRate(u8),
    RightRate(u8),
    UpRate(u8),
    TanHalfFovY,
    Aspect,
}

impl CameraCoeff {
    pub fn temporal_rate(self) -> Option<Self> {
        match self {
            Self::Eye(component) => Some(Self::EyeRate(component)),
            Self::Forward(component) => Some(Self::ForwardRate(component)),
            Self::Right(component) => Some(Self::RightRate(component)),
            Self::Up(component) => Some(Self::UpRate(component)),
            Self::EyeRate(_)
            | Self::ForwardRate(_)
            | Self::RightRate(_)
            | Self::UpRate(_)
            | Self::TanHalfFovY
            | Self::Aspect => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CoeffOp {
    ConstF64(u64),
    Scalar(ScalarId),
    Camera(CameraCoeff),
    ScalarParamDerivative(ScalarId, ParamId),
    ParamRate(ParamId, u8),
    Add(CoeffId, CoeffId),
    Mul(CoeffId, CoeffId),
    Neg(CoeffId),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CoeffSemanticKey {
    ConstF64(u64),
    Scalar(ScalarId),
    Camera(CameraCoeff),
    ScalarParamDerivative(ScalarId, ParamId),
    ParamRate(ParamId, u8),
    Add(Box<CoeffSemanticKey>, Box<CoeffSemanticKey>),
    Mul(Box<CoeffSemanticKey>, Box<CoeffSemanticKey>),
    Neg(Box<CoeffSemanticKey>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoeffNode {
    pub id: CoeffId,
    pub op: CoeffOp,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CoeffProgram {
    pub nodes: Vec<CoeffNode>,
}

impl CoeffProgram {
    pub fn get(&self, id: CoeffId) -> Result<&CoeffNode, String> {
        self.nodes.get(id.index()).ok_or_else(|| {
            format!(
                "pixels::program: coefficient {id} is outside {} nodes",
                self.nodes.len()
            )
        })
    }

    pub fn is_exact_zero(&self, id: CoeffId) -> bool {
        matches!(
            self.nodes.get(id.index()).map(|node| &node.op),
            Some(CoeffOp::ConstF64(bits)) if f64::from_bits(*bits) == 0.0
        )
    }

    pub fn is_exact_one(&self, id: CoeffId) -> bool {
        matches!(
            self.nodes.get(id.index()).map(|node| &node.op),
            Some(CoeffOp::ConstF64(bits)) if f64::from_bits(*bits) == 1.0
        )
    }

    pub fn exact_constant(&self, id: CoeffId) -> Option<f64> {
        match &self.nodes.get(id.index())?.op {
            CoeffOp::ConstF64(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }

    fn semantic_key(&self, id: CoeffId) -> Result<CoeffSemanticKey, String> {
        let key = match self.get(id)?.op {
            CoeffOp::ConstF64(bits) => CoeffSemanticKey::ConstF64(bits),
            CoeffOp::Scalar(value) => CoeffSemanticKey::Scalar(value),
            CoeffOp::Camera(value) => CoeffSemanticKey::Camera(value),
            CoeffOp::ScalarParamDerivative(scalar, param) => {
                CoeffSemanticKey::ScalarParamDerivative(scalar, param)
            }
            CoeffOp::ParamRate(param, order) => CoeffSemanticKey::ParamRate(param, order),
            CoeffOp::Add(a, b) => {
                let (a, b) = (self.semantic_key(a)?, self.semantic_key(b)?);
                let (a, b) = if a <= b { (a, b) } else { (b, a) };
                CoeffSemanticKey::Add(Box::new(a), Box::new(b))
            }
            CoeffOp::Mul(a, b) => {
                let (a, b) = (self.semantic_key(a)?, self.semantic_key(b)?);
                let (a, b) = if a <= b { (a, b) } else { (b, a) };
                CoeffSemanticKey::Mul(Box::new(a), Box::new(b))
            }
            CoeffOp::Neg(value) => CoeffSemanticKey::Neg(Box::new(self.semantic_key(value)?)),
        };
        Ok(key)
    }

    pub fn influencing_params(
        &self,
        roots: impl IntoIterator<Item = CoeffId>,
        scalar_params: &BTreeMap<ScalarId, Vec<ParamId>>,
    ) -> Result<Vec<ParamId>, String> {
        let mut params = std::collections::BTreeSet::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut stack = roots.into_iter().collect::<Vec<_>>();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match self.get(id)?.op {
                CoeffOp::ConstF64(_) | CoeffOp::Camera(_) | CoeffOp::ParamRate(_, _) => {}
                CoeffOp::Scalar(scalar) | CoeffOp::ScalarParamDerivative(scalar, _) => {
                    if let Some(found) = scalar_params.get(&scalar) {
                        params.extend(found);
                    }
                }
                CoeffOp::Add(a, b) | CoeffOp::Mul(a, b) => stack.extend([a, b]),
                CoeffOp::Neg(value) => stack.push(value),
            }
        }
        Ok(params.into_iter().collect())
    }

    pub fn evaluate(
        &self,
        scalar: &impl Fn(ScalarId) -> Result<f64, String>,
        camera: &impl Fn(CameraCoeff) -> Result<f64, String>,
    ) -> Result<Vec<f64>, String> {
        self.evaluate_with_derivatives(
            scalar,
            camera,
            &|scalar, param| {
                Err(format!(
                    "pixels::program: no evaluator supplied for derivative of {scalar} by {param}"
                ))
            },
            &|param, order| {
                Err(format!(
                    "pixels::program: no evaluator supplied for rate order {order} of {param}"
                ))
            },
        )
    }

    pub fn evaluate_with_derivatives(
        &self,
        scalar: &impl Fn(ScalarId) -> Result<f64, String>,
        camera: &impl Fn(CameraCoeff) -> Result<f64, String>,
        scalar_derivative: &impl Fn(ScalarId, ParamId) -> Result<f64, String>,
        param_rate: &impl Fn(ParamId, u8) -> Result<f64, String>,
    ) -> Result<Vec<f64>, String> {
        let mut values = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let get = |id: CoeffId, values: &[f64]| {
                values.get(id.index()).copied().ok_or_else(|| {
                    format!("pixels::program: coefficient {id} names a non-predecessor")
                })
            };
            let value = match node.op {
                CoeffOp::ConstF64(bits) => f64::from_bits(bits),
                CoeffOp::Scalar(id) => scalar(id)?,
                CoeffOp::Camera(id) => camera(id)?,
                CoeffOp::ScalarParamDerivative(scalar, param) => scalar_derivative(scalar, param)?,
                CoeffOp::ParamRate(param, order) => param_rate(param, order)?,
                CoeffOp::Add(a, b) => get(a, &values)? + get(b, &values)?,
                CoeffOp::Mul(a, b) => get(a, &values)? * get(b, &values)?,
                CoeffOp::Neg(value) => -get(value, &values)?,
            };
            if !value.is_finite() {
                return Err(format!(
                    "pixels::program: coefficient {} evaluated non-finite",
                    node.id
                ));
            }
            values.push(value);
        }
        Ok(values)
    }

    /// Rebuild the reachable coefficient DAG in semantic topological order.
    ///
    /// Append order is an implementation detail of lowering and must not
    /// affect serialized IDs. The returned map names each old node's new ID;
    /// unreachable nodes map to `None`.
    pub fn canonicalize_reachable(
        &self,
        roots: impl IntoIterator<Item = CoeffId>,
    ) -> Result<(Self, Vec<Option<CoeffId>>), String> {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
        enum CanonicalOp {
            ConstF64(u64),
            Scalar(ScalarId),
            Camera(CameraCoeff),
            ScalarParamDerivative(ScalarId, ParamId),
            ParamRate(ParamId, u8),
            Add(CoeffId, CoeffId),
            Mul(CoeffId, CoeffId),
            Neg(CoeffId),
        }

        fn children(op: &CoeffOp) -> impl Iterator<Item = CoeffId> {
            let mut values = [None, None];
            match *op {
                CoeffOp::Add(a, b) | CoeffOp::Mul(a, b) => {
                    values = [Some(a), Some(b)];
                }
                CoeffOp::Neg(value) => values[0] = Some(value),
                CoeffOp::ConstF64(_)
                | CoeffOp::Scalar(_)
                | CoeffOp::Camera(_)
                | CoeffOp::ScalarParamDerivative(_, _)
                | CoeffOp::ParamRate(_, _) => {}
            }
            values.into_iter().flatten()
        }

        let mut reachable = vec![false; self.nodes.len()];
        let mut stack = roots.into_iter().collect::<Vec<_>>();
        while let Some(id) = stack.pop() {
            let node = self.get(id)?;
            if reachable[id.index()] {
                continue;
            }
            reachable[id.index()] = true;
            stack.extend(children(&node.op));
        }

        let mut depths = vec![0_usize; self.nodes.len()];
        let mut maximum_depth = 0_usize;
        for node in &self.nodes {
            if node.id.index() >= self.nodes.len() {
                return Err(format!(
                    "pixels::program: coefficient node {} is outside canonical storage",
                    node.id
                ));
            }
            let mut depth = 0_usize;
            for child in children(&node.op) {
                if child.index() >= node.id.index() {
                    return Err(format!(
                        "pixels::program: coefficient {} names non-predecessor {child}",
                        node.id
                    ));
                }
                depth = depth.max(
                    depths[child.index()]
                        .checked_add(1)
                        .ok_or_else(|| "P015: coefficient DAG depth overflow".to_string())?,
                );
            }
            depths[node.id.index()] = depth;
            maximum_depth = maximum_depth.max(depth);
        }

        let mut remap = vec![None::<CoeffId>; self.nodes.len()];
        let mut nodes = Vec::<CoeffNode>::new();
        let mut canonical = BTreeMap::<CanonicalOp, CoeffId>::new();
        for depth in 0..=maximum_depth {
            let mut level = Vec::<(CanonicalOp, usize)>::new();
            for (old_index, node) in self.nodes.iter().enumerate() {
                if !reachable[old_index] || depths[old_index] != depth {
                    continue;
                }
                let mapped = |id: CoeffId| {
                    remap[id.index()].ok_or_else(|| {
                        format!(
                            "pixels::program: coefficient {} was not mapped before dependent {}",
                            id, node.id
                        )
                    })
                };
                let key = match node.op {
                    CoeffOp::ConstF64(bits) => CanonicalOp::ConstF64(bits),
                    CoeffOp::Scalar(value) => CanonicalOp::Scalar(value),
                    CoeffOp::Camera(value) => CanonicalOp::Camera(value),
                    CoeffOp::ScalarParamDerivative(scalar, param) => {
                        CanonicalOp::ScalarParamDerivative(scalar, param)
                    }
                    CoeffOp::ParamRate(param, order) => CanonicalOp::ParamRate(param, order),
                    CoeffOp::Add(a, b) => {
                        let (a, b) = (mapped(a)?, mapped(b)?);
                        CanonicalOp::Add(a.min(b), a.max(b))
                    }
                    CoeffOp::Mul(a, b) => {
                        let (a, b) = (mapped(a)?, mapped(b)?);
                        CanonicalOp::Mul(a.min(b), a.max(b))
                    }
                    CoeffOp::Neg(value) => CanonicalOp::Neg(mapped(value)?),
                };
                level.push((key, old_index));
            }
            level.sort();
            for (key, old_index) in level {
                let id = if let Some(existing) = canonical.get(&key) {
                    *existing
                } else {
                    let id = CoeffId(u32::try_from(nodes.len()).map_err(
                        |_| "P015: renderer capacity `coefficient_programs` overflows u32",
                    )?);
                    let op = match key.clone() {
                        CanonicalOp::ConstF64(bits) => CoeffOp::ConstF64(bits),
                        CanonicalOp::Scalar(value) => CoeffOp::Scalar(value),
                        CanonicalOp::Camera(value) => CoeffOp::Camera(value),
                        CanonicalOp::ScalarParamDerivative(scalar, param) => {
                            CoeffOp::ScalarParamDerivative(scalar, param)
                        }
                        CanonicalOp::ParamRate(param, order) => CoeffOp::ParamRate(param, order),
                        CanonicalOp::Add(a, b) => CoeffOp::Add(a, b),
                        CanonicalOp::Mul(a, b) => CoeffOp::Mul(a, b),
                        CanonicalOp::Neg(value) => CoeffOp::Neg(value),
                    };
                    nodes.push(CoeffNode { id, op });
                    canonical.insert(key, id);
                    id
                };
                remap[old_index] = Some(id);
            }
        }
        Ok((Self { nodes }, remap))
    }
}

/// Append-only CSE builder. Commutative operands are sorted before insertion.
/// Additions are flattened and rebuilt in semantic-key order so compiler
/// construction grouping cannot change f64 evaluation order.
#[derive(Clone, Debug, Default)]
pub struct CoeffBuilder {
    program: CoeffProgram,
    canonical: BTreeMap<CoeffOp, CoeffId>,
}

impl CoeffBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_program(program: CoeffProgram) -> Result<Self, String> {
        let mut canonical = BTreeMap::new();
        for (index, node) in program.nodes.iter().enumerate() {
            if node.id.index() != index {
                return Err(format!(
                    "pixels::program: coefficient node {} is out of canonical position {index}",
                    node.id
                ));
            }
            if canonical.insert(node.op.clone(), node.id).is_some() {
                return Err(format!(
                    "pixels::program: duplicate canonical coefficient operation at {}",
                    node.id
                ));
            }
        }
        Ok(Self { program, canonical })
    }

    fn intern(&mut self, op: CoeffOp) -> Result<CoeffId, String> {
        if let Some(id) = self.canonical.get(&op) {
            return Ok(*id);
        }
        let id = CoeffId(
            u32::try_from(self.program.nodes.len())
                .map_err(|_| "P015: renderer capacity `coefficient_programs` overflows u32")?,
        );
        self.program.nodes.push(CoeffNode { id, op: op.clone() });
        self.canonical.insert(op, id);
        Ok(id)
    }

    pub fn constant(&mut self, value: f64) -> Result<CoeffId, String> {
        if !value.is_finite() {
            return Err("P004: field operation `projective coefficient` is not available in `AaaByteExact`: non-finite coefficient".to_string());
        }
        let canonical = if value == 0.0 { 0.0 } else { value };
        self.intern(CoeffOp::ConstF64(canonical.to_bits()))
    }

    pub fn scalar(&mut self, value: ScalarId) -> Result<CoeffId, String> {
        self.intern(CoeffOp::Scalar(value))
    }

    pub fn camera(&mut self, value: CameraCoeff) -> Result<CoeffId, String> {
        self.intern(CoeffOp::Camera(value))
    }

    pub fn scalar_param_derivative(
        &mut self,
        scalar: ScalarId,
        param: ParamId,
    ) -> Result<CoeffId, String> {
        self.intern(CoeffOp::ScalarParamDerivative(scalar, param))
    }

    pub fn param_rate(&mut self, param: ParamId, order: u8) -> Result<CoeffId, String> {
        if !(1..=2).contains(&order) {
            return Err(format!(
                "pixels::program: unsupported parameter rate order {order}"
            ));
        }
        self.intern(CoeffOp::ParamRate(param, order))
    }

    pub fn neg(&mut self, value: CoeffId) -> Result<CoeffId, String> {
        if let Some(constant) = self.program.exact_constant(value) {
            return self.constant(-constant);
        }
        if let CoeffOp::Neg(inner) = self.program.get(value)?.op {
            return Ok(inner);
        }
        self.intern(CoeffOp::Neg(value))
    }

    pub fn add(&mut self, a: CoeffId, b: CoeffId) -> Result<CoeffId, String> {
        let mut pending = vec![a, b];
        let mut leaves = Vec::new();
        while let Some(id) = pending.pop() {
            match self.program.get(id)?.op {
                CoeffOp::Add(left, right) => pending.extend([left, right]),
                _ if self.program.is_exact_zero(id) => {}
                _ => leaves.push((self.program.semantic_key(id)?, id)),
            }
        }
        leaves.sort_by(|left, right| left.0.cmp(&right.0));
        let mut leaves = leaves.into_iter().map(|(_, id)| id);
        let Some(mut result) = leaves.next() else {
            return self.constant(0.0);
        };
        for value in leaves {
            let (a, b) = if result <= value {
                (result, value)
            } else {
                (value, result)
            };
            result = self.intern(CoeffOp::Add(a, b))?;
        }
        Ok(result)
    }

    pub fn sub(&mut self, a: CoeffId, b: CoeffId) -> Result<CoeffId, String> {
        let negative = self.neg(b)?;
        self.add(a, negative)
    }

    pub fn mul(&mut self, a: CoeffId, b: CoeffId) -> Result<CoeffId, String> {
        if self.program.is_exact_zero(a) || self.program.is_exact_zero(b) {
            return self.constant(0.0);
        }
        if self.program.is_exact_one(a) {
            return Ok(b);
        }
        if self.program.is_exact_one(b) {
            return Ok(a);
        }
        if let (Some(a), Some(b)) = (
            self.program.exact_constant(a),
            self.program.exact_constant(b),
        ) {
            return self.constant(a * b);
        }
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        self.intern(CoeffOp::Mul(a, b))
    }

    pub fn scale(&mut self, value: CoeffId, scale: f64) -> Result<CoeffId, String> {
        let scale = self.constant(scale)?;
        self.mul(value, scale)
    }

    pub fn finish(self) -> CoeffProgram {
        self.program
    }

    pub fn program(&self) -> &CoeffProgram {
        &self.program
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PredicateSense {
    StrictNegative,
    NonPositive,
    EqualZero,
    NonNegative,
    StrictPositive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PredicateProgram {
    pub id: PredicateProgramId,
    pub polynomial: PolyProgramId,
    pub sense: PredicateSense,
    pub boundary_family: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ProgramTables {
    pub coefficients: CoeffProgram,
    pub polynomials: Vec<PolyProgram>,
    pub predicates: Vec<PredicateProgram>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficient_cse_is_commutative_and_bit_exact() {
        let mut builder = CoeffBuilder::new();
        let scalar = builder.scalar(ScalarId(4)).unwrap();
        let two = builder.constant(2.0).unwrap();
        let a = builder.add(scalar, two).unwrap();
        let b = builder.add(two, scalar).unwrap();
        assert_eq!(a, b);
        let quarter = builder.constant(0.25).unwrap();
        let half = builder.constant(0.5).unwrap();
        let folded = builder.add(quarter, half).unwrap();
        assert_eq!(
            builder
                .program()
                .evaluate(&|_| Ok(0.0), &|_| Ok(0.0),)
                .unwrap()[folded.index()],
            0.75
        );
    }

    #[test]
    fn coefficient_zero_is_removed_only_when_exact() {
        let mut builder = CoeffBuilder::new();
        let scalar = builder.scalar(ScalarId(0)).unwrap();
        let zero = builder.constant(-0.0).unwrap();
        assert_eq!(builder.add(scalar, zero).unwrap(), scalar);
        let camera = builder.camera(CameraCoeff::Eye(0)).unwrap();
        assert_ne!(builder.sub(camera, camera).unwrap(), zero);
    }

    #[test]
    fn reachable_coefficients_are_canonical_across_construction_orders() {
        fn build(reverse: bool) -> (CoeffProgram, CoeffId) {
            let mut builder = CoeffBuilder::new();
            let (scalar, camera) = if reverse {
                let camera = builder.camera(CameraCoeff::Eye(2)).unwrap();
                builder.constant(91.0).unwrap(); // deliberately unreachable
                let scalar = builder.scalar(ScalarId(7)).unwrap();
                (scalar, camera)
            } else {
                let scalar = builder.scalar(ScalarId(7)).unwrap();
                builder.constant(37.0).unwrap(); // deliberately unreachable
                let camera = builder.camera(CameraCoeff::Eye(2)).unwrap();
                (scalar, camera)
            };
            let sum = builder.add(scalar, camera).unwrap();
            let product = builder.mul(sum, scalar).unwrap();
            (builder.finish(), product)
        }

        let (left, left_root) = build(false);
        let (right, right_root) = build(true);
        let (left, left_map) = left.canonicalize_reachable([left_root]).unwrap();
        let (right, right_map) = right.canonicalize_reachable([right_root]).unwrap();
        assert_eq!(left, right);
        assert_eq!(left.nodes.len(), 4);
        assert_eq!(left_map[left_root.index()], right_map[right_root.index()]);
        assert_eq!(
            left.nodes
                .iter()
                .filter(|node| matches!(node.op, CoeffOp::ConstF64(_)))
                .count(),
            0
        );
    }
}
