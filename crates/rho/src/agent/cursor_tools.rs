//! Cursor Agent tool names accepted on `--allowed-tools`.
//!
//! Closed set: Rho only puts names it has classified on argv, because
//! `cursor-agent -p` is full-power by default and `--exclude-tools` does not
//! fence. An allow list is therefore mandatory, and unknown names never reach
//! the child.
//!
//! Deliberately absent:
//! - `task_tool_call` (nested fan-out)
//! - `ask_question_tool_call` (no headless answer path)
//! - `switch_mode_tool_call` (could leave plan mode)
//! - computer-use, screen, cloud, and PR tools

use std::{fmt, str::FromStr};

use rho_sdk::CapabilityKind;
use thiserror::Error;

/// Cursor Agent tool names accepted on `--allowed-tools`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CursorTool {
    Read,
    Grep,
    Glob,
    Ls,
    SemSearch,
    ReadLints,
    Edit,
    Delete,
    Shell,
    WriteShellStdin,
    WebSearch,
    WebFetch,
    Fetch,
    Mcp,
    ListMcpResources,
    ReadMcpResource,
    UpdateTodos,
    ReadTodos,
    CreatePlan,
    ApplyAgentDiff,
}

impl CursorTool {
    pub const ALL: &[CursorTool] = &[
        Self::Read,
        Self::Grep,
        Self::Glob,
        Self::Ls,
        Self::SemSearch,
        Self::ReadLints,
        Self::Edit,
        Self::Delete,
        Self::Shell,
        Self::WriteShellStdin,
        Self::WebSearch,
        Self::WebFetch,
        Self::Fetch,
        Self::Mcp,
        Self::ListMcpResources,
        Self::ReadMcpResource,
        Self::UpdateTodos,
        Self::ReadTodos,
        Self::CreatePlan,
        Self::ApplyAgentDiff,
    ];

    /// Exact snake_case name passed to `--allowed-tools`.
    pub fn as_flag(self) -> &'static str {
        match self {
            Self::Read => "read_tool_call",
            Self::Grep => "grep_tool_call",
            Self::Glob => "glob_tool_call",
            Self::Ls => "ls_tool_call",
            Self::SemSearch => "sem_search_tool_call",
            Self::ReadLints => "read_lints_tool_call",
            Self::Edit => "edit_tool_call",
            Self::Delete => "delete_tool_call",
            Self::Shell => "shell_tool_call",
            Self::WriteShellStdin => "write_shell_stdin_tool_call",
            Self::WebSearch => "web_search_tool_call",
            Self::WebFetch => "web_fetch_tool_call",
            Self::Fetch => "fetch_tool_call",
            Self::Mcp => "mcp_tool_call",
            Self::ListMcpResources => "list_mcp_resources_tool_call",
            Self::ReadMcpResource => "read_mcp_resource_tool_call",
            Self::UpdateTodos => "update_todos_tool_call",
            Self::ReadTodos => "read_todos_tool_call",
            Self::CreatePlan => "create_plan_tool_call",
            Self::ApplyAgentDiff => "apply_agent_diff_tool_call",
        }
    }

    // Spawn maps Plan via `is_read_only`; Phase D has not wired session yet.
    #[allow(dead_code)]
    pub fn capability_kind(self) -> CapabilityKind {
        match self {
            Self::Read
            | Self::Grep
            | Self::Glob
            | Self::Ls
            | Self::SemSearch
            | Self::ReadLints
            | Self::ReadTodos
            | Self::ReadMcpResource
            | Self::ListMcpResources => CapabilityKind::Read,
            Self::Edit | Self::Delete | Self::ApplyAgentDiff => CapabilityKind::Write,
            Self::Shell | Self::WriteShellStdin => CapabilityKind::Process,
            Self::WebSearch | Self::WebFetch | Self::Fetch => CapabilityKind::Network,
            // MCP servers expose arbitrary tools, including command execution.
            // Process is the most conservative Rho class that still requires
            // approval under Auto / Allow edits.
            Self::Mcp => CapabilityKind::Process,
            // Todo and plan tools mutate session artifacts, not just display.
            // Write is the conservative class (same bar as Edit).
            Self::UpdateTodos | Self::CreatePlan => CapabilityKind::Write,
        }
    }

    #[allow(dead_code)]
    pub fn is_read_only(self) -> bool {
        matches!(self.capability_kind(), CapabilityKind::Read)
    }

    fn accepted_names() -> String {
        Self::ALL
            .iter()
            .map(|tool| tool.as_flag())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for CursorTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_flag())
    }
}

impl FromStr for CursorTool {
    type Err = CursorToolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|tool| tool.as_flag() == value)
            .ok_or_else(|| CursorToolError {
                value: value.to_string(),
                expected: Self::accepted_names(),
            })
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("unknown Cursor tool '{value}'; expected one of: {expected}")]
pub struct CursorToolError {
    value: String,
    expected: String,
}

#[cfg(test)]
#[path = "cursor_tools_tests.rs"]
mod tests;
