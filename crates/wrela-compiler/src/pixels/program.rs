//! Canonical coefficient and predicate programs used by projective lowering.
//!
//! These records are compiler data. They deliberately describe coefficient
//! dependencies instead of forming another executable Wrela IR.

use std::collections::BTreeMap;

use super::ids::{CoeffId, ParamId, PolyProgramId, PredicateProgramId, ScalarId};
use super::polynomial::PolyProgram;

fn secondary_lower_f32_bits(value: f64) -> u64 {
    let rounded = value as f32;
    let outward = if f64::from(rounded) > value {
        super::reference::interval::next_down_f32(rounded)
    } else {
        rounded
    };
    f64::from(outward).to_bits()
}

fn secondary_upper_f32_bits(value: f64) -> u64 {
    let rounded = value as f32;
    let outward = if f64::from(rounded) < value {
        super::reference::interval::next_up_f32(rounded)
    } else {
        rounded
    };
    f64::from(outward).to_bits()
}

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
    fn serialized_secondary_bounds_round_outward_to_guest_f32() {
        for value in [
            -1.0e20,
            -1.000_000_059_604_644_8,
            -f64::from(f32::MIN_POSITIVE),
            0.0,
            f64::from(f32::MIN_POSITIVE),
            1.000_000_059_604_644_8,
            1.0e20,
        ] {
            let lower = f64::from_bits(secondary_lower_f32_bits(value));
            let upper = f64::from_bits(secondary_upper_f32_bits(value));
            assert!(lower <= value, "lower={lower:?} value={value:?}");
            assert!(upper >= value, "upper={upper:?} value={value:?}");
            assert_eq!(f64::from(lower as f32), lower);
            assert_eq!(f64::from(upper as f32), upper);
        }
    }

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

    #[test]
    fn machine_verifier_rejects_rehashed_numeric_policy_mutations() {
        let base = minimal_verified_frame_program().program().clone();

        let mut changed_lut = base.clone();
        let camera = changed_lut
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::CameraLightPost)
            .unwrap();
        camera.records[0].operands[25] += 1;
        let error = super::super::verify::check_program(changed_lut)
            .expect_err("monotone but noncanonical LUT");
        assert!(error.contains("sealed LUT reference"), "{error}");

        let mut changed_influence = base.clone();
        let summaries = changed_influence
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::ShadingSummary)
            .unwrap();
        let light = summaries
            .records
            .iter_mut()
            .find(|record| record.tag == 6)
            .unwrap();
        light.operands[12] = u64::from(1.0_f32.to_bits());
        light.operands[15] = u64::from(1.0_f32.to_bits());
        let error = super::super::verify::check_program(changed_influence)
            .expect_err("light influence outside the sealed world");
        assert!(error.contains("influence bounds differ"), "{error}");

        let mut changed_incident = base.clone();
        let summaries = changed_incident
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::ShadingSummary)
            .unwrap();
        summaries
            .records
            .iter_mut()
            .find(|record| record.tag == 6)
            .unwrap()
            .operands[18] = u64::from(1.0_f32.to_bits());
        let error = super::super::verify::check_program(changed_incident)
            .expect_err("noncanonical maximum incident radiance");
        assert!(error.contains("maximum incident-radiance"), "{error}");

        let mut weakened_radius = base.clone();
        let fixed = weakened_radius
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::FixedDomain)
            .unwrap();
        let policy = fixed
            .records
            .iter_mut()
            .find(|record| record.tag == 5)
            .unwrap();
        policy.operands[3] = policy.operands[3].saturating_sub(1);
        let error = super::super::verify::check_program(weakened_radius)
            .expect_err("weakened fixed-q radius");
        assert!(
            error.contains("does not match sealed camera policy"),
            "{error}"
        );

        let mut missing_policy = base;
        let fixed = missing_policy
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::FixedDomain)
            .unwrap();
        fixed.records.retain(|record| record.tag != 5);
        for (stable_id, record) in fixed.records.iter_mut().enumerate() {
            record.stable_id = stable_id as u32;
        }
        let error = super::super::verify::check_program(missing_policy)
            .expect_err("missing fixed-q policy");
        assert!(error.contains("expected exactly one"), "{error}");
    }

    fn program_with_local_index_ids(ids: usize) -> FrameProgram {
        let mut program = minimal_verified_frame_program().program().clone();
        let fixed = program
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::FixedDomain)
            .unwrap();
        fixed.records.retain(|record| record.tag != 30);
        let chunk_payload = wrela_machine::pixels::FRAME_PROGRAM_MAX_OPERANDS_V1 - 5;
        for index_kind in 0..6_u64 {
            let ids = if index_kind == 0 { ids } else { 0 };
            let mut payload = if ids == 0 {
                Vec::new()
            } else {
                vec![0, ids as u64]
            };
            payload.extend((0..ids).map(|id| id as u64));
            let chunks = payload.len().div_ceil(chunk_payload);
            fixed.records.push(FrameRecord {
                stable_id: fixed.records.len() as u32,
                tag: 30,
                flags: 0,
                operands: vec![
                    0,
                    index_kind,
                    u64::from(ids != 0),
                    ids as u64,
                    chunks as u64,
                ],
            });
            for (chunk_index, chunk) in payload.chunks(chunk_payload).enumerate() {
                let offset = chunk_index * chunk_payload;
                let mut operands = vec![
                    1,
                    index_kind,
                    chunk_index as u64,
                    offset as u64,
                    chunk.len() as u64,
                ];
                operands.extend_from_slice(chunk);
                fixed.records.push(FrameRecord {
                    stable_id: fixed.records.len() as u32,
                    tag: 30,
                    flags: 0,
                    operands,
                });
            }
        }
        program
    }

    #[test]
    fn local_index_wire_chunks_at_operand_boundary() {
        let chunk_payload = wrela_machine::pixels::FRAME_PROGRAM_MAX_OPERANDS_V1 - 5;
        for ids in [chunk_payload - 2, chunk_payload - 1] {
            let program = program_with_local_index_ids(ids);
            super::super::verify::check_program(program).unwrap();
        }
    }

    #[test]
    fn local_index_wire_rejects_reordered_ids() {
        let mut program = program_with_local_index_ids(2);
        let fixed = program
            .tables
            .iter_mut()
            .find(|table| table.kind == FrameProgramTableKindV1::FixedDomain)
            .unwrap();
        let chunk = fixed
            .records
            .iter_mut()
            .find(|record| record.tag == 30 && record.operands[..2] == [1, 0])
            .unwrap();
        chunk.operands[7] = chunk.operands[6];
        let error = super::super::verify::check_program(program).unwrap_err();
        assert!(error.contains("IDs are not strictly ordered"), "{error}");
    }
}

// FrameProgram v1 rich model. The projective coefficient program above stays
// compiler-rich; this section is the verified, pointer-free model consumed by
// the explicit encoder.

