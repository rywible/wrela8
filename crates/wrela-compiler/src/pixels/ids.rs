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
        assert_eq!(ProgramRendererId(9).to_string(), "r9");
    }
}
