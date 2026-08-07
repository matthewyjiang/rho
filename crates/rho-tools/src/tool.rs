use std::{
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
};

use serde_json::Value;
use thiserror::Error;

use crate::cancellation::RunCancellation;
pub use rho_sdk::model::{ToolCall, ToolResult, ToolSpec};

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
    #[error("tool interrupted")]
    Cancelled,
    #[error("{0}")]
    Message(String),
}

/// Future returned by app-tool trait methods.
pub type AppToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>>;

/// Extension point for agent tools exposed to model tool calls.
///
/// Implementors should provide a stable JSON schema from `spec` and execute
/// `call` using only the supplied arguments and context, returning user-visible
/// output in the `ToolResult`. Methods return an explicit `Send` future.
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn call<'a>(&'a self, args: Value, ctx: ToolContext, id: String) -> AppToolFuture<'a>;

    /// Runs the tool, reporting interim progress through `on_update`.
    ///
    /// Each update replaces the previous one and contains only progress
    /// content; the presenter renders the tool name and command header.
    fn call_with_updates<'a>(
        &'a self,
        args: Value,
        ctx: ToolContext,
        id: String,
        _on_update: &'a mut (dyn FnMut(Vec<String>) + Send),
    ) -> AppToolFuture<'a> {
        self.call(args, ctx, id)
    }

    fn call_with_updates_and_cancellation<'a>(
        &'a self,
        args: Value,
        ctx: ToolContext,
        id: String,
        cancellation: RunCancellation,
        on_update: &'a mut (dyn FnMut(Vec<String>) + Send),
    ) -> AppToolFuture<'a> {
        let call = self.call_with_updates(args, ctx, id, on_update);
        Box::pin(async move {
            tokio::select! {
                result = call => result,
                () = cancellation.cancelled() => Err(ToolError::Cancelled),
            }
        })
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
                crate::paths::display(path)
            }
        })
        .unwrap_or_else(|| crate::paths::display(&path))
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

/// Appended by [`truncate`] when it drops content.
///
/// Callers that must keep a hard byte budget subtract this before choosing the
/// `max` they pass, so the marker cannot push the result past their limit.
pub use rho_sdk::TRUNCATION_MARKER;

pub fn truncate(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let boundary = rho_sdk::floor_char_boundary(&s, max);
    s.truncate(boundary);
    s.push_str(TRUNCATION_MARKER);
    s
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
