//! Antigravity built-in tool names a `runtime: antigravity` agent may declare.
//!
//! Closed set: each name is a canonical `agy_acp_server` built-in that Rho
//! sends in the session's `_meta.agy.enabledTools` allowlist and classifies for
//! permission answers (see `antigravity_runtime::permissions`). Antigravity
//! enables every built-in by default, so a nonempty list is mandatory and
//! unknown names are rejected at parse time.
//!
//! Deliberately absent (agy_acp_server 1.3.0):
//! - `ask_question` (no headless answer path)
//! - `start_subagent` (nested agents outside Rho's run)
//! - `schedule` (timers that outlive the run)
//! - `generate_image`, `finish`
//! - `list_directory`, `search_directory`, `find_file` (disabled server-side
//!   by default; allowlisting them produced an unstable tool surface)

use std::{fmt, str::FromStr};

use rho_sdk::CapabilityKind;
use thiserror::Error;

/// Antigravity built-ins Rho has classified for fencing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AntigravityTool {
    ViewFile,
    CreateFile,
    EditFile,
    RunCommand,
    SearchWeb,
    ReadUrlContent,
}

impl AntigravityTool {
    pub const ALL: &[AntigravityTool] = &[
        Self::ViewFile,
        Self::CreateFile,
        Self::EditFile,
        Self::RunCommand,
        Self::SearchWeb,
        Self::ReadUrlContent,
    ];

    /// Canonical built-in name, as written in agent definitions and sent in
    /// `enabledTools`.
    pub fn as_name(self) -> &'static str {
        match self {
            Self::ViewFile => "view_file",
            Self::CreateFile => "create_file",
            Self::EditFile => "edit_file",
            Self::RunCommand => "run_command",
            Self::SearchWeb => "search_web",
            Self::ReadUrlContent => "read_url_content",
        }
    }

    /// One-line description for picker detail text.
    pub fn detail(self) -> &'static str {
        match self {
            Self::ViewFile => "Read a file.",
            Self::CreateFile => "Create a file.",
            Self::EditFile => "Edit a file.",
            Self::RunCommand => "Run a shell command.",
            Self::SearchWeb => "Search the web.",
            Self::ReadUrlContent => "Fetch a URL.",
        }
    }

    pub fn capability_kind(self) -> CapabilityKind {
        match self {
            Self::ViewFile => CapabilityKind::Read,
            Self::CreateFile | Self::EditFile => CapabilityKind::Write,
            Self::RunCommand => CapabilityKind::Process,
            Self::SearchWeb | Self::ReadUrlContent => CapabilityKind::Network,
        }
    }

    pub fn is_read_only(self) -> bool {
        matches!(self.capability_kind(), CapabilityKind::Read)
    }

    fn accepted_names() -> String {
        Self::ALL
            .iter()
            .map(|tool| tool.as_name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for AntigravityTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_name())
    }
}

impl FromStr for AntigravityTool {
    type Err = AntigravityToolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|tool| tool.as_name() == value)
            .ok_or_else(|| AntigravityToolError {
                value: value.to_string(),
                expected: Self::accepted_names(),
            })
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("unknown Antigravity tool '{value}'; expected one of: {expected}")]
pub struct AntigravityToolError {
    value: String,
    expected: String,
}
