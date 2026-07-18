//! Workspace coding tools and SDK tool adapters shared by Rho hosts.
//!
//! The crate has two layers:
//!
//! - Application tools ([`tool::Tool`]) implement the user-facing built-ins
//!   (`bash`, `read_file`, `write_file`, `edit_file`, `list_dir`) with output
//!   truncation, diffs, and display formatting.
//! - SDK adapters ([`sdk_adapter`], [`sdk_shell`]) wrap those implementations
//!   in the public [`rho_sdk::tool::Tool`] contract so hosts can register them
//!   on an SDK runtime with explicit workspace policies.

pub mod cancellation;
mod paths;
pub mod tool;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod bash;
pub mod diff;
pub mod edit_file;
pub mod edit_file_args;
pub mod list_dir;
#[cfg(windows)]
pub mod powershell;
pub mod read_file;
pub mod rtk;
pub mod sdk_adapter;
pub mod sdk_security;
pub mod sdk_shell;
pub mod sdk_support;
pub mod write_file;

pub use cancellation::RunCancellation;
pub use sdk_adapter::{coding_tools, CodingToolOptions};
pub use sdk_shell::shell_tool;
pub use tool::{
    compact_display_path, resolve_path, truncate, Tool, ToolContext, ToolDisplayStyle, ToolError,
};

/// Default per-tool output budget, in bytes, when the host does not configure
/// one explicitly.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 12_000;
