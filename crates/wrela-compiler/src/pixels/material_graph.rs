//! Structural material graph.

use super::arena::Arena;
use super::graph::CanonicalIdentity;
use super::ids::{MaterialId, ScalarId};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NormalModel {
    Geometric,
    AnalyticSlope { x: ScalarId, y: ScalarId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TextureFilterV1 {
    Nearest,
    Bilinear,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImmutableTexture {
    pub asset: String,
    pub stable_id: u32,
    pub width: u32,
    pub height: u32,
    pub filter: TextureFilterV1,
    pub content_digest: String,
    pub filter_error_min_bits: u32,
    pub filter_error_max_bits: u32,
}

const CHECKER_2X2_V1_RGBA8: &[u8] = &[
    0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
];

pub fn compiler_texture(
    stable_id: u32,
    filter: TextureFilterV1,
) -> Result<ImmutableTexture, String> {
    let (asset, width, height, bytes) = match stable_id {
        19 => ("Checker2x2V1", 2, 2, CHECKER_2X2_V1_RGBA8),
        other => {
            return Err(format!(
                "P004: field operation `texture_lookup` is not available in `AaaByteExact`: unknown compiler-owned texture asset id `{other}`"
            ));
        }
    };
    Ok(ImmutableTexture {
        asset: asset.to_string(),
        stable_id,
        width,
        height,
        filter,
        content_digest: wrela_machine::sha256::sha256_hex(bytes),
        // P3 carries the finite filter contribution without pretending P9's
        // sampler/mip proof already exists. Every normalized channel lies in
        // [0,1], so this interval is conservative for either admitted filter.
        filter_error_min_bits: 0.0_f32.to_bits(),
        filter_error_max_bits: 1.0_f32.to_bits(),
    })
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
    pub pattern: Option<ImmutableTexture>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_texture_metadata_and_content_are_compiler_owned() {
        let texture = compiler_texture(19, TextureFilterV1::Bilinear).unwrap();
        assert_eq!(texture.asset, "Checker2x2V1");
        assert_eq!((texture.width, texture.height), (2, 2));
        assert_eq!(
            texture.content_digest,
            wrela_machine::sha256::sha256_hex(CHECKER_2X2_V1_RGBA8)
        );
        assert!(compiler_texture(20, TextureFilterV1::Nearest).is_err());
    }
}
