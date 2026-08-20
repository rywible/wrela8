//! Structural material graph.

use super::arena::Arena;
use super::graph::CanonicalIdentity;
use super::ids::{MaterialId, ScalarId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TextureFilterV1 {
    Nearest,
    Bilinear,
    Trilinear,
    Anisotropic4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UvSourceV1 {
    Plane,
    Sphere,
    Cylinder,
    Torus,
    BoxFeature,
    RoundBoxFeature,
    ObjectTriplanar,
    WorldTriplanar,
}

impl UvSourceV1 {
    pub const fn tag(self) -> u64 {
        match self {
            Self::Plane => 1,
            Self::Sphere => 2,
            Self::Cylinder => 3,
            Self::Torus => 4,
            Self::BoxFeature => 5,
            Self::RoundBoxFeature => 6,
            Self::ObjectTriplanar => 7,
            Self::WorldTriplanar => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImmutableTexture {
    pub asset: String,
    pub stable_id: u32,
    pub format_tag: u64,
    pub width: u32,
    pub height: u32,
    pub filter: TextureFilterV1,
    pub uv_source: UvSourceV1,
    pub content_digest: String,
    pub filter_error_min_bits: u32,
    pub filter_error_max_bits: u32,
}

pub fn compiler_texture(
    stable_id: u32,
    filter: TextureFilterV1,
    uv_source: UvSourceV1,
) -> Result<ImmutableTexture, String> {
    if filter == TextureFilterV1::Nearest {
        return Err(
            "P004: texture filter `Nearest` is not available in `AaaByteExact`; use Bilinear, Trilinear, or Anisotropic4"
                .to_string(),
        );
    }
    let compiled = super::texture::compiler_asset(stable_id).map_err(|_| {
        format!(
            "P004: field operation `texture_lookup` is not available in `AaaByteExact`: unknown compiler-owned texture asset id `{stable_id}`"
        )
    })?;
    Ok(ImmutableTexture {
        asset: compiled.name.to_string(),
        stable_id,
        format_tag: compiled.format.tag(),
        width: compiled.width,
        height: compiled.height,
        filter,
        uv_source,
        content_digest: compiled.digest,
        filter_error_min_bits: 0.0_f32.to_bits(),
        filter_error_max_bits: 1.0_f32.to_bits(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NormalModel {
    Geometric,
    AnalyticSlope { x: ScalarId, y: ScalarId },
    TextureSlope { texture: ImmutableTexture },
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
        let texture =
            compiler_texture(19, TextureFilterV1::Bilinear, UvSourceV1::WorldTriplanar).unwrap();
        assert_eq!(texture.asset, "Checker2x2V1");
        assert_eq!((texture.width, texture.height), (2, 2));
        assert_eq!(
            texture.content_digest,
            super::super::texture::compiler_asset(19).unwrap().digest
        );
        assert_eq!(
            compiler_texture(20, TextureFilterV1::Trilinear, UvSourceV1::Plane)
                .unwrap()
                .asset,
            "LinearData2x2V1"
        );
        assert_eq!(
            compiler_texture(20, TextureFilterV1::Nearest, UvSourceV1::WorldTriplanar),
            Err(
                "P004: texture filter `Nearest` is not available in `AaaByteExact`; use Bilinear, Trilinear, or Anisotropic4"
                    .to_string()
            )
        );
        assert!(
            compiler_texture(23, TextureFilterV1::Nearest, UvSourceV1::WorldTriplanar).is_err()
        );
    }
}
