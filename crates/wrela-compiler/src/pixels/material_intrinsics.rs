//! Closed material-constructor classification.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialIntrinsic {
    Standard,
    Clay,
    Porcelain,
    Textured,
}

pub fn classify(member: &str) -> Option<MaterialIntrinsic> {
    Some(match member {
        "standard" => MaterialIntrinsic::Standard,
        "clay" => MaterialIntrinsic::Clay,
        "porcelain" => MaterialIntrinsic::Porcelain,
        "textured" => MaterialIntrinsic::Textured,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_constructors_are_explicit_and_unknown_ones_fail_closed() {
        assert_eq!(classify("standard"), Some(MaterialIntrinsic::Standard));
        assert_eq!(classify("clay"), Some(MaterialIntrinsic::Clay));
        assert_eq!(classify("porcelain"), Some(MaterialIntrinsic::Porcelain));
        assert_eq!(classify("textured"), Some(MaterialIntrinsic::Textured));
        assert_eq!(classify("noise_texture"), None);
    }
}
