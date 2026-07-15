use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::cancellation::RunCancellation;
pub use crate::provider_backend::{ToolCall, ToolResult, ToolSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolDisplayStyle {
    DefaultTool,
    FileOrCommand,
    FileDiff,
    Skill,
    Web,
    Questionnaire,
}

impl ToolDisplayStyle {
    pub const fn default_tool() -> Self {
        Self::DefaultTool
    }

    pub const fn file_or_command() -> Self {
        Self::FileOrCommand
    }

    pub const fn file_diff() -> Self {
        Self::FileDiff
    }

    pub const fn skill() -> Self {
        Self::Skill
    }

    pub const fn web() -> Self {
        Self::Web
    }

    pub const fn questionnaire() -> Self {
        Self::Questionnaire
    }
}

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub max_output_bytes: usize,
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("utf-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("{0}")]
    Message(String),
}

/// Extension point for agent tools exposed to model tool calls.
///
/// Implementors should provide a stable JSON schema from `spec` and execute
/// `call` using only the supplied arguments and context, returning user-visible
/// output in the `ToolResult`.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError>;

    async fn call_with_updates(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
        _on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<ToolResult, ToolError> {
        self.call(args, ctx, id).await
    }

    async fn call_with_updates_and_cancellation(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
        cancellation: RunCancellation,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<ToolResult, ToolError> {
        tokio::select! {
            result = self.call_with_updates(args, ctx, id, on_update) => result,
            () = cancellation.cancelled() => Err(ToolError::Message("tool interrupted".into())),
        }
    }
}

pub fn resolve_path(cwd: &std::path::Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        cwd.join(p)
    }
}

pub fn compact_display_path(cwd: &std::path::Path, path: &str) -> String {
    let cwd = normalize_path(cwd);
    let path = normalize_path(&resolve_path(&cwd, path));
    path.strip_prefix(&cwd)
        .ok()
        .map(|path| {
            if path.as_os_str().is_empty() {
                ".".to_string()
            } else {
                path.display().to_string()
            }
        })
        .unwrap_or_else(|| path.display().to_string())
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

pub fn truncate(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let boundary = previous_char_boundary(&s, max);
    s.truncate(boundary);
    s.push_str("\n[truncated]");
    s
}

fn previous_char_boundary(s: &str, index: usize) -> usize {
    let mut index = index.min(s.len());
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_display_path_renders_cwd_as_dot() {
        let cwd = Path::new("/home/emgym/rho");

        assert_eq!(compact_display_path(cwd, "/home/emgym/rho/."), ".");
        assert_eq!(compact_display_path(cwd, "."), ".");
    }

    #[test]
    fn compact_display_path_normalizes_relative_children() {
        let cwd = Path::new("/home/emgym/rho");

        assert_eq!(
            compact_display_path(cwd, "/home/emgym/rho/src/../Cargo.toml"),
            "Cargo.toml"
        );
        assert_eq!(compact_display_path(cwd, "./src"), "src");
    }

    #[test]
    fn truncate_keeps_ascii_prefix() {
        assert_eq!(truncate("abcdef".into(), 3), "abc\n[truncated]");
    }

    #[test]
    fn truncate_does_not_split_utf8_character() {
        assert_eq!(truncate("aébc".into(), 2), "a\n[truncated]");
    }

    #[test]
    fn truncate_allows_exact_utf8_boundary() {
        assert_eq!(truncate("aébc".into(), 3), "aé\n[truncated]");
    }
}