use wrela_machine::pixels::FrameProgramTableKindV1;
pub use wrela_machine::pixels::{
    FrameProgramModelV1 as FrameProgram, FrameRecordV1 as FrameRecord,
    FrameTableModelV1 as FrameTable,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedFrameProgram(FrameProgram);

impl VerifiedFrameProgram {
    pub fn program(&self) -> &FrameProgram {
        &self.0
    }

    pub(crate) fn new(program: FrameProgram) -> Self {
        Self(program)
    }
}

fn push_record(table: &mut Vec<FrameRecord>, tag: u16, flags: u16, operands: Vec<u64>) {
    table.push(FrameRecord {
        stable_id: u32::try_from(table.len()).expect("P4 ceilings fit FrameProgram u32 IDs"),
        tag,
        flags,
        operands,
    });
}

fn id(value: super::ids::ScalarId) -> u64 {
    u64::from(value.0)
}

fn dependency_tag(value: super::scalar::Dependency) -> u64 {
    use super::scalar::Dependency;
    match value {
        Dependency::Constant => 0,
        Dependency::Coordinate => 1,
        Dependency::Parameter => 2,
        Dependency::Surface => 4,
        Dependency::CoordinateAndParameter => 3,
        Dependency::CoordinateAndSurface => 5,
        Dependency::ParameterAndSurface => 6,
        Dependency::CoordinateParameterAndSurface => 7,
    }
}

fn semantic_tag(value: super::scalar::SemanticOpId) -> u64 {
    use super::scalar::SemanticOpId;
    match value {
        SemanticOpId::SqrtF32V1 => 1,
        SemanticOpId::RsqrtF32V1 => 2,
        SemanticOpId::SinRestrictedF32V1 => 3,
        SemanticOpId::CosRestrictedF32V1 => 4,
        SemanticOpId::Normalize3F32V1 => 5,
        SemanticOpId::SmoothMinF32V1 => 6,
        SemanticOpId::FiniteColorF32V1 => 7,
        SemanticOpId::MaterialRoughnessF32V1 => 8,
    }
}

fn compare_tag(value: super::scalar::CompareOp) -> u64 {
    use super::scalar::CompareOp;
    match value {
        CompareOp::Lt => 1,
        CompareOp::Le => 2,
        CompareOp::Gt => 3,
        CompareOp::Ge => 4,
        CompareOp::Eq => 5,
        CompareOp::Ne => 6,
    }
}

fn axis_tag(value: super::graph::Axis) -> u64 {
    match value {
        super::graph::Axis::X => 1,
        super::graph::Axis::Y => 2,
        super::graph::Axis::Z => 3,
    }
}

fn scalar_type_tag(value: super::params::ScalarType) -> u64 {
    use super::params::ScalarType;
    match value {
        ScalarType::F32 => 1,
        ScalarType::F64 => 2,
        ScalarType::U8 => 3,
        ScalarType::U16 => 4,
        ScalarType::U32 => 5,
        ScalarType::U64 => 6,
        ScalarType::I8 => 7,
        ScalarType::I16 => 8,
        ScalarType::I32 => 9,
        ScalarType::I64 => 10,
        ScalarType::Usize => 11,
        ScalarType::Isize => 12,
    }
}

fn param_use_bit(value: super::params::ParamUse) -> u8 {
    use super::params::ParamUse;
    match value {
        ParamUse::Geometry => 0,
        ParamUse::Material => 1,
        ParamUse::Camera => 2,
        ParamUse::Light => 3,
        ParamUse::Exposure => 4,
        ParamUse::Post => 5,
        ParamUse::Probe => 6,
    }
}

fn exclusion_reason_tag(value: super::exclusions::ExclusionReason) -> u64 {
    use super::exclusions::ExclusionReason;
    match value {
        ExclusionReason::WorldBoundsDisjoint => 1,
        ExclusionReason::ProjectedBoundsDisjoint => 2,
        ExclusionReason::QRangesDisjoint => 3,
        ExclusionReason::OutsideNearFar => 4,
        ExclusionReason::CsgNonInfluential => 5,
        ExclusionReason::FeatureValidityImpossible => 6,
        ExclusionReason::SupportShellDisjoint => 7,
        ExclusionReason::MaterialClassIrrelevant => 8,
        ExclusionReason::FullDomainProjection => 9,
        ExclusionReason::StaticStrictOrder => 10,
        ExclusionReason::GlobalParameterBoxStrictSign => 11,
        ExclusionReason::GlobalSpatialBoxStrictSign => 12,
        ExclusionReason::DuplicateCanonicalFeature => 13,
    }
}

fn scalar_record(node: &super::scalar::ScalarNode) -> (u16, Vec<u64>) {
    use super::scalar::ScalarOp;
    let pair = |a: super::ids::ScalarId, b: super::ids::ScalarId| vec![id(a), id(b)];
    match &node.op {
        ScalarOp::ConstF32(bits) => (1, vec![u64::from(*bits)]),
        ScalarOp::ConstF64(bits) => (2, vec![*bits]),
        ScalarOp::CoordX => (3, vec![]),
        ScalarOp::CoordY => (4, vec![]),
        ScalarOp::CoordZ => (5, vec![]),
        ScalarOp::SurfacePosition(component) => (6, vec![u64::from(*component)]),
        ScalarOp::SurfaceNormal(component) => (7, vec![u64::from(*component)]),
        ScalarOp::Param(param) => (8, vec![u64::from(param.0)]),
        ScalarOp::Add(a, b) => (9, pair(*a, *b)),
        ScalarOp::Sub(a, b) => (10, pair(*a, *b)),
        ScalarOp::Mul(a, b) => (11, pair(*a, *b)),
        ScalarOp::Div(a, b) => (12, pair(*a, *b)),
        ScalarOp::Neg(value) => (13, vec![id(*value)]),
        ScalarOp::Abs(value) => (14, vec![id(*value)]),
        ScalarOp::Min(a, b) => (15, pair(*a, *b)),
        ScalarOp::Max(a, b) => (16, pair(*a, *b)),
        ScalarOp::Clamp { value, lo, hi } => (17, vec![id(*value), id(*lo), id(*hi)]),
        ScalarOp::Sqrt(value, semantic) => (18, vec![id(*value), semantic_tag(*semantic)]),
        ScalarOp::Rsqrt(value, semantic) => (19, vec![id(*value), semantic_tag(*semantic)]),
        ScalarOp::SinRestricted(value, semantic) => (20, vec![id(*value), semantic_tag(*semantic)]),
        ScalarOp::CosRestricted(value, semantic) => (21, vec![id(*value), semantic_tag(*semantic)]),
        ScalarOp::Dot3(a, b) => (22, a.iter().chain(b).map(|value| id(*value)).collect()),
        ScalarOp::Cross3Component { component, a, b } => {
            let mut operands = vec![u64::from(*component)];
            operands.extend(a.iter().chain(b).map(|value| id(*value)));
            (23, operands)
        }
        ScalarOp::Length2(value) => (24, value.iter().map(|value| id(*value)).collect()),
        ScalarOp::Length3(value) => (25, value.iter().map(|value| id(*value)).collect()),
        ScalarOp::Normalize3Component {
            component,
            value,
            semantic,
        } => {
            let mut operands = vec![u64::from(*component), semantic_tag(*semantic)];
            operands.extend(value.iter().map(|value| id(*value)));
            (26, operands)
        }
        ScalarOp::Compare { op, a, b } => (27, vec![compare_tag(*op), id(*a), id(*b)]),
        ScalarOp::Select { predicate, a, b } => (28, vec![id(*predicate), id(*a), id(*b)]),
        ScalarOp::SelectIndex { index, options } => {
            let mut operands = vec![id(*index), options.len() as u64];
            operands.extend(options.iter().map(|value| id(*value)));
            (29, operands)
        }
        ScalarOp::SmoothMin { a, b, k, semantic } => {
            (30, vec![id(*a), id(*b), id(*k), semantic_tag(*semantic)])
        }
        ScalarOp::FiniteOr {
            value,
            fallback,
            semantic,
        } => (31, vec![id(*value), id(*fallback), semantic_tag(*semantic)]),
        ScalarOp::MaterialRoughness { value, semantic } => {
            (32, vec![id(*value), semantic_tag(*semantic)])
        }
    }
}

fn transform_operands(transform: &super::graph::TransformProgram) -> Vec<u64> {
    use super::graph::TransformProgram;
    let mut output = Vec::new();
    match transform {
        TransformProgram::Translate { by } => {
            output.push(1);
            output.extend(by.iter().map(|value| u64::from(value.0)));
        }
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => {
            output.push(2);
            output.extend(
                row_x
                    .iter()
                    .chain(row_y)
                    .chain(row_z)
                    .map(|value| u64::from(value.0)),
            );
        }
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => {
            output.push(3);
            output.extend(
                translation
                    .iter()
                    .chain(row_x)
                    .chain(row_y)
                    .chain(row_z)
                    .map(|value| u64::from(value.0)),
            );
        }
        TransformProgram::UniformScale { scale } => {
            output.extend([4, u64::from(scale.0)]);
        }
        TransformProgram::SourceRigidSequence { steps, composed }
        | TransformProgram::RigidSequence { steps, composed } => {
            output.extend([
                if matches!(transform, TransformProgram::SourceRigidSequence { .. }) {
                    5
                } else {
                    6
                },
                steps.len() as u64,
            ]);
            for step in steps {
                let encoded = transform_operands(step);
                output.push(encoded.len() as u64);
                output.extend(encoded);
            }
            let encoded = transform_operands(composed);
            output.push(encoded.len() as u64);
            output.extend(encoded);
        }
    }
    output
}

fn field_record(node: &super::graph::FieldNode) -> (u16, Vec<u64>) {
    use super::graph::{FieldKind, Primitive};
    let scalar = u64::from(node.scalar_value.0);
    let scalars = |values: &[super::ids::ScalarId]| {
        values
            .iter()
            .map(|value| u64::from(value.0))
            .collect::<Vec<_>>()
    };
    match &node.kind {
        FieldKind::Primitive(primitive) => {
            let (tag, mut operands) = match primitive {
                Primitive::Plane { normal, offset } => {
                    let mut values = scalars(normal);
                    values.push(id(*offset));
                    (1, values)
                }
                Primitive::Sphere { center, radius } => {
                    let mut values = scalars(center);
                    values.push(id(*radius));
                    (2, values)
                }
                Primitive::Box { center, half } => (
                    3,
                    center.iter().chain(half).map(|value| id(*value)).collect(),
                ),
                Primitive::RoundBox {
                    center,
                    half,
                    radius,
                } => {
                    let mut values = center
                        .iter()
                        .chain(half)
                        .map(|value| id(*value))
                        .collect::<Vec<_>>();
                    values.push(id(*radius));
                    (4, values)
                }
                Primitive::Capsule { a, b, radius } => {
                    let mut values = a
                        .iter()
                        .chain(b)
                        .map(|value| id(*value))
                        .collect::<Vec<_>>();
                    values.push(id(*radius));
                    (5, values)
                }
                Primitive::FiniteCylinder { a, b, radius } => {
                    let mut values = a
                        .iter()
                        .chain(b)
                        .map(|value| id(*value))
                        .collect::<Vec<_>>();
                    values.push(id(*radius));
                    (6, values)
                }
                Primitive::FiniteCone {
                    a,
                    b,
                    radius_a,
                    radius_b,
                } => {
                    let mut values = a
                        .iter()
                        .chain(b)
                        .map(|value| id(*value))
                        .collect::<Vec<_>>();
                    values.extend([id(*radius_a), id(*radius_b)]);
                    (7, values)
                }
                Primitive::Torus {
                    center,
                    axis,
                    major,
                    minor,
                } => {
                    let mut values = center
                        .iter()
                        .chain(axis)
                        .map(|value| id(*value))
                        .collect::<Vec<_>>();
                    values.extend([id(*major), id(*minor)]);
                    (8, values)
                }
            };
            operands.insert(0, scalar);
            (tag, operands)
        }
        FieldKind::HardUnion { a, b } => (20, vec![scalar, a.0.into(), b.0.into()]),
        FieldKind::HardIntersection { a, b } => (21, vec![scalar, a.0.into(), b.0.into()]),
        FieldKind::HardSubtract { a, b } => (22, vec![scalar, a.0.into(), b.0.into()]),
        FieldKind::SmoothUnion { a, b, k } => {
            (23, vec![scalar, a.0.into(), b.0.into(), k.0.into()])
        }
        FieldKind::SmoothIntersection { a, b, k } => {
            (24, vec![scalar, a.0.into(), b.0.into(), k.0.into()])
        }
        FieldKind::SmoothSubtract { a, b, k } => {
            (25, vec![scalar, a.0.into(), b.0.into(), k.0.into()])
        }
        FieldKind::Neg { child } => (26, vec![scalar, child.0.into()]),
        FieldKind::Transform { child, transform } => {
            let mut operands = vec![scalar, child.0.into()];
            operands.extend(transform_operands(transform));
            (27, operands)
        }
        FieldKind::FiniteRepeat {
            child,
            axis,
            first,
            count,
            period,
        } => (
            28,
            vec![
                scalar,
                child.0.into(),
                axis_tag(*axis),
                *first as i64 as u64,
                (*count).into(),
                period.0.into(),
            ],
        ),
        FieldKind::BoundedDisplace {
            base,
            displacement,
            contract,
        } => (
            29,
            vec![
                scalar,
                base.0.into(),
                displacement.0.into(),
                contract.amplitude_bound.0.into(),
                contract.gradient_bound.0.into(),
                contract.hessian_bound.0.into(),
                contract.third_derivative_bound.0.into(),
                contract.coordinate_x.0.into(),
                contract.frequency.0.into(),
                contract.phase.0.into(),
                match contract.derivation {
                    super::graph::ClosedDeformDerivation::SinusoidalX => 1,
                },
            ],
        ),
        FieldKind::Mark { child, .. } => (30, vec![scalar, child.0.into()]),
    }
}

fn feature_kind(kind: super::primitive::FeatureKind) -> u16 {
    match kind {
        super::primitive::FeatureKind::Plane => 1,
        super::primitive::FeatureKind::Quadric => 2,
        super::primitive::FeatureKind::Quartic => 3,
    }
}

fn strict_sign_tag(sign: super::projective::StrictSign) -> u64 {
    match sign {
        super::projective::StrictSign::Negative => 1,
        super::projective::StrictSign::Positive => 2,
    }
}

fn strict_obligation_operands(value: super::projective::StrictSignObligation) -> Vec<u64> {
    vec![
        u64::from(value.coefficient.0),
        value.enclosure.lo.to_bits(),
        value.enclosure.hi.to_bits(),
        strict_sign_tag(value.sign),
    ]
}

fn composition_operands(value: &super::polynomial::CompositionPlan) -> Vec<u64> {
    use super::polynomial::CompositionPlan;
    match value {
        CompositionPlan::Specialized(schedule) => {
            let mut output = vec![
                1,
                u64::from(schedule.source_degree_u),
                u64::from(schedule.source_degree_q),
                u64::from(schedule.source_degree_x),
                u64::from(schedule.q_hat_degree),
                u64::from(schedule.correction_face_count),
                u64::from(schedule.composed_degree),
                u64::from(schedule.source_term_count),
                u64::from(schedule.temporary_count),
                schedule.coefficient_order.len() as u64,
            ];
            output.extend(schedule.coefficient_order.iter().copied().map(u64::from));
            output.push(schedule.steps.len() as u64);
            for step in &schedule.steps {
                output.extend([
                    u64::from(step.source_term),
                    u64::from(step.u_power),
                    u64::from(step.q_power),
                    u64::from(step.lifted_power_offset),
                    u64::from(step.coefficient_order),
                ]);
            }
            output.push(schedule.correction_faces.len() as u64);
            for face in &schedule.correction_faces {
                output.extend([
                    match face.correction_sign {
                        -1 => 1,
                        1 => 2,
                        _ => 0,
                    },
                    face.output_coefficient_order.len() as u64,
                ]);
                output.extend(face.output_coefficient_order.iter().copied().map(u64::from));
                output.push(face.steps.len() as u64);
                for step in &face.steps {
                    output.extend([
                        u64::from(step.source_term),
                        u64::from(step.u_power),
                        u64::from(step.q_power),
                        u64::from(step.lifted_power_offset),
                        u64::from(step.coefficient_order),
                    ]);
                }
            }
            output
        }
        CompositionPlan::IntervalTaylorFallback {
            source_degree_u,
            source_degree_q,
            source_degree_x,
            composed_degree,
            source_term_count,
        } => vec![
            2,
            u64::from(*source_degree_u),
            u64::from(*source_degree_q),
            u64::from(*source_degree_x),
            u64::from(*composed_degree),
            u64::from(*source_term_count),
        ],
    }
}

fn event_kind(kind: super::event_kinds::EventKind) -> u64 {
    super::event_kinds::kind_wire_tag(kind)
}

fn representation_tag(representation: &super::events::EventRepresentation) -> u16 {
    super::event_kinds::representation_wire_tag(representation_kind(representation))
}

/// Payload-free tag for a sealed representation, so the wire mapping and the
/// guest-facing classification share one vocabulary (`pixels::event_kinds`).
pub(crate) fn representation_kind(
    representation: &super::events::EventRepresentation,
) -> super::event_kinds::RepresentationTag {
    use super::event_kinds::RepresentationTag as Tag;
    use super::events::EventRepresentation;
    match representation {
        EventRepresentation::LinearLeadingCoefficient { .. } => Tag::LinearLeadingCoefficient,
        EventRepresentation::QuadraticDiscriminant { .. } => Tag::QuadraticDiscriminant,
        EventRepresentation::SparsePredicate { .. } => Tag::SparsePredicate,
        EventRepresentation::DeformationTaylorPredicate { .. } => Tag::DeformationTaylorPredicate,
        EventRepresentation::TorusLocalOracle { .. } => Tag::TorusLocalOracle,
        EventRepresentation::SmoothBandTaylorPredicate { .. } => Tag::SmoothBandTaylorPredicate,
        EventRepresentation::SmoothTieTaylorPredicate { .. } => Tag::SmoothTieTaylorPredicate,
        EventRepresentation::MaterialDifferenceTaylorPredicate { .. } => {
            Tag::MaterialDifferenceTaylorPredicate
        }
        EventRepresentation::RepeatAffineBoundary { .. } => Tag::RepeatAffineBoundary,
        EventRepresentation::ClipQ { .. } => Tag::ClipQ,
        EventRepresentation::ProjectedBoundary { .. } => Tag::ProjectedBoundary,
        EventRepresentation::FixedPointReset => Tag::FixedPointReset,
        EventRepresentation::DirectDepthCrossProduct { .. } => Tag::DirectDepthCrossProduct,
        EventRepresentation::TaylorDepthDifference { .. } => Tag::TaylorDepthDifference,
    }
}

fn scalar_derivative_operands(value: &super::events::ScalarDerivativeProgram) -> Vec<u64> {
    let mut operands = vec![value.sources.len() as u64];
    operands.extend(value.sources.iter().map(|source| u64::from(source.0)));
    operands.extend(value.first_world_abs.iter().map(|bound| bound.to_bits()));
    operands.extend([
        value.second_world_abs.to_bits(),
        value.third_world_abs.to_bits(),
        value.parameter_abs.len() as u64,
    ]);
    for (parameter, bound) in &value.parameter_abs {
        operands.extend([u64::from(parameter.0), bound.to_bits()]);
    }
    operands.extend([
        u64::from(value.frame_delta_abs.is_some()),
        value.frame_delta_abs.unwrap_or(0.0).to_bits(),
        u64::from(value.frame_second_delta_abs.is_some()),
        value.frame_second_delta_abs.unwrap_or(0.0).to_bits(),
    ]);
    operands
}

fn phase_recurrence_operands(value: &super::deform::PhaseRecurrenceProgram) -> Vec<u64> {
    let mut operands = vec![
        u64::from(value.coordinate_x.0),
        u64::from(value.frequency_scalar.0),
        u64::from(value.phase_scalar.0),
        value.frequency.lo.to_bits(),
        value.frequency.hi.to_bits(),
        value.phase.lo.to_bits(),
        value.phase.hi.to_bits(),
    ];
    operands.extend(value.sine_coefficients);
    operands.extend(value.cosine_coefficients);
    operands
}

fn third_derivative_operands(value: &super::derivatives::ThirdDerivatives) -> [u64; 10] {
    [
        u64::from(value.uuu.0),
        u64::from(value.uuv.0),
        u64::from(value.uuq.0),
        u64::from(value.uvv.0),
        u64::from(value.uvq.0),
        u64::from(value.uqq.0),
        u64::from(value.vvv.0),
        u64::from(value.vvq.0),
        u64::from(value.vqq.0),
        u64::from(value.qqq.0),
    ]
}

fn event_representation_operands(representation: &super::events::EventRepresentation) -> Vec<u64> {
    use super::events::EventRepresentation;
    match representation {
        EventRepresentation::LinearLeadingCoefficient { coefficient, root } => {
            vec![u64::from(coefficient.0), u64::from(root.0)]
        }
        EventRepresentation::QuadraticDiscriminant { discriminant, root } => {
            vec![u64::from(discriminant.0), u64::from(root.0)]
        }
        EventRepresentation::SparsePredicate { predicate } => vec![u64::from(predicate.0)],
        EventRepresentation::DeformationTaylorPredicate {
            predictor,
            predictor_derivatives,
            displacement,
            scalar_derivatives,
            phase_recurrence,
            taylor_order,
            world_delta_abs_bound,
            third_derivative_abs_bound,
            remainder,
        } => {
            let mut operands = vec![
                u64::from(predictor.0),
                u64::from(predictor_derivatives.0),
                u64::from(displacement.0),
            ];
            let derivatives = scalar_derivative_operands(scalar_derivatives);
            operands.push(derivatives.len() as u64);
            operands.extend(derivatives);
            let phase = phase_recurrence_operands(phase_recurrence);
            operands.push(phase.len() as u64);
            operands.extend(phase);
            operands.extend([
                u64::from(*taylor_order),
                world_delta_abs_bound.to_bits(),
                third_derivative_abs_bound.to_bits(),
                remainder.to_bits(),
            ]);
            operands
        }
        EventRepresentation::TorusLocalOracle {
            root,
            derivative_u,
            derivative_q,
            derivative_uq,
            derivative_qq,
            third_u,
            value_abs_bound,
            derivative_u_abs_bound,
            derivative_q_abs_bound,
            derivative_uq_abs_bound,
            derivative_qq_abs_bound,
            third_u_abs_bound,
            taylor_order,
            remainder,
        } => vec![
            u64::from(root.0),
            u64::from(derivative_u.0),
            u64::from(derivative_q.0),
            u64::from(derivative_uq.0),
            u64::from(derivative_qq.0),
            u64::from(third_u.0),
            value_abs_bound.to_bits(),
            derivative_u_abs_bound.to_bits(),
            derivative_q_abs_bound.to_bits(),
            derivative_uq_abs_bound.to_bits(),
            derivative_qq_abs_bound.to_bits(),
            third_u_abs_bound.to_bits(),
            u64::from(*taylor_order),
            remainder.to_bits(),
        ],
        EventRepresentation::SmoothBandTaylorPredicate {
            left,
            right,
            left_negated,
            right_negated,
            radius,
            derivatives,
            taylor_order,
            world_delta_abs_bound,
            remainder,
        } => {
            let mut operands = vec![
                u64::from(left.0),
                u64::from(right.0),
                u64::from(*left_negated),
                u64::from(*right_negated),
                u64::from(radius.0),
            ];
            let derivatives = scalar_derivative_operands(derivatives);
            operands.push(derivatives.len() as u64);
            operands.extend(derivatives);
            operands.extend([
                u64::from(*taylor_order),
                world_delta_abs_bound.to_bits(),
                remainder.to_bits(),
            ]);
            operands
        }
        EventRepresentation::SmoothTieTaylorPredicate {
            left,
            right,
            left_negated,
            right_negated,
            derivatives,
            taylor_order,
            world_delta_abs_bound,
            remainder,
        } => {
            let mut operands = vec![
                u64::from(left.0),
                u64::from(right.0),
                u64::from(*left_negated),
                u64::from(*right_negated),
            ];
            let derivatives = scalar_derivative_operands(derivatives);
            operands.push(derivatives.len() as u64);
            operands.extend(derivatives);
            operands.extend([
                u64::from(*taylor_order),
                world_delta_abs_bound.to_bits(),
                remainder.to_bits(),
            ]);
            operands
        }
        EventRepresentation::MaterialDifferenceTaylorPredicate {
            left,
            right,
            comparison,
            derivatives,
            taylor_order,
            world_delta_abs_bound,
            remainder,
        } => {
            let mut operands = vec![
                u64::from(left.0),
                u64::from(right.0),
                compare_tag(*comparison),
            ];
            let derivatives = scalar_derivative_operands(derivatives);
            operands.push(derivatives.len() as u64);
            operands.extend(derivatives);
            operands.extend([
                u64::from(*taylor_order),
                world_delta_abs_bound.to_bits(),
                remainder.to_bits(),
            ]);
            operands
        }
        EventRepresentation::RepeatAffineBoundary { axis, boundary } => {
            vec![
                axis_tag(*axis),
                boundary.lo.to_bits(),
                boundary.hi.to_bits(),
            ]
        }
        EventRepresentation::ClipQ { q } => vec![q.to_bits()],
        EventRepresentation::ProjectedBoundary {
            horizontal,
            coordinate,
        } => vec![u64::from(*horizontal), u64::from(*coordinate)],
        EventRepresentation::FixedPointReset => Vec::new(),
        EventRepresentation::DirectDepthCrossProduct {
            numerator,
            denominator_a,
            denominator_b,
        } => {
            let mut operands = vec![u64::from(numerator.0)];
            operands.extend(strict_obligation_operands(*denominator_a));
            operands.extend(strict_obligation_operands(*denominator_b));
            operands
        }
        EventRepresentation::TaylorDepthDifference {
            a,
            b,
            taylor_order,
            remainder,
        } => {
            let mut operands = vec![u64::from(a.0), u64::from(b.0), u64::from(*taylor_order)];
            operands.extend(third_derivative_operands(&remainder.a_third));
            operands.extend(third_derivative_operands(&remainder.b_third));
            operands.extend([
                u64::from(remainder.next_derivative_order),
                remainder.local_x_domain.lo.to_bits(),
                remainder.local_x_domain.hi.to_bits(),
                remainder.q_domain.lo.to_bits(),
                remainder.q_domain.hi.to_bits(),
                remainder.fallback_difference.lo.to_bits(),
                remainder.fallback_difference.hi.to_bits(),
                remainder.fallback_remainder_abs_bound.to_bits(),
                u64::from(remainder.requires_strict_g_q),
                u64::from(remainder.discard_taylor_on_fallback),
            ]);
            operands
        }
    }
}

fn event_side_tag(value: super::event_kinds::EventSide) -> u64 {
    use super::event_kinds::EventSide;
    match value {
        EventSide::Inactive => 1,
        EventSide::Active => 2,
        EventSide::OutsideValidity => 3,
        EventSide::InsideValidity => 4,
        EventSide::RepeatLeft => 5,
        EventSide::RepeatRight => 6,
        EventSide::SmoothLeft => 7,
        EventSide::SmoothRight => 8,
        EventSide::IdentityLeft => 9,
        EventSide::IdentityRight => 10,
        EventSide::MaterialLeft => 11,
        EventSide::MaterialRight => 12,
        EventSide::OutsideClip => 13,
        EventSide::InsideClip => 14,
        EventSide::ResetOnly => 15,
        EventSide::DepthAFront => 16,
        EventSide::DepthBFront => 17,
        EventSide::RecomputeRootSet => 18,
        EventSide::Ambiguous => 19,
    }
}

fn positive_margin_rule_tag(rule: &str) -> u64 {
    match rule {
        "world-bounds-disjoint" => 1,
        "half-open-projected-span-gap" => 2,
        "positive-q-range-gap" => 3,
        "feature-projection-outside-near-far" => 4,
        "projected-boundary-outside-complete-output-domain" => 5,
        "csg-noninfluential" => 6,
        "feature-validity-impossible" => 7,
        "support-shell-disjoint" => 8,
        "material-class-irrelevant" => 9,
        "static-strict-order" => 10,
        "duplicate-canonical-feature" => 11,
        _ => 0,
    }
}

fn proven_sign_tag(value: super::exclusions::ProvenSign) -> u64 {
    match value {
        super::exclusions::ProvenSign::Negative => 1,
        super::exclusions::ProvenSign::Positive => 2,
    }
}

fn bernstein_proof_operands(value: &super::exclusions::BernsteinProofPayload) -> Vec<u64> {
    let mut operands = vec![value.normalized_box.len() as u64];
    for axis in &value.normalized_box {
        operands.extend([axis.lo.to_bits(), axis.hi.to_bits()]);
    }
    operands.extend([
        value
            .polynomial
            .map_or(0, |program| u64::from(program.0) + 1),
        value
            .coefficient_program_root
            .map_or(0, |coefficient| u64::from(coefficient.0) + 1),
        value.degrees.len() as u64,
    ]);
    operands.extend(value.degrees.iter().copied().map(u64::from));
    operands.push(value.coefficient_order.len() as u64);
    for order in &value.coefficient_order {
        operands.push(order.len() as u64);
        operands.extend(order.iter().copied().map(u64::from));
    }
    operands.extend([
        value.outward_conversion_radius.to_bits(),
        value.subdivision_tree.len() as u64,
    ]);
    for node in &value.subdivision_tree {
        operands.extend([
            u64::from(node.path_bits),
            u64::from(node.depth),
            node.split_variable.map_or(0, |axis| u64::from(axis) + 1),
            node.sign.map_or(0, proven_sign_tag),
            node.margin.to_bits(),
        ]);
    }
    operands.extend([
        proven_sign_tag(value.strict_sign),
        value.minimum_margin.to_bits(),
    ]);
    operands
}

fn exclusion_subject_operands(value: super::exclusions::ExclusionSubject) -> Vec<u64> {
    match value {
        super::exclusions::ExclusionSubject::Candidate(feature) => {
            vec![1, u64::from(feature.0)]
        }
        super::exclusions::ExclusionSubject::Event(subject) => vec![
            2,
            event_kind(subject.kind),
            subject.feature.map_or(0, |value| u64::from(value.0) + 1),
            subject.owner.map_or(0, |value| u64::from(value.0) + 1),
            u64::from(subject.ordinal),
        ],
        super::exclusions::ExclusionSubject::Competition(subject) => {
            vec![3, u64::from(subject.a.0), u64::from(subject.b.0)]
        }
    }
}

fn camera_coeff(value: CameraCoeff) -> u64 {
    match value {
        CameraCoeff::Eye(component) => 0x0100 | u64::from(component),
        CameraCoeff::Forward(component) => 0x0200 | u64::from(component),
        CameraCoeff::Right(component) => 0x0300 | u64::from(component),
        CameraCoeff::Up(component) => 0x0400 | u64::from(component),
        CameraCoeff::EyeRate(component) => 0x0500 | u64::from(component),
        CameraCoeff::ForwardRate(component) => 0x0600 | u64::from(component),
        CameraCoeff::RightRate(component) => 0x0700 | u64::from(component),
        CameraCoeff::UpRate(component) => 0x0800 | u64::from(component),
        CameraCoeff::TanHalfFovY => 0x0900,
        CameraCoeff::Aspect => 0x0a00,
    }
}

fn capacity_values(
    structural: &super::capacities::StructuralCapacities,
    projective: &super::capacities::ProjectiveCapacities,
) -> Vec<u64> {
    vec![
        structural.worker_count.into(),
        structural.object_count.into(),
        structural.feature_count.into(),
        structural.parameter_slots.into(),
        structural.max_csg_stack.into(),
        projective.candidate_features_per_tile.into(),
        projective.row_start_roots.into(),
        projective.active_sheets_per_row.into(),
        projective.row_event_intervals.into(),
        projective.root_stack_nodes.into(),
        projective.event_stack_nodes.into(),
        projective.runs_per_row.into(),
        projective.corridors_per_row.into(),
        structural.max_transparent_layers.into(),
        projective.polynomial_terms_per_program.into(),
        structural.probe_bytes,
        projective.total_renderer_state_bytes,
        projective.index_bytes,
        projective.coefficient_nodes.into(),
        projective.polynomial_programs.into(),
        projective.rational_programs.into(),
        projective.derivative_bundles.into(),
        projective.derivative_clusters.into(),
        projective.event_generators.into(),
        projective.competition_pairs_per_tile.into(),
        projective.max_index_slice.into(),
        projective.final_per_worker_scratch_bytes,
        projective.final_all_worker_scratch_bytes,
        structural.max_event_records.into(),
        structural.max_run_records_per_tile_row.into(),
        structural.max_local_rebuild_queue.into(),
    ]
}

/// Re-derives, at the exponent the frame program actually seals, the fixed-point
/// interval the guest will clamp a feature's root-`q` search to, and confirms
/// that every near/far clip event omitted as vacuous really is unreachable.
///
/// The guest reads a feature's sealed projected `q` span through
/// `__wrela_pixels_p7_feature_q_span` (f64 bits -> f32 -> one outward f32 ulp)
/// and then through `__wrela_pixels_p7_interval_from_f32`, which truncates to
/// the sealed exponent, widens by one unit for the truncation, and widens again
/// by `|raw| / 65536 + 64` quanta. `select_visibility` intersects the global
/// `[q_lo, q_hi]` raw domain with the result, so a feature whose widened raw
/// span stops strictly short of the global raw bound can never produce a root on
/// that clip plane, and the clip crossing the omitted event would have marked
/// does not exist anywhere in the output.
fn verify_clip_q_omissions(
    projective: &super::verify::ProjectiveProgram,
    exponent: i16,
    q_lo: f64,
    q_hi: f64,
) -> Result<(), String> {
    use super::reference::interval::{next_down, next_down_f32, next_up, next_up_f32};

    let scale = 2.0_f64.powi(-i32::from(exponent));
    if !scale.is_finite() || scale <= 0.0 {
        return Err("P017: fixed-q exponent has no finite reciprocal scale".to_string());
    }
    // The same global raw bounds the generated glue seals for the guest.
    let global_lo_raw = (q_lo * scale).floor();
    let global_hi_raw = (q_hi * scale).ceil();
    // Truncation slack, then `|raw| / 65536 + 64`, both outward.
    let widen = |raw: f64| next_up(1.0 + (raw.abs() * scale).abs() / 65536.0 + 64.0);
    for entry in &projective.events.ledger {
        let Some(super::events::OmissionHint::ClipQOutsideFeatureQSpan {
            clip_q, feature_q, ..
        }) = entry.omission.as_ref()
        else {
            continue;
        };
        let feature = entry.subject.feature.ok_or_else(|| {
            format!(
                "pixels::program: clip-q omission {:?} has no feature",
                entry.subject
            )
        })?;
        let near = matches!(entry.subject.kind, super::event_kinds::EventKind::NearClip);
        let guest_edge = if near {
            f64::from(next_up_f32(feature_q.hi as f32))
        } else {
            f64::from(next_down_f32(feature_q.lo as f32))
        };
        let scaled = guest_edge * scale;
        if !scaled.is_finite() {
            return Err(format!(
                "P017: clip-q omission for {feature} does not scale into the sealed fixed-q domain"
            ));
        }
        let reachable = if near {
            next_up(scaled.ceil() + widen(guest_edge))
        } else {
            next_down(scaled.floor() - widen(guest_edge))
        };
        let excluded = if near {
            reachable < global_hi_raw
        } else {
            reachable > global_lo_raw
        };
        if !excluded {
            return Err(format!(
                "P017: clip event {:?} for {feature} was omitted as vacuous, but at fixed-q exponent \
                 {exponent} the guest's clamped root-q interval still reaches clip q={clip_q} \
                 (raw reach {reachable}, global raw domain [{global_lo_raw}, {global_hi_raw}])",
                entry.subject.kind,
            ));
        }
    }
    Ok(())
}

fn maximum_incident_radiance_v1(kind: &str, radiance_max: [f32; 3]) -> Result<[f32; 3], String> {
    if kind == "Disabled" {
        return Ok([0.0; 3]);
    }
    let scale = match kind {
        "Point" => {
            let radius = super::reference::light::POINT_RADIUS_MIN_V1 as f32;
            1.0 / (radius * radius)
        }
        "Directional" | "Rectangle" | "Disk" => 1.0,
        other => return Err(format!("P015: unknown sealed light kind `{other}`")),
    };
    let result = radiance_max.map(|component| component * scale);
    if result.into_iter().any(|component| !component.is_finite()) {
        return Err(format!(
            "P007: maximum incident radiance for {kind} light is not representable as f32"
        ));
    }
    Ok(result)
}

pub fn finish_frame_program(
    renderer_index: usize,
    graph: &super::symbolic::SymbolicGraph,
    structural: &super::verify::VerifiedStructuralProgram,
    projective: &super::verify::VerifiedProjectiveProgram,
    config: &super::config::RendererConfig,
) -> Result<FrameProgram, String> {
    let q_lo = 1.0 / config.far;
    let q_hi = 1.0 / config.near;
    let q_span = q_hi - q_lo;
    // For three consecutive values within the certified near/far q range,
    // |dq| <= span and |ddq| <= 2*span. The family setup is deliberately
    // conservative; P7 may select tighter per-run records but may not exceed
    // this sealed domain/error policy.
    let reciprocal_model_error = q_hi * f64::from(f32::EPSILON) * 8.0 + f64::EPSILON;
    let fixed_q_setup = super::reference::fixed_q::setup_from_real_envelopes(
        q_lo,
        q_hi,
        q_span,
        q_span * 2.0,
        64,
        reciprocal_model_error,
    )
    .map_err(|_| {
        format!(
            "P017: fixed-q setup cannot represent the certified q/dq/ddq envelope in any v1 dyadic i32 domain\n\
             q range is [{q_lo}, {q_hi}], |dq| <= {q_span}, |ddq| <= {}, exponent range is [-96, 63], and saturation is forbidden",
            q_span * 2.0,
        )
    })?;
    let renderer_index = u16::try_from(renderer_index)
        .map_err(|_| "P015: renderer index exceeds FrameProgram u16".to_string())?;
    let structural = structural.program();
    let projective = projective.program();
    verify_clip_q_omissions(projective, fixed_q_setup.run.domain.exponent, q_lo, q_hi)?;
    let mut by_kind = BTreeMap::<FrameProgramTableKindV1, Vec<FrameRecord>>::new();
    for kind in FrameProgramTableKindV1::ALL {
        by_kind.insert(kind, Vec::new());
    }

    let scalars = by_kind
        .get_mut(&FrameProgramTableKindV1::Scalar)
        .expect("namespace seeded");
    for (_, node) in graph.scalar.iter() {
        let (tag, mut operands) = scalar_record(node);
        operands.insert(0, dependency_tag(node.dependency));
        push_record(scalars, tag, 0, operands);
    }

    let fields = by_kind
        .get_mut(&FrameProgramTableKindV1::Field)
        .expect("namespace seeded");
    for (_, node) in graph.fields.iter() {
        let (tag, operands) = field_record(node);
        push_record(fields, tag, 0, operands);
    }

    let objects = by_kind
        .get_mut(&FrameProgramTableKindV1::Object)
        .expect("namespace seeded");
    for object in &structural.objects.objects {
        let mut operands = vec![
            object.id.0.into(),
            object.source_root.0.into(),
            object.scalar_root.0.into(),
            object.identity_set.into(),
            object.primitive_occurrences.len() as u64,
            object.repeat_instances.len() as u64,
            object.support_max.lo.to_bits(),
            object.support_max.hi.to_bits(),
        ];
        for axis in [&object.bounds.min, &object.bounds.max] {
            operands.extend(axis.iter().map(|value| value.to_bits()));
        }
        for path in &object.primitive_occurrences {
            operands.push(path.len() as u64);
            for step in path {
                operands.extend([u64::from(step.field.0), u64::from(step.child_slot)]);
            }
        }
        for repeat in &object.repeat_instances {
            operands.extend([
                u64::from(repeat.repeat_field.0),
                repeat.equivalent_fields.len() as u64,
            ]);
            operands.extend(
                repeat
                    .equivalent_fields
                    .iter()
                    .map(|field| u64::from(field.0)),
            );
            operands.extend([
                axis_tag(repeat.axis),
                repeat.first as i64 as u64,
                repeat.index as i64 as u64,
                repeat.period.lo.to_bits(),
                repeat.period.hi.to_bits(),
            ]);
        }
        push_record(objects, 1, 0, operands);
    }

    let features = by_kind
        .get_mut(&FrameProgramTableKindV1::Feature)
        .expect("namespace seeded");
    for (feature, equation) in structural
        .features
        .iter()
        .zip(&projective.equations.features)
    {
        let span = projective
            .spans
            .get(feature.id.index())
            .ok_or_else(|| format!("pixels::program: missing projected span for {}", feature.id))?;
        let mut operands = vec![
            feature.object.0.into(),
            feature.primitive.0.into(),
            feature.template_id.into(),
            feature.identity_set.into(),
            feature.scalar_semantic_root.0.into(),
            equation.root_equation.0.into(),
            equation.q_degree.into(),
            equation.max_root_count.into(),
            span.pixels.x.start.into(),
            span.pixels.x.end.into(),
            span.pixels.y.start.into(),
            span.pixels.y.end.into(),
            span.tiles.x.start.into(),
            span.tiles.x.end.into(),
            span.tiles.y.start.into(),
            span.tiles.y.end.into(),
            span.q.lo.to_bits(),
            span.q.hi.to_bits(),
            feature.support_expand.to_bits(),
        ];
        operands.extend([
            equation
                .rational_program
                .map_or(0, |value| u64::from(value.0) + 1),
            equation.validity_predicates.len() as u64,
        ]);
        operands.extend(
            equation
                .validity_predicates
                .iter()
                .map(|value| u64::from(value.0)),
        );
        operands.push(match equation.orientation_program {
            super::primitive::OrientationProgram::Outward => 1,
            super::primitive::OrientationProgram::Inward => 2,
            super::primitive::OrientationProgram::DeformedOutward => 3,
            super::primitive::OrientationProgram::DeformedInward => 4,
        });
        match equation.q_seed_kind {
            super::projective::SeedKind::Affine { denominator } => {
                operands.push(1);
                operands.extend(strict_obligation_operands(denominator));
            }
            super::projective::SeedKind::StableQuadratic {
                leading_coefficient,
                leading_enclosure,
                leading_sign,
                linear_fallback,
                generic_isolation_fallback,
            } => operands.extend([
                2,
                u64::from(leading_coefficient.0),
                leading_enclosure.lo.to_bits(),
                leading_enclosure.hi.to_bits(),
                leading_sign.map_or(0, strict_sign_tag),
                u64::from(linear_fallback),
                u64::from(generic_isolation_fallback),
            ]),
            super::projective::SeedKind::GenericIsolatedRoot => operands.push(3),
        }
        match equation.root_isolation {
            super::projective::RootIsolationProgram::Affine => operands.push(1),
            super::projective::RootIsolationProgram::StableQuadratic {
                linear_fallback,
                generic_isolation_fallback,
            } => operands.extend([
                2,
                u64::from(linear_fallback),
                u64::from(generic_isolation_fallback),
            ]),
            super::projective::RootIsolationProgram::CertifiedBernstein {
                maximum_depth,
                ambiguity_depth,
                preserve_all_positive_q_roots,
            } => operands.extend([
                3,
                u64::from(maximum_depth),
                u64::from(ambiguity_depth),
                u64::from(preserve_all_positive_q_roots),
            ]),
        }
        let composition = composition_operands(&equation.quadratic_composition);
        operands.push(composition.len() as u64);
        operands.extend(composition);
        operands.extend([
            u64::from(equation.deformed_predictor),
            equation.influencing_params.len() as u64,
        ]);
        operands.extend(
            equation
                .influencing_params
                .iter()
                .map(|param| u64::from(param.0)),
        );
        operands.extend([
            feature.occurrence_path.len() as u64,
            u64::from(feature.validity.shared_boundary),
        ]);
        for step in &feature.occurrence_path {
            operands.extend([u64::from(step.field.0), u64::from(step.child_slot)]);
        }
        for axis in [&feature.world_bounds.min, &feature.world_bounds.max] {
            operands.extend(axis.iter().map(|value| value.to_bits()));
        }
        push_record(features, feature_kind(feature.kind), 0, operands);
    }

    let materials = by_kind
        .get_mut(&FrameProgramTableKindV1::Material)
        .expect("namespace seeded");
    for (_, node) in graph.materials.iter() {
        use super::material_graph::MaterialKind;
        match &node.kind {
            MaterialKind::Sample(sample) => {
                let mut operands = sample
                    .base_color
                    .iter()
                    .chain(&sample.emissive)
                    .map(|value| id(*value))
                    .collect::<Vec<_>>();
                operands.extend([
                    id(sample.opacity),
                    id(sample.roughness),
                    id(sample.metallic),
                    id(sample.specular_level),
                    id(sample.ior),
                ]);
                let (normal_tag, normal_x, normal_y, normal_texture) = match &sample.normal {
                    super::material_graph::NormalModel::Geometric => {
                        (0, u64::MAX, u64::MAX, u64::MAX)
                    }
                    super::material_graph::NormalModel::AnalyticSlope { x, y } => {
                        (1, u64::from(x.0), u64::from(y.0), u64::MAX)
                    }
                    super::material_graph::NormalModel::TextureSlope { texture } => {
                        (2, u64::MAX, u64::MAX, u64::from(texture.stable_id))
                    }
                };
                let (pattern_texture, pattern_filter) =
                    sample
                        .pattern
                        .as_ref()
                        .map_or((u64::MAX, u64::MAX), |texture| {
                            let filter = match texture.filter {
                                super::material_graph::TextureFilterV1::Nearest => 0,
                                super::material_graph::TextureFilterV1::Bilinear => 1,
                                super::material_graph::TextureFilterV1::Trilinear => 2,
                                super::material_graph::TextureFilterV1::Anisotropic4 => 3,
                            };
                            (u64::from(texture.stable_id), filter)
                        });
                operands.extend([
                    normal_tag,
                    normal_x,
                    normal_y,
                    normal_texture,
                    pattern_texture,
                    pattern_filter,
                ]);
                push_record(materials, 1, 0, operands);
            }
            MaterialKind::Select { predicate, a, b } => push_record(
                materials,
                2,
                0,
                vec![id(*predicate), a.0.into(), b.0.into()],
            ),
            MaterialKind::IdentityTable { cases, .. } => {
                let mut operands = vec![cases.len() as u64];
                operands.extend(cases.iter().map(|(_, material)| u64::from(material.0)));
                push_record(materials, 3, 0, operands);
            }
        }
    }

    // P9 immutable texture records own the complete mip/minmax/moment bytes.
    // Stable asset IDs, not material traversal order, decide record order.
    let mut texture_ids = std::collections::BTreeSet::new();
    for (_, node) in graph.materials.iter() {
        if let super::material_graph::MaterialKind::Sample(sample) = &node.kind {
            if let Some(texture) = &sample.pattern {
                texture_ids.insert(texture.stable_id);
            }
            if let super::material_graph::NormalModel::TextureSlope { texture } = &sample.normal {
                texture_ids.insert(texture.stable_id);
            }
        }
    }
    let textures = by_kind
        .get_mut(&FrameProgramTableKindV1::Texture)
        .expect("namespace seeded");
    for stable_id in texture_ids {
        let asset = super::texture::compiler_asset(stable_id)?;
        let digest = asset.digest_bytes();
        let mut operands = vec![
            u64::from(asset.stable_id),
            asset.format.tag(),
            u64::from(asset.width),
            u64::from(asset.height),
            match asset.wrap_u {
                super::texture::WrapMode::Clamp => 0,
                super::texture::WrapMode::Repeat => 1,
            },
            match asset.wrap_v {
                super::texture::WrapMode::Clamp => 0,
                super::texture::WrapMode::Repeat => 1,
            },
            asset.mips.len() as u64,
        ];
        operands.extend(
            digest
                .chunks_exact(8)
                .map(|word| u64::from_le_bytes(word.try_into().expect("eight-byte digest word"))),
        );
        for mip in asset.mips {
            operands.extend([
                u64::from(mip.width),
                u64::from(mip.height),
                mip.bytes.len() as u64,
            ]);
            operands.extend(mip.bytes.into_iter().map(u64::from));
            operands.push(mip.channel_min.len() as u64);
            operands.extend(mip.channel_min.into_iter().map(u64::from));
            operands.extend(mip.channel_max.into_iter().map(u64::from));
            match mip.slope_moments {
                Some(moments) => {
                    operands.push(1);
                    operands.push(moments.len() as u64);
                    operands.extend(moments.into_iter().flatten().map(|value| value as u64));
                }
                None => operands.push(0),
            }
        }
        push_record(textures, 1, 0, operands);
    }

    // One auditable summary-program record per identity/material leaf. The
    // runtime may choose any ladder rung no stronger than `basis`; every
    // output range remains an explicit compiler-derived premise.
    let summaries = by_kind
        .get_mut(&FrameProgramTableKindV1::ShadingSummary)
        .expect("namespace seeded");
    for material_index in &projective.indexes.material_programs {
        let mut stack = vec![material_index.material];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(material_id) = stack.pop() {
            if !seen.insert(material_id) {
                continue;
            }
            match &graph.materials.get(material_id)?.kind {
                super::material_graph::MaterialKind::Sample(sample) => {
                    let scalar_ids = sample
                        .base_color
                        .iter()
                        .chain(&sample.emissive)
                        .copied()
                        .chain([
                            sample.opacity,
                            sample.roughness,
                            sample.metallic,
                            sample.specular_level,
                            sample.ior,
                        ])
                        .collect::<Vec<_>>();
                    let mut input_bits = 0_u64;
                    let mut coordinate_dependent = false;
                    let mut parameter_dependent = false;
                    for scalar in &scalar_ids {
                        match graph.scalar.get(*scalar)?.dependency {
                            super::scalar::Dependency::Constant => {}
                            super::scalar::Dependency::Coordinate => coordinate_dependent = true,
                            super::scalar::Dependency::Parameter => parameter_dependent = true,
                            super::scalar::Dependency::Surface => input_bits |= 1 | 2,
                            super::scalar::Dependency::CoordinateAndParameter => {
                                coordinate_dependent = true;
                                parameter_dependent = true;
                            }
                            super::scalar::Dependency::CoordinateAndSurface => {
                                coordinate_dependent = true;
                                input_bits |= 1 | 2;
                            }
                            super::scalar::Dependency::ParameterAndSurface => {
                                parameter_dependent = true;
                                input_bits |= 1 | 2;
                            }
                            super::scalar::Dependency::CoordinateParameterAndSurface => {
                                coordinate_dependent = true;
                                parameter_dependent = true;
                                input_bits |= 1 | 2;
                            }
                        }
                    }
                    if coordinate_dependent {
                        input_bits |= 1 | 2 | 4;
                    }
                    if parameter_dependent {
                        input_bits |= 8;
                    }
                    if sample.pattern.is_some() {
                        input_bits |= 16 | 32;
                    }
                    if !matches!(sample.normal, super::material_graph::NormalModel::Geometric) {
                        input_bits |= 64;
                    }
                    let basis = if coordinate_dependent || sample.pattern.is_some() {
                        5_u64 // exact per-pixel fallback is the only unconditional premise
                    } else {
                        1_u64 // constant summary
                    };
                    let mut operands = vec![
                        u64::from(material_index.identity_set),
                        u64::from(material_id.0),
                        input_bits,
                        basis,
                        0, // rank
                        0, // anchor count; proposer anchors are runtime scratch
                        scalar_ids.len() as u64,
                    ];
                    for scalar in scalar_ids {
                        let range = structural.values.get(scalar)?;
                        operands.extend([
                            u64::from(scalar.0),
                            range.lo.to_bits(),
                            range.hi.to_bits(),
                        ]);
                    }
                    operands.extend([
                        sample
                            .pattern
                            .as_ref()
                            .map_or(u64::MAX, |texture| u64::from(texture.stable_id)),
                        match &sample.normal {
                            super::material_graph::NormalModel::TextureSlope { texture } => {
                                u64::from(texture.stable_id)
                            }
                            _ => u64::MAX,
                        },
                        sample
                            .pattern
                            .as_ref()
                            .map_or(8, |texture| texture.uv_source.tag()),
                        match &sample.normal {
                            super::material_graph::NormalModel::TextureSlope { texture } => {
                                texture.uv_source.tag()
                            }
                            _ => 8,
                        },
                        // Exact-per-pixel rung: no proposer coefficient is
                        // consumed, and the arithmetic residual is the sealed
                        // zero interval in every HDR channel. Runtime shading
                        // still carries these point intervals through the
                        // common singleton-byte gate.
                        0,
                        0.0_f64.to_bits(),
                        0.0_f64.to_bits(),
                        0.0_f64.to_bits(),
                    ]);
                    push_record(summaries, 1, 0, operands);
                }
                super::material_graph::MaterialKind::Select { a, b, .. } => stack.extend([*a, *b]),
                super::material_graph::MaterialKind::IdentityTable { cases, .. } => {
                    stack.extend(cases.iter().map(|(_, material)| *material));
                }
            }
        }
    }
    push_record(
        summaries,
        2,
        0,
        vec![
            u64::from(config.light_capacity),
            u64::from(config.ao_enabled),
            u64::from(config.ao_radius.to_bits()),
            u64::from(config.ao_strength.to_bits()),
            5,  // AO tap count
            4,  // maximum separable rank
            25, // fixed cross-pivot grid entries
        ],
    );
    for (slot, range) in config.light_ranges.iter().enumerate() {
        let kind = config
            .light_kinds
            .get(slot)
            .map(String::as_str)
            .unwrap_or("Disabled");
        let maximum_incident = maximum_incident_radiance_v1(kind, range.radiance_max)?;
        push_record(
            summaries,
            6,
            0,
            vec![
                slot as u64,
                u64::from(range.position_min.x.to_bits()),
                u64::from(range.position_min.y.to_bits()),
                u64::from(range.position_min.z.to_bits()),
                u64::from(range.position_max.x.to_bits()),
                u64::from(range.position_max.y.to_bits()),
                u64::from(range.position_max.z.to_bits()),
                u64::from(range.axis_component_max.to_bits()),
                u64::from(range.radiance_max[0].to_bits()),
                u64::from(range.radiance_max[1].to_bits()),
                u64::from(range.radiance_max[2].to_bits()),
                u64::from(range.max_delta.to_bits()),
                // No supported v1 light has a finite receiver cutoff. These
                // are conservative receiver-influence bounds, not emitter
                // geometry bounds, so every slot seals the whole world.
                u64::from(config.world_min.x.to_bits()),
                u64::from(config.world_min.y.to_bits()),
                u64::from(config.world_min.z.to_bits()),
                u64::from(config.world_max.x.to_bits()),
                u64::from(config.world_max.y.to_bits()),
                u64::from(config.world_max.z.to_bits()),
                u64::from(maximum_incident[0].to_bits()),
                u64::from(maximum_incident[1].to_bits()),
                u64::from(maximum_incident[2].to_bits()),
            ],
        );
    }

    // Compiler-emitted surface-object BVH used only by secondary segments.
    // It indexes complete object/feature bounds and never substitutes a
    // volumetric approximation for the field evaluator.
    let mut secondary_objects = Vec::new();
    for object in &structural.objects.objects {
        let range = projective
            .indexes
            .object_features
            .iter()
            .find(|range| range.object == object.id)
            .ok_or_else(|| format!("P027: object {} has no feature range", object.id))?;
        secondary_objects.push(super::reference::secondary::SurfaceObject {
            object_id: object.id.0,
            feature_first: range.first.0,
            feature_count: range.count,
            bounds: super::reference::secondary::Aabb {
                min: super::reference::light::Vec3 {
                    x: object.bounds.min[0],
                    y: object.bounds.min[1],
                    z: object.bounds.min[2],
                },
                max: super::reference::light::Vec3 {
                    x: object.bounds.max[0],
                    y: object.bounds.max[1],
                    z: object.bounds.max[2],
                },
            },
        });
    }
    if !secondary_objects.is_empty() {
        let bvh = super::reference::secondary::SurfaceBvh::build(secondary_objects)?;
        let (objects, nodes, root) = bvh.wire_parts();
        let mut operands = vec![
            objects.len() as u64,
            nodes.len() as u64,
            root as u64,
            bvh.stack_capacity as u64,
        ];
        for object in objects {
            operands.extend([
                u64::from(object.object_id),
                u64::from(object.feature_first),
                u64::from(object.feature_count),
                secondary_lower_f32_bits(object.bounds.min.x),
                secondary_lower_f32_bits(object.bounds.min.y),
                secondary_lower_f32_bits(object.bounds.min.z),
                secondary_upper_f32_bits(object.bounds.max.x),
                secondary_upper_f32_bits(object.bounds.max.y),
                secondary_upper_f32_bits(object.bounds.max.z),
            ]);
        }
        for node in nodes {
            operands.extend([
                secondary_lower_f32_bits(node.bounds.min.x),
                secondary_lower_f32_bits(node.bounds.min.y),
                secondary_lower_f32_bits(node.bounds.min.z),
                secondary_upper_f32_bits(node.bounds.max.x),
                secondary_upper_f32_bits(node.bounds.max.y),
                secondary_upper_f32_bits(node.bounds.max.z),
                node.first as u64,
                node.count as u64,
                node.left.map_or(u64::MAX, |value| value as u64),
                node.right.map_or(u64::MAX, |value| value as u64),
            ]);
        }
        push_record(summaries, 3, 0, operands);
    }

    // The canonical P9 transfer tables are shared image data. Only renderer
    // zero owns their packed records; generated accessors for every renderer
    // deliberately reference that one verified copy. Four little-endian u16
    // entries per operand keep the FrameProgram representation bounded.
    if renderer_index == 0 {
        for (tag, kind) in [
            (4, super::tables::TableKind::FilmicV1),
            (5, super::tables::TableKind::SrgbV1),
        ] {
            let values = super::tables::values(kind)?;
            let digest = wrela_machine::sha256::sha256(kind.bytes());
            let mut operands = vec![values.len() as u64];
            operands.extend(
                digest
                    .chunks_exact(8)
                    .map(|word| u64::from_le_bytes(word.try_into().expect("digest word"))),
            );
            for entries in values.chunks(4) {
                let mut word = 0_u64;
                for (index, value) in entries.iter().enumerate() {
                    word |= u64::from(*value) << (index * 16);
                }
                operands.push(word);
            }
            push_record(summaries, tag, 0, operands);
        }
    }

    let params = by_kind
        .get_mut(&FrameProgramTableKindV1::Parameter)
        .expect("namespace seeded");
    for slot in &structural.params.slots {
        let uses = slot.uses.iter().fold(0_u64, |bits, use_kind| {
            bits | (1_u64 << param_use_bit(*use_kind))
        });
        let mut operands = vec![
            slot.packed_offset.into(),
            slot.component.map_or(u64::MAX, u64::from),
            scalar_type_tag(slot.scalar_ty),
            slot.range.min.to_bits(),
            slot.range.max.to_bits(),
            u64::from(slot.immutable),
            uses,
            slot.path.len() as u64,
        ];
        operands.extend(slot.path.iter().map(|part| *part as u64));
        if let Some(rate) = slot.rate {
            operands.extend([rate.max_delta.to_bits(), rate.max_second_delta.to_bits()]);
        }
        push_record(params, 1, u16::from(slot.rate.is_some()), operands);
    }

    let events = by_kind
        .get_mut(&FrameProgramTableKindV1::Event)
        .expect("namespace seeded");
    for event in &projective.events.generators {
        let mut operands = vec![
            event_kind(event.kind),
            event.maximum_root_count.into(),
            event.subdivision_depth.into(),
            event.pixels.x.start.into(),
            event.pixels.x.end.into(),
            event.pixels.y.start.into(),
            event.pixels.y.end.into(),
            event.tiles.x.start.into(),
            event.tiles.x.end.into(),
            event.tiles.y.start.into(),
            event.tiles.y.end.into(),
            event.participants.iter().count() as u64,
            event.coefficient_dependencies.len() as u64,
        ];
        for participant in event.participants.iter() {
            let (tag, value) = match participant {
                super::events::Participant::Feature(value) => (1_u64, value.0),
                super::events::Participant::Object(value) => (2_u64, value.0),
                super::events::Participant::Field(value) => (3_u64, value.0),
                super::events::Participant::MaterialEvent(value) => (4_u64, value),
            };
            operands.extend([tag, value.into()]);
        }
        operands.extend(
            event
                .coefficient_dependencies
                .iter()
                .map(|value| u64::from(value.0)),
        );
        let representation = event_representation_operands(&event.representation);
        operands.push(representation.len() as u64);
        operands.extend(representation);
        operands.extend([
            event_side_tag(event.side_meaning.negative),
            event_side_tag(event.side_meaning.zero),
            event_side_tag(event.side_meaning.positive),
        ]);
        push_record(
            events,
            representation_tag(&event.representation),
            0,
            operands,
        );
    }

    let csg = by_kind
        .get_mut(&FrameProgramTableKindV1::Csg)
        .expect("namespace seeded");
    if let Some(value) = structural.csg.constant {
        push_record(csg, if value { 5 } else { 6 }, 0, vec![]);
    } else {
        for instruction in &structural.csg.instructions {
            match instruction {
                super::csg::CsgInst::Push(object) => push_record(csg, 1, 0, vec![object.0.into()]),
                super::csg::CsgInst::Not => push_record(csg, 2, 0, vec![]),
                super::csg::CsgInst::And => push_record(csg, 3, 0, vec![]),
                super::csg::CsgInst::Or => push_record(csg, 4, 0, vec![]),
            }
        }
    }

    let fixed = by_kind
        .get_mut(&FrameProgramTableKindV1::FixedDomain)
        .expect("namespace seeded");
    push_record(
        fixed,
        1,
        0,
        capacity_values(&structural.capacities, &projective.capacities),
    );
    push_record(
        fixed,
        5,
        0,
        vec![
            fixed_q_setup.run.domain.exponent as i64 as u64,
            u64::try_from(fixed_q_setup.maximum_raw)
                .map_err(|_| "P017: fixed-q maximum raw value is negative".to_string())?,
            u64::from(fixed_q_setup.run.microtile_width),
            u64::try_from(fixed_q_setup.run.error_radius)
                .map_err(|_| "P017: fixed-q error radius is negative".to_string())?,
        ],
    );
    // PositiveMargin `facts` are compiler-only explanatory strings. Runtime
    // consumes the versioned rule tag plus the exclusion's exact margin and
    // subject; Bernstein proofs retain their complete numeric payload.
    for proof in &projective.exclusions.proofs {
        match &proof.payload {
            super::exclusions::ProofPayload::PositiveMargin { rule, .. } => {
                push_record(
                    fixed,
                    3,
                    0,
                    vec![u64::from(proof.id.0), positive_margin_rule_tag(rule)],
                );
            }
            super::exclusions::ProofPayload::Bernstein(payload) => {
                let mut operands = vec![u64::from(proof.id.0)];
                operands.extend(bernstein_proof_operands(payload));
                push_record(fixed, 4, 0, operands);
            }
        }
    }
    for exclusion in &projective.exclusions.records {
        let mut operands = vec![
            exclusion.id.0.into(),
            exclusion.domain.0.into(),
            exclusion_reason_tag(exclusion.reason),
            exclusion.margin.lo.to_bits(),
            exclusion.margin.hi.to_bits(),
            exclusion.proof.0.into(),
            exclusion.dependencies.len() as u64,
        ];
        operands.extend(
            exclusion
                .dependencies
                .iter()
                .map(|dependency| u64::from(dependency.0)),
        );
        let subject = exclusion_subject_operands(exclusion.subject);
        operands.push(subject.len() as u64);
        operands.extend(subject);
        push_record(fixed, 2, 0, operands);
    }
    for node in &projective.equations.coefficients.nodes {
        use CoeffOp;
        let (tag, operands) = match node.op {
            CoeffOp::ConstF64(bits) => (10, vec![node.id.0.into(), bits]),
            CoeffOp::Scalar(value) => (11, vec![node.id.0.into(), value.0.into()]),
            CoeffOp::Camera(value) => (12, vec![node.id.0.into(), camera_coeff(value)]),
            CoeffOp::ScalarParamDerivative(value, param) => {
                (13, vec![node.id.0.into(), value.0.into(), param.0.into()])
            }
            CoeffOp::ParamRate(param, order) => {
                (14, vec![node.id.0.into(), param.0.into(), order.into()])
            }
            CoeffOp::Add(a, b) => (15, vec![node.id.0.into(), a.0.into(), b.0.into()]),
            CoeffOp::Mul(a, b) => (16, vec![node.id.0.into(), a.0.into(), b.0.into()]),
            CoeffOp::Neg(value) => (17, vec![node.id.0.into(), value.0.into()]),
        };
        push_record(fixed, tag, 0, operands);
    }
    for polynomial in &projective.equations.polynomials {
        let mut operands = vec![
            polynomial.id.0.into(),
            polynomial.terms.len() as u64,
            polynomial.degree_u.into(),
            polynomial.degree_v.into(),
            polynomial.degree_q.into(),
            polynomial.degree_x.into(),
            polynomial.degree_t.into(),
            polynomial.coefficient_program.into(),
        ];
        for term in &polynomial.terms {
            operands.push(term.coefficient.0.into());
            operands.extend([
                term.exponents.u.into(),
                term.exponents.v.into(),
                term.exponents.q.into(),
                term.exponents.x.into(),
                term.exponents.t.into(),
                term.exponents.param_terms.iter().count() as u64,
            ]);
            for param in term.exponents.param_terms.iter() {
                operands.extend([u64::from(param.param.0), u64::from(param.exponent)]);
            }
        }
        push_record(fixed, 20, 0, operands);
    }
    for rational in &projective.equations.rationals {
        let mut operands = vec![
            u64::from(rational.id.0),
            u64::from(rational.numerator.0),
            u64::from(rational.denominator.0),
            u64::from(rational.domain.0),
        ];
        operands.extend(strict_obligation_operands(rational.denominator_proof));
        push_record(fixed, 21, 0, operands);
    }
    for predicate in &projective.equations.predicates {
        push_record(
            fixed,
            22,
            0,
            vec![
                u64::from(predicate.id.0),
                u64::from(predicate.polynomial.0),
                match predicate.sense {
                    PredicateSense::StrictNegative => 1,
                    PredicateSense::NonPositive => 2,
                    PredicateSense::EqualZero => 3,
                    PredicateSense::NonNegative => 4,
                    PredicateSense::StrictPositive => 5,
                },
                u64::from(predicate.boundary_family),
            ],
        );
    }
    for bundle in &projective.derivatives.bundles {
        let mut operands = vec![
            u64::from(bundle.id.0),
            u64::from(bundle.feature.0),
            u64::from(bundle.g.0),
            u64::from(bundle.first.u.0),
            u64::from(bundle.first.v.0),
            u64::from(bundle.first.q.0),
            u64::from(bundle.second.uu.0),
            u64::from(bundle.second.uv.0),
            u64::from(bundle.second.uq.0),
            u64::from(bundle.second.vv.0),
            u64::from(bundle.second.vq.0),
            u64::from(bundle.second.qq.0),
            u64::from(bundle.third.uuu.0),
            u64::from(bundle.third.uuv.0),
            u64::from(bundle.third.uuq.0),
            u64::from(bundle.third.uvv.0),
            u64::from(bundle.third.uvq.0),
            u64::from(bundle.third.uqq.0),
            u64::from(bundle.third.vvv.0),
            u64::from(bundle.third.vvq.0),
            u64::from(bundle.third.vqq.0),
            u64::from(bundle.third.qqq.0),
            u64::from(bundle.g_t.0),
            bundle.g_tt.map_or(0, |value| u64::from(value.0) + 1),
            bundle.parameter.len() as u64,
        ];
        for parameter in &bundle.parameter {
            operands.extend([
                u64::from(parameter.parameter.0),
                u64::from(parameter.polynomial.0),
                u64::from(parameter.declared_rate.is_some()),
            ]);
            if let Some((first, second)) = parameter.declared_rate {
                operands.extend([first, second]);
            }
        }
        operands.push(bundle.influencing_params.len() as u64);
        operands.extend(
            bundle
                .influencing_params
                .iter()
                .map(|param| u64::from(param.0)),
        );
        push_record(fixed, 23, 0, operands);
    }
    for cluster in &projective.derivatives.clusters {
        let tube = &cluster.root_tube;
        let mut operands = vec![
            u64::from(cluster.object.0),
            cluster.leaf_signature.len() as u64,
        ];
        for path in &cluster.leaf_signature {
            operands.push(path.len() as u64);
            for step in path {
                operands.extend([u64::from(step.field.0), u64::from(step.child_slot)]);
            }
        }
        operands.push(cluster.bundles.len() as u64);
        operands.extend(cluster.bundles.iter().map(|value| u64::from(value.0)));
        operands.extend([
            u64::from(tube.scalar_root.0),
            tube.scalar_derivative_sources.len() as u64,
        ]);
        operands.extend(
            tube.scalar_derivative_sources
                .iter()
                .map(|value| u64::from(value.0)),
        );
        operands.extend([
            tube.value_domain.lo.to_bits(),
            tube.value_domain.hi.to_bits(),
            tube.first_world_abs[0].to_bits(),
            tube.first_world_abs[1].to_bits(),
            tube.first_world_abs[2].to_bits(),
            tube.second_world_abs.to_bits(),
            tube.third_world_abs.to_bits(),
            tube.parameter_abs.len() as u64,
        ]);
        for (parameter, bound) in &tube.parameter_abs {
            operands.extend([u64::from(parameter.0), bound.to_bits()]);
        }
        operands.extend([
            tube.frame_delta_abs.map_or(0, f64::to_bits),
            tube.frame_second_delta_abs.map_or(0, f64::to_bits),
            u64::from(tube.taylor_order),
            u64::from(tube.subdivision_depth),
            tube.world_delta_abs_bound.to_bits(),
            tube.remainder.to_bits(),
            u64::from(tube.maximum_predictor_roots),
            u64::from(tube.maximum_object_roots),
            u64::from(tube.requires_boundary_events),
        ]);
        push_record(fixed, 24, 0, operands);
    }
    for deformation in &projective.deformations {
        let phase = &deformation.phase_recurrence;
        let mut operands = vec![
            u64::from(deformation.feature.0),
            u64::from(deformation.deformation_field.0),
            u64::from(deformation.predictor.0),
            u64::from(deformation.residual.0),
            u64::from(deformation.coordinate_x.0),
            deformation.frequency.lo.to_bits(),
            deformation.frequency.hi.to_bits(),
            deformation.phase.lo.to_bits(),
            deformation.phase.hi.to_bits(),
            deformation.value_bound.to_bits(),
            deformation.first_derivative_bound.to_bits(),
            deformation.second_derivative_bound.to_bits(),
            deformation.third_derivative_bound.to_bits(),
            u64::from(deformation.taylor_order),
            u64::from(deformation.approximation.revision),
            deformation.approximation.folded_domain.lo.to_bits(),
            deformation.approximation.folded_domain.hi.to_bits(),
            deformation.approximation.sine_remainder.to_bits(),
            deformation.approximation.cosine_remainder.to_bits(),
            match deformation.tube_method {
                "monotone-krawczyk" => 1,
                _ => 0,
            },
            u64::from(deformation.maximum_root_count),
            u64::from(phase.coordinate_x.0),
            u64::from(phase.frequency_scalar.0),
            u64::from(phase.phase_scalar.0),
            phase.frequency.lo.to_bits(),
            phase.frequency.hi.to_bits(),
            phase.phase.lo.to_bits(),
            phase.phase.hi.to_bits(),
        ];
        operands.extend(phase.sine_coefficients);
        operands.extend(phase.cosine_coefficients);
        push_record(fixed, 25, 0, operands);
    }
    for repeat in &structural.repeats {
        let mut operands = vec![
            u64::from(repeat.object.0),
            u64::from(repeat.source_root.0),
            u64::from(repeat.instance_count),
            u64::from(repeat.affine_translation_count),
            u64::from(repeat.wrap_event_families),
            u64::from(repeat.certificate_must_fix_instance),
            repeat.instances.len() as u64,
        ];
        for instance in &repeat.instances {
            operands.extend([
                u64::from(instance.object.0),
                instance.translations.len() as u64,
            ]);
            for translation in &instance.translations {
                operands.extend([
                    u64::from(translation.repeat_field.0),
                    axis_tag(translation.axis),
                    translation.first as i64 as u64,
                    translation.index as i64 as u64,
                    translation.period.lo.to_bits(),
                    translation.period.hi.to_bits(),
                    translation.translation.lo.to_bits(),
                    translation.translation.hi.to_bits(),
                ]);
            }
        }
        operands.push(repeat.wrap_events.len() as u64);
        for event in &repeat.wrap_events {
            operands.extend([
                u64::from(event.repeat_field.0),
                axis_tag(event.axis),
                event.left_index as i64 as u64,
                event.right_index as i64 as u64,
                event.boundary.lo.to_bits(),
                event.boundary.hi.to_bits(),
            ]);
        }
        push_record(fixed, 26, 0, operands);
    }
    for deformation in &structural.deformations {
        push_record(
            fixed,
            27,
            0,
            vec![
                u64::from(deformation.field.0),
                u64::from(deformation.displacement.0),
                1,
                deformation.amplitude.to_bits(),
                deformation.gradient.to_bits(),
                deformation.hessian.to_bits(),
                deformation.third_derivative.to_bits(),
                u64::from(deformation.coordinate_x.0),
                u64::from(deformation.frequency_scalar.0),
                u64::from(deformation.phase_scalar.0),
                deformation.frequency.lo.to_bits(),
                deformation.frequency.hi.to_bits(),
                deformation.phase.lo.to_bits(),
                deformation.phase.hi.to_bits(),
            ],
        );
    }
    for event in &structural.material_events {
        let mut operands = vec![
            u64::from(event.predicate.0),
            match event.kind {
                super::material::MaterialEventKind::NominalIdentity => 1,
                super::material::MaterialEventKind::ScalarThreshold => 2,
                super::material::MaterialEventKind::ProceduralBoundary => 3,
            },
            u64::from(event.crossing_bound),
            event.owners.len() as u64,
        ];
        operands.extend(event.owners.iter().map(|object| u64::from(object.0)));
        operands.push(event.feature_owners.len() as u64);
        operands.extend(
            event
                .feature_owners
                .iter()
                .map(|feature| u64::from(feature.0)),
        );
        push_record(fixed, 28, 0, operands);
    }
    for (index_kind, index) in [
        &projective.indexes.tile_features,
        &projective.indexes.tile_events,
        &projective.indexes.tile_competitions,
        &projective.indexes.row_block_repeats,
        &projective.indexes.tile_lights,
        &projective.indexes.tile_probes,
    ]
    .into_iter()
    .enumerate()
    {
        let mut payload = Vec::with_capacity(
            index
                .cells
                .len()
                .checked_mul(2)
                .and_then(|cells| cells.checked_add(index.ids.len()))
                .ok_or_else(|| "P015: local-index wire payload length overflow".to_string())?,
        );
        for cell in &index.cells {
            payload.extend([u64::from(cell.offset), u64::from(cell.count)]);
        }
        payload.extend(index.ids.iter().map(|value| u64::from(*value)));
        const CHUNK_HEADER_OPERANDS: usize = 5;
        let chunk_payload =
            wrela_machine::pixels::FRAME_PROGRAM_MAX_OPERANDS_V1 - CHUNK_HEADER_OPERANDS;
        let chunk_count = payload.len().div_ceil(chunk_payload);
        push_record(
            fixed,
            30,
            0,
            vec![
                0,
                u64::try_from(index_kind)
                    .map_err(|_| "P015: local-index kind exceeds u64".to_string())?,
                u64::try_from(index.cells.len())
                    .map_err(|_| "P015: local-index cell count exceeds u64".to_string())?,
                u64::try_from(index.ids.len())
                    .map_err(|_| "P015: local-index ID count exceeds u64".to_string())?,
                u64::try_from(chunk_count)
                    .map_err(|_| "P015: local-index chunk count exceeds u64".to_string())?,
            ],
        );
        for (chunk_index, chunk) in payload.chunks(chunk_payload).enumerate() {
            let offset = chunk_index
                .checked_mul(chunk_payload)
                .ok_or_else(|| "P015: local-index chunk offset overflow".to_string())?;
            let mut operands = vec![
                1,
                u64::try_from(index_kind)
                    .map_err(|_| "P015: local-index kind exceeds u64".to_string())?,
                u64::try_from(chunk_index)
                    .map_err(|_| "P015: local-index chunk index exceeds u64".to_string())?,
                u64::try_from(offset)
                    .map_err(|_| "P015: local-index chunk offset exceeds u64".to_string())?,
                u64::try_from(chunk.len())
                    .map_err(|_| "P015: local-index chunk length exceeds u64".to_string())?,
            ];
            operands.extend_from_slice(chunk);
            push_record(fixed, 30, 0, operands);
        }
    }
    for range in &projective.indexes.object_features {
        push_record(
            fixed,
            31,
            0,
            vec![
                u64::from(range.object.0),
                u64::from(range.first.0),
                u64::from(range.count),
            ],
        );
    }
    for (feature, bundle) in projective.indexes.feature_derivatives.iter().enumerate() {
        push_record(
            fixed,
            32,
            0,
            vec![
                u64::try_from(feature).map_err(|_| "P015: feature derivative index exceeds u64")?,
                u64::from(bundle.0),
            ],
        );
    }
    for material in &projective.indexes.material_programs {
        push_record(
            fixed,
            33,
            0,
            vec![
                u64::from(material.identity_set),
                u64::from(material.material.0),
            ],
        );
    }
    push_record(
        fixed,
        34,
        0,
        vec![
            u64::from(projective.indexes.tiles_x),
            u64::from(projective.indexes.tiles_y),
            projective.indexes.bytes,
        ],
    );
    for pair in &projective.competitions.pairs {
        push_record(
            fixed,
            35,
            0,
            vec![
                u64::from(pair.id.0),
                u64::from(pair.a.0),
                u64::from(pair.b.0),
                u64::from(pair.event.0),
                u64::from(pair.pixels.x.start),
                u64::from(pair.pixels.x.end),
                u64::from(pair.pixels.y.start),
                u64::from(pair.pixels.y.end),
                u64::from(pair.tiles.x.start),
                u64::from(pair.tiles.x.end),
                u64::from(pair.tiles.y.start),
                u64::from(pair.tiles.y.end),
                pair.q_overlap.lo.to_bits(),
                pair.q_overlap.hi.to_bits(),
            ],
        );
    }
    // Event and competition ledgers are compiler-only completeness audits.
    // Every emitted runtime subject is represented above, and every omission
    // is represented by a numeric exclusion record and proof payload.

    let camera = by_kind
        .get_mut(&FrameProgramTableKindV1::CameraLightPost)
        .expect("namespace seeded");
    let mut operands = vec![
        config.width.into(),
        config.height.into(),
        config.refresh_hz.into(),
        config.shade_hz.into(),
        config.near.to_bits(),
        config.far.to_bits(),
        config.camera_max_motion.to_bits().into(),
        config.light_capacity.into(),
    ];
    for slot in 0..8 {
        let kind = config
            .light_kinds
            .get(slot)
            .map(String::as_str)
            .unwrap_or("Disabled");
        operands.push(super::config::light_kind_tag(kind).ok_or_else(|| {
            format!("pixels::program: sealed renderer has unknown light kind `{kind}`")
        })?);
    }
    operands.extend([
        config.exposure.min.to_bits().into(),
        config.exposure.max.to_bits().into(),
        u64::from(config.ao_enabled),
        u64::from(config.ao_radius.to_bits()),
        u64::from(config.ao_strength.to_bits()),
        u64::from(config.probes_enabled),
        config.probe_initialization_worst_case_ms.into(),
        config.initialization_deadline_ms.into(),
    ]);
    let tone_lut =
        super::reference::display::sealed_tone_lut(&config.tone_curve).ok_or_else(|| {
            format!(
                "P022: unsupported sealed tone curve `{}`",
                config.tone_curve
            )
        })?;
    super::reference::display::validate_monotone_lut(tone_lut)
        .map_err(|_| "P018: tone or transfer table is not monotone: tone LUT".to_string())?;
    super::tables::verify_all()?;
    super::reference::display::validate_monotone_lut(super::reference::display::srgb_transfer_lut())
        .map_err(|_| "P018: tone or transfer table is not monotone: sRGB transfer LUT".to_string())?;
    operands.extend([
        match config.tone_curve.as_str() {
            "Linear" => 0,
            "FilmicV1" => 1,
            _ => unreachable!("sealed tone curve checked above"),
        },
        tone_lut.len() as u64,
        1,
        super::reference::display::srgb_transfer_lut().len() as u64,
        (-16_i64) as u64,
        16,
        1,
        1,
    ]);
    for value in [
        config.world_min.x,
        config.world_min.y,
        config.world_min.z,
        config.world_max.x,
        config.world_max.y,
        config.world_max.z,
    ]
    .into_iter()
    .chain(config.environment.min)
    .chain(config.environment.max)
    {
        operands.push(value.to_bits().into());
    }
    push_record(camera, 1, 0, operands);

    let flags = u32::from(
        graph
            .materials
            .iter()
            .any(|(_, node)| matches!(node.kind, super::material_graph::MaterialKind::Sample(_))),
    );
    Ok(FrameProgram {
        renderer_index,
        flags,
        numeric_revision: super::version::FRAME_PROGRAM_NUMERIC_REVISION_V1,
        formal_revision: super::version::FRAME_PROGRAM_FORMAL_REVISION_V1,
        tables: FrameProgramTableKindV1::ALL
            .into_iter()
            .map(|kind| FrameTable {
                kind,
                records: by_kind.remove(&kind).expect("namespace seeded"),
            })
            .collect(),
    })
}

pub(crate) fn minimal_verified_frame_program() -> VerifiedFrameProgram {
    fn records(tables: &mut [FrameTable], kind: FrameProgramTableKindV1) -> &mut Vec<FrameRecord> {
        &mut tables
            .iter_mut()
            .find(|table| table.kind == kind)
            .expect("canonical table namespace")
            .records
    }

    let mut tables = FrameProgramTableKindV1::ALL
        .into_iter()
        .map(|kind| FrameTable {
            kind,
            records: Vec::new(),
        })
        .collect::<Vec<_>>();
    records(&mut tables, FrameProgramTableKindV1::Scalar).push(FrameRecord {
        stable_id: 0,
        tag: 1,
        flags: 0,
        operands: vec![0, 0.0_f32.to_bits().into()],
    });
    records(&mut tables, FrameProgramTableKindV1::Field).push(FrameRecord {
        stable_id: 0,
        tag: 1,
        flags: 0,
        operands: vec![0, 0, 0, 0, 0],
    });
    records(&mut tables, FrameProgramTableKindV1::Object).push(FrameRecord {
        stable_id: 0,
        tag: 1,
        flags: 0,
        operands: vec![
            0,
            0,
            0,
            0,
            1,
            0,
            0.0_f64.to_bits(),
            0.0_f64.to_bits(),
            0.0_f64.to_bits(),
            0.0_f64.to_bits(),
            0.0_f64.to_bits(),
            0.0_f64.to_bits(),
            0.0_f64.to_bits(),
            0.0_f64.to_bits(),
            1,
            0,
            0,
        ],
    });
    let mut feature_operands = vec![0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0];
    feature_operands[16] = 0.25_f64.to_bits();
    feature_operands[17] = 4.0_f64.to_bits();
    feature_operands.extend([
        0, // validity predicates
        1, // outward orientation
        1, // affine q seed
        0,
        1.0_f64.to_bits(),
        1.0_f64.to_bits(),
        2, // positive denominator
        1, // affine root isolation
        6, // composition payload
        2,
        0,
        0,
        0,
        0,
        0,
        0, // not deformed
        0, // influencing parameters
        1, // occurrence path
        0, // no shared boundary
        0,
        0,
    ]);
    feature_operands.extend([0.0_f64.to_bits(); 6]);
    records(&mut tables, FrameProgramTableKindV1::Feature).push(FrameRecord {
        stable_id: 0,
        tag: 1,
        flags: 0,
        operands: feature_operands,
    });
    records(&mut tables, FrameProgramTableKindV1::Csg).push(FrameRecord {
        stable_id: 0,
        tag: 5,
        flags: 0,
        operands: Vec::new(),
    });
    records(&mut tables, FrameProgramTableKindV1::FixedDomain).push(FrameRecord {
        stable_id: 0,
        tag: 1,
        flags: 0,
        operands: vec![1; 31],
    });
    let near = 0.1_f64;
    let far = 128.0_f64;
    let policy = wrela_machine::pixels::derive_fixed_q_policy_v1(near, far)
        .expect("minimal camera has a fixed-q policy");
    records(&mut tables, FrameProgramTableKindV1::FixedDomain).push(FrameRecord {
        stable_id: 1,
        tag: 5,
        flags: 0,
        operands: vec![
            policy.exponent as i64 as u64,
            policy.maximum_raw as u64,
            u64::from(policy.reset_width),
            policy.error_radius as u64,
        ],
    });
    for index_kind in 0..6 {
        let fixed = records(&mut tables, FrameProgramTableKindV1::FixedDomain);
        fixed.push(FrameRecord {
            stable_id: fixed.len() as u32,
            tag: 30,
            flags: 0,
            operands: vec![0, index_kind, 0, 0, 0],
        });
    }
    let mut camera = vec![
        64,
        32,
        60,
        60,
        near.to_bits(),
        far.to_bits(),
        u64::from(0.0_f32.to_bits()),
        0,
    ];
    camera.extend([0; 8]);
    camera.extend([
        u64::from(0.0_f32.to_bits()),
        u64::from(0.0_f32.to_bits()),
        0,
        u64::from(1.0_f32.to_bits()),
        u64::from(1.0_f32.to_bits()),
        0,
        0,
        1,
        0,
        17,
        1,
        super::tables::TRANSFER_TABLE_ENTRIES_V1 as u64,
        (-16_i64) as u64,
        16,
        1,
        1,
    ]);
    camera.extend(
        [0.0_f32; 6]
            .into_iter()
            .chain([0.0_f32, 0.0, 0.0, 1.0, 1.0, 1.0])
            .map(|value| u64::from(value.to_bits())),
    );
    records(&mut tables, FrameProgramTableKindV1::CameraLightPost).push(FrameRecord {
        stable_id: 0,
        tag: 1,
        flags: 0,
        operands: camera,
    });
    for slot in 0..8_u64 {
        let summaries = records(&mut tables, FrameProgramTableKindV1::ShadingSummary);
        summaries.push(FrameRecord {
            stable_id: summaries.len() as u32,
            tag: 6,
            flags: 0,
            operands: vec![
                slot,
                u64::from((-65504.0_f32).to_bits()),
                u64::from((-65504.0_f32).to_bits()),
                u64::from((-65504.0_f32).to_bits()),
                u64::from(65504.0_f32.to_bits()),
                u64::from(65504.0_f32.to_bits()),
                u64::from(65504.0_f32.to_bits()),
                u64::from(65504.0_f32.to_bits()),
                u64::from(65504.0_f32.to_bits()),
                u64::from(65504.0_f32.to_bits()),
                u64::from(65504.0_f32.to_bits()),
                u64::from(65504.0_f32.to_bits()),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        });
    }
    super::verify::check_program(FrameProgram {
        renderer_index: 0,
        flags: 0,
        numeric_revision: super::version::FRAME_PROGRAM_NUMERIC_REVISION_V1,
        formal_revision: super::version::FRAME_PROGRAM_FORMAL_REVISION_V1,
        tables,
    })
    .unwrap()
}
