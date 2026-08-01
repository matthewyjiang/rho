use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceSpan {
    pub(crate) label: String,
    pub(crate) line: u32,
    pub(crate) column: u32,
}

impl fmt::Display for SourceSpan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}:{}", self.label, self.line, self.column)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Diagnostic {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) span: Option<SourceSpan>,
}
