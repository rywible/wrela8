//! Structural material graph.

use super::arena::Arena;
use super::graph::CanonicalIdentity;
use super::ids::{MaterialId, ScalarId};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NormalModel {
    Geometric,
    AnalyticSlope { x: ScalarId, y: ScalarId },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MaterialSampleNode {
    pub base_color: [ScalarId; 3],
    pub opacity: ScalarId,
    pub emissive: [ScalarId; 3],
    pub roughness: ScalarId,
    pub metallic: ScalarId,
    pub specular_level: ScalarId,
    pub ior: ScalarId,
    pub normal: NormalModel,
    pub pattern: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaterialKind {
    Sample(MaterialSampleNode),
    Select {
        predicate: ScalarId,
        a: MaterialId,
        b: MaterialId,
    },
    IdentityTable {
        enum_key: String,
        cases: Vec<(CanonicalIdentity, MaterialId)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialNode {
    pub kind: MaterialKind,
}

pub type MaterialArena = Arena<MaterialId, MaterialNode>;
