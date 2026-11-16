use parameters_core::Diagnostic;
use parameters_lang::TextDiagnostic;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TextRange>,
}

impl Problem {
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl From<&Diagnostic> for Problem {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            code: diagnostic.code.id().to_owned(),
            severity: if diagnostic.is_error() {
                Severity::Error
            } else {
                Severity::Warning
            },
            message: diagnostic.message.clone(),
            path: diagnostic.location.path.to_string(),
            suggestion: diagnostic.suggestion.clone(),
            range: None,
        }
    }
}

impl From<&TextDiagnostic> for Problem {
    fn from(diagnostic: &TextDiagnostic) -> Self {
        Self {
            range: Some(TextRange {
                start: TextPosition {
                    line: diagnostic.start.line,
                    column: diagnostic.start.column,
                },
                end: TextPosition {
                    line: diagnostic.end.line,
                    column: diagnostic.end.column,
                },
            }),
            ..Self::from(&diagnostic.diagnostic)
        }
    }
}
