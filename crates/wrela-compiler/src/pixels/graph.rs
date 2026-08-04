//! Structural field graph produced by symbolic evaluation.

use super::arena::Arena;
use super::ids::{FieldId, ScalarId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalIdentity {
    pub enum_key: String,
    pub variant: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Primitive {
    Plane {
        normal: [ScalarId; 3],
        offset: ScalarId,
    },
    Sphere {
        center: [ScalarId; 3],
        radius: ScalarId,
    },
    Box {
        center: [ScalarId; 3],
        half: [ScalarId; 3],
    },
    RoundBox {
        center: [ScalarId; 3],
        half: [ScalarId; 3],
        radius: ScalarId,
    },
    Capsule {
        a: [ScalarId; 3],
        b: [ScalarId; 3],
        radius: ScalarId,
    },
    FiniteCylinder {
        a: [ScalarId; 3],
        b: [ScalarId; 3],
        radius: ScalarId,
    },
    FiniteCone {
        a: [ScalarId; 3],
        b: [ScalarId; 3],
        radius_a: ScalarId,
        radius_b: ScalarId,
    },
    Torus {
        center: [ScalarId; 3],
        axis: [ScalarId; 3],
        major: ScalarId,
        minor: ScalarId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransformProgram {
    Translate {
        by: [ScalarId; 3],
    },
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
    /// Multiplies the child's signed-distance value. It is deliberately not a
    /// coordinate transform: field.wr preserves the zero set and scales only
    /// the returned distance. Later bounds/codegen passes must follow the
    /// node's authoritative scalar tape rather than evaluate `child(p/scale)`.
    UniformScale {
        scale: ScalarId,
    },
    /// Pre-canonicalization representation of adjacent source rigid
    /// transforms. `composed` is a cached geometric summary, not an
    /// evaluation-equivalent replacement: source-order f32 rounding is
    /// authoritative.
    SourceRigidSequence {
        steps: Vec<TransformProgram>,
        composed: Box<TransformProgram>,
    },
    /// Canonical representation of a source rigid-transform sequence. The
    /// ordered steps remain authoritative so canonicalization cannot change
    /// f32 rounding. `composed` is retained only for later geometric analyses.
    RigidSequence {
        steps: Vec<TransformProgram>,
        composed: Box<TransformProgram>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClosedDeformDerivation {
    SinusoidalX,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DerivedDeformContract {
    pub amplitude_bound: ScalarId,
    pub gradient_bound: ScalarId,
    pub hessian_bound: ScalarId,
    pub third_derivative_bound: ScalarId,
    pub derivation: ClosedDeformDerivation,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldKind {
    Primitive(Primitive),
    HardUnion {
        a: FieldId,
        b: FieldId,
    },
    HardIntersection {
        a: FieldId,
        b: FieldId,
    },
    HardSubtract {
        a: FieldId,
        b: FieldId,
    },
    SmoothUnion {
        a: FieldId,
        b: FieldId,
        k: ScalarId,
    },
    SmoothIntersection {
        a: FieldId,
        b: FieldId,
        k: ScalarId,
    },
    SmoothSubtract {
        a: FieldId,
        b: FieldId,
        k: ScalarId,
    },
    Neg {
        child: FieldId,
    },
    Transform {
        child: FieldId,
        transform: TransformProgram,
    },
    FiniteRepeat {
        child: FieldId,
        axis: Axis,
        first: i32,
        count: u32,
        period: ScalarId,
    },
    BoundedDisplace {
        base: FieldId,
        displacement: ScalarId,
        contract: DerivedDeformContract,
    },
    Mark {
        child: FieldId,
        object_source: CanonicalIdentity,
        material_source: CanonicalIdentity,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldNode {
    pub kind: FieldKind,
    pub scalar_value: ScalarId,
}

pub type FieldArena = Arena<FieldId, FieldNode>;
