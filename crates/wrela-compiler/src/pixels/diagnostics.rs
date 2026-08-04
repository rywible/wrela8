//! Stable diagnostic values shared by Pixels compiler stages.

use std::fmt;

use crate::syntax::ast::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelsDiagnostic {
    pub code: &'static str,
    pub message: String,
    pub primary: Span,
    pub notes: Vec<String>,
    pub help: Vec<String>,
}

impl PixelsDiagnostic {
    pub fn new(code: &'static str, message: impl Into<String>, primary: Span) -> Self {
        Self {
            code,
            message: message.into(),
            primary,
            notes: Vec::new(),
            help: Vec::new(),
        }
    }

    pub fn from_prefixed(
        message: impl Into<String>,
        primary: Span,
        fallback_code: &'static str,
    ) -> Self {
        let message = message.into();
        for code in PIXELS_CODES {
            if let Some(message) = message
                .strip_prefix(code)
                .and_then(|rest| rest.strip_prefix(": "))
            {
                return Self::with_guidance(code, message, primary);
            }
        }
        Self::with_guidance(fallback_code, &message, primary)
    }

    pub fn from_code(code: &'static str, message: &str, primary: Span) -> Self {
        Self::with_guidance(code, message, primary)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message, Span::default())
    }

    fn with_guidance(code: &'static str, message: &str, primary: Span) -> Self {
        let mut lines = message.lines();
        let mut diagnostic = Self::new(code, lines.next().unwrap_or_default(), primary);
        diagnostic.notes.extend(
            lines
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string),
        );
        match code {
            "P004" => diagnostic.help.push(
                "use only the closed AaaByteExact field-operation and rigid-transform surface"
                    .to_string(),
            ),
            "P015" => diagnostic.help.push(
                "reduce renderer declarations, generated actors, or the sealed capacity request"
                    .to_string(),
            ),
            _ => {}
        }
        diagnostic
    }

    pub fn category(&self) -> &'static str {
        match self.code {
            "P001" => "pixels P001",
            "P002" => "pixels P002",
            "P003" => "pixels P003",
            "P004" => "pixels P004",
            "P005" => "pixels P005",
            "P006" => "pixels P006",
            "P007" => "pixels P007",
            "P008" => "pixels P008",
            "P009" => "pixels P009",
            "P010" => "pixels P010",
            "P011" => "pixels P011",
            "P012" => "pixels P012",
            "P013" => "pixels P013",
            "P014" => "pixels P014",
            "P015" => "pixels P015",
            "P016" => "pixels P016",
            "P017" => "pixels P017",
            "P018" => "pixels P018",
            "P019" => "pixels P019",
            "P020" => "pixels P020",
            "P021" => "pixels P021",
            "P022" => "pixels P022",
            "P023" => "pixels P023",
            "P024" => "pixels P024",
            "P025" => "pixels P025",
            "internal" => "internal",
            _ => "pixels",
        }
    }
}

const PIXELS_CODES: &[&str] = &[
    "P001", "P002", "P003", "P004", "P005", "P006", "P007", "P008", "P009", "P010", "P011", "P012",
    "P013", "P014", "P015", "P016", "P017", "P018", "P019", "P020", "P021", "P022", "P023", "P024",
    "P025",
];

impl fmt::Display for PixelsDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PixelsError {
    Diagnostic(PixelsDiagnostic),
}

impl PixelsError {
    pub fn diagnostic(&self) -> &PixelsDiagnostic {
        match self {
            Self::Diagnostic(diagnostic) => diagnostic,
        }
    }
}

impl fmt::Display for PixelsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic().fmt(f)
    }
}

impl std::error::Error for PixelsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_diagnostic_retains_code_span_notes_and_help_shape() {
        let span = Span {
            line: 4,
            col: 7,
            ..Span::default()
        };
        let diagnostic =
            PixelsDiagnostic::from_prefixed("P010: renderer mode is invalid", span, "P008");
        assert_eq!(diagnostic.code, "P010");
        assert_eq!(diagnostic.primary, span);
        assert_eq!(diagnostic.to_string(), "renderer mode is invalid");
        assert!(diagnostic.notes.is_empty());
        assert!(diagnostic.help.is_empty());
    }

    #[test]
    fn unsupported_operation_preserves_why_chain_and_remediation() {
        let diagnostic = PixelsDiagnostic::from_prefixed(
            "P004: field operation `warp` is not available in `AaaByteExact`\n  renderer call chain: world -> warp",
            Span::default(),
            "P008",
        );
        assert_eq!(diagnostic.code, "P004");
        assert_eq!(
            diagnostic.notes,
            ["renderer call chain: world -> warp".to_string()]
        );
        assert_eq!(diagnostic.help.len(), 1);
    }

    #[test]
    fn internal_stage_failure_is_not_reclassified_as_p020() {
        let diagnostic = PixelsDiagnostic::internal(
            "pixels::support: child budget missing after topological traversal",
        );
        assert_eq!(diagnostic.code, "internal");
        assert_eq!(diagnostic.category(), "internal");
        assert_eq!(
            diagnostic.message,
            "pixels::support: child budget missing after topological traversal"
        );
        assert_eq!(diagnostic.primary, Span::default());
    }
}
