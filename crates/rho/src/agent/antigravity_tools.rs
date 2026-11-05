//! Antigravity built-in tool names a `runtime: antigravity` agent may declare.
//!
//! Closed set: each name is an `agy acp` built-in that Rho writes into the
//! run's agy custom agent `tools:` allowlist (see `antigravity_runtime::agent_file`)
//! and classifies for permission answers (see `antigravity_runtime::fence`).
//! agy offers every built-in to an agent without a list, so a nonempty list is
//! mandatory and unknown names are rejected at parse time (agy 1.5.0 itself
//! fails the run on an unknown name and silently ignores deprecated ones).
//! The `agy_acp_server` names `view_file` and `create_file` still parse, as
//! `read_file` and `write_file`; `read_url_content` has no equivalent.
//!
//! Deliberately absent (agy 1.5.0):
//! - `invoke_subagent`, `define_subagent`, `manage_subagents`, `send_message`
//!   (nested agents outside Rho's run)
//! - `schedule` (timers that outlive the run)
//! - `ask_question` (no headless answer path)
//! - `generate_image`
//! - `manage_task` (agy always adds it)

use std::{fmt, str::FromStr};

use rho_sdk::CapabilityKind;
use thiserror::Error;

/// Antigravity built-ins Rho has classified for fencing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AntigravityTool {
    ReadFile,
    WriteFile,
    EditFile,
    RunCommand,
    SearchWeb,
}

impl AntigravityTool {
    pub const ALL: &[AntigravityTool] = &[
        Self::ReadFile,
        Self::WriteFile,
        Self::EditFile,
        Self::RunCommand,
        Self::SearchWeb,
    ];

    /// agy built-in name, as written in Rho and agy agent definitions.
    pub fn as_name(self) -> &'static str {
        match self {
            Self::ReadFile => "read_file",
            Self::WriteFile => "write_file",
            Self::EditFile => "edit_file",
            Self::RunCommand => "run_command",
            Self::SearchWeb => "search_web",
        }
    }

    /// One-line description for picker detail text.
    pub fn detail(self) -> &'static str {
        match self {
            Self::ReadFile => "Read a file.",
            Self::WriteFile => "Create or overwrite a file.",
            Self::EditFile => "Edit a file.",
            Self::RunCommand => "Run a shell command.",
            Self::SearchWeb => "Search the web.",
        }
    }

    pub fn capability_kind(self) -> CapabilityKind {
        match self {
            Self::ReadFile => CapabilityKind::Read,
            Self::WriteFile | Self::EditFile => CapabilityKind::Write,
            Self::RunCommand => CapabilityKind::Process,
            Self::SearchWeb => CapabilityKind::Network,
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
        // NEXT_MAJOR(rho-coding-agent agent definitions): drop the
        // agy_acp_server aliases `view_file` and `create_file`.
        match value {
            "view_file" => return Ok(Self::ReadFile),
            "create_file" => return Ok(Self::WriteFile),
            _ => {}
        }
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
