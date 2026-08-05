//! Dense, stable IDs used by the symbolic Pixels compiler.

use std::fmt;

macro_rules! dense_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            pub(crate) fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

dense_id!(ScalarId, "s");
dense_id!(FieldId, "f");
dense_id!(ObjectId, "o");
dense_id!(FeatureId, "g");
dense_id!(MaterialId, "m");
dense_id!(ParamId, "p");
dense_id!(EventTemplateId, "e");
dense_id!(CoeffId, "c");
dense_id!(PolyProgramId, "poly");
dense_id!(RationalProgramId, "rat");
dense_id!(PredicateProgramId, "pred");
dense_id!(DerivativeBundleId, "d");
dense_id!(CompetitionPairId, "pair");
dense_id!(ExclusionId, "x");
dense_id!(DomainId, "domain");
dense_id!(ProofRecordId, "proof");

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProgramRendererId(pub u16);

impl fmt::Display for ProgramRendererId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

pub trait DenseId: Copy {
    fn from_index(index: usize) -> Result<Self, String>;
    fn index(self) -> usize;
}

macro_rules! dense_impl {
    ($name:ident) => {
        impl DenseId for $name {
            fn from_index(index: usize) -> Result<Self, String> {
                u32::try_from(index)
                    .map(Self)
                    .map_err(|_| concat!("pixels::arena: ", stringify!($name), " overflow").into())
            }

            fn index(self) -> usize {
                self.index()
            }
        }
    };
}

dense_impl!(ScalarId);
dense_impl!(FieldId);
dense_impl!(ObjectId);
dense_impl!(FeatureId);
dense_impl!(MaterialId);
dense_impl!(ParamId);
dense_impl!(EventTemplateId);
dense_impl!(CoeffId);
dense_impl!(PolyProgramId);
dense_impl!(RationalProgramId);
dense_impl!(PredicateProgramId);
dense_impl!(DerivativeBundleId);
dense_impl!(CompetitionPairId);
dense_impl!(ExclusionId);
dense_impl!(DomainId);
dense_impl!(ProofRecordId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_the_normative_stable_spelling() {
        assert_eq!(ScalarId(12).to_string(), "s12");
        assert_eq!(FieldId(7).to_string(), "f7");
        assert_eq!(ObjectId(3).to_string(), "o3");
        assert_eq!(FeatureId(4).to_string(), "g4");
        assert_eq!(MaterialId(5).to_string(), "m5");
        assert_eq!(ParamId(6).to_string(), "p6");
        assert_eq!(EventTemplateId(8).to_string(), "e8");
        assert_eq!(CoeffId(10).to_string(), "c10");
        assert_eq!(PolyProgramId(11).to_string(), "poly11");
        assert_eq!(RationalProgramId(12).to_string(), "rat12");
        assert_eq!(PredicateProgramId(12).to_string(), "pred12");
        assert_eq!(DerivativeBundleId(13).to_string(), "d13");
        assert_eq!(CompetitionPairId(14).to_string(), "pair14");
        assert_eq!(ExclusionId(15).to_string(), "x15");
        assert_eq!(DomainId(16).to_string(), "domain16");
        assert_eq!(ProofRecordId(17).to_string(), "proof17");
        assert_eq!(ProgramRendererId(9).to_string(), "r9");
    }
}
