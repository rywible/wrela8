//! Stable diagnostic values shared by Pixels compiler stages.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelsDiagnostic {
    pub code: Option<&'static str>,
    pub message: String,
}

impl PixelsDiagnostic {
    pub fn build(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for PixelsDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "{code}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PixelsError {
    Diagnostic(PixelsDiagnostic),
}

impl fmt::Display for PixelsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostic(diagnostic) => diagnostic.fmt(f),
        }
    }
}

impl std::error::Error for PixelsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncoded_build_diagnostic_has_no_synthetic_code() {
        let diagnostic = PixelsDiagnostic::build("renderer index 0 is unavailable");
        assert_eq!(diagnostic.to_string(), "renderer index 0 is unavailable");
    }
}
