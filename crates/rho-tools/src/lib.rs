//! Workspace coding tools and SDK tool adapters shared by Rho hosts.
//!
//! The crate has two layers:
//!
//! - Application tools ([`tool::Tool`]) implement the user-facing built-ins
//!   (`bash`, `read_file`, `write_file`, `edit_file`, `list_dir`) with output
//!   truncation, diffs, and display formatting.
//! - Workspace searches implement `grep` and `glob` over the shared
//!   [`workspace_walk`] walker.
//! - SDK adapters ([`sdk_adapter`], [`sdk_shell`], [`sdk_search`]) wrap those
//!   implementations in the public [`rho_sdk::tool::Tool`] contract so hosts
//!   can register them on an SDK runtime with explicit workspace policies.

pub mod cancellation;
pub mod image_format;
mod path_glob;
mod paths;
mod process_env;
mod process_stream;
mod search;
mod shell_process;
pub use shell_process::{parse_shell_content, ShellContent};
pub mod tool;
pub mod tool_card;
pub mod workspace_walk;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod bash;
pub mod diff;
pub mod edit_file;
pub mod edit_file_args;
mod glob;
mod grep;
mod grep_format;
pub mod list_dir;
#[cfg(windows)]
pub mod powershell;
pub mod read_file;
pub mod rtk;
pub mod sdk_adapter;
mod sdk_search;
pub mod sdk_security;
pub mod sdk_shell;
pub mod sdk_support;
pub mod write_file;

pub use cancellation::RunCancellation;
pub use image_format::{supported_image_mime_type, MAX_IMAGE_FILE_BYTES};
pub use process_env::apply_process_environment;
pub use sdk_adapter::{coding_tool, coding_tools, CodingToolKind, CodingToolOptions};
pub use sdk_shell::{shell_invocation, shell_tool, ShellToolOptions};
pub use tool::{compact_display_path, resolve_path, truncate, Tool, ToolContext, ToolError};

/// Default per-tool output budget, in bytes, when the host does not configure
/// one explicitly.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 12_000;
