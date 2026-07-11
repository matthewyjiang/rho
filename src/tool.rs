use std::{
    collections::HashMap,
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use serde_json::Value;
use thiserror::Error;

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

    pub fn for_tool_name(name: &str) -> Self {
        match name {
            "edit_file" | "write_file" => Self::file_diff(),
            "bash" | "powershell" | "list_dir" | "read_file" => Self::file_or_command(),
            "skill" => Self::skill(),
            "web_search" | "fetch_content" | "get_search_content" => Self::web(),
            "questionnaire" => Self::questionnaire(),
            _ => Self::default_tool(),
        }
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

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::for_tool_name(&self.spec().name)
    }

    fn display_command(&self, _args: &Value) -> Option<String> {
        None
    }

    fn display_content(&self, _args: &Value, _ctx: &ToolContext) -> Option<String> {
        None
    }

    fn display_start_lines(&self, args: &Value, ctx: &ToolContext) -> Vec<String> {
        let mut lines = vec![self.spec().name];
        if let Some(command) = self
            .display_command(args)
            .filter(|command| !command.trim().is_empty())
        {
            lines.push(command);
        } else if let Some(content) = self
            .display_content(args, ctx)
            .filter(|content| !content.trim().is_empty())
        {
            lines.push(content);
        }
        lines
    }

    fn display_lines(&self, args: &Value, ctx: &ToolContext, result: &ToolResult) -> Vec<String> {
        let mut lines = vec![self.spec().name];
        if let Some(command) = self
            .display_command(args)
            .filter(|command| !command.trim().is_empty())
        {
            lines.push(command);
        }
        let content = self
            .display_content(args, ctx)
            .unwrap_or_else(|| result.content.clone());
        if !content.trim().is_empty() {
            lines.push(content);
        }
        lines
    }

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
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    shutdown: Option<Arc<dyn ToolShutdown>>,
}

pub trait ToolShutdown: Send + Sync {
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            shutdown: None,
        }
    }

    pub fn set_shutdown<T: ToolShutdown + 'static>(&mut self, shutdown: T) {
        self.shutdown = Some(Arc::new(shutdown));
    }

    pub async fn shutdown(&self) {
        if let Some(shutdown) = &self.shutdown {
            shutdown.shutdown().await;
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.insert(tool.spec().name, Arc::new(tool));
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
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
    fn tool_display_style_is_recovered_from_tool_name() {
        assert_eq!(
            ToolDisplayStyle::for_tool_name("read_file"),
            ToolDisplayStyle::FileOrCommand
        );
        assert_eq!(
            ToolDisplayStyle::for_tool_name("bash"),
            ToolDisplayStyle::FileOrCommand
        );
        assert_eq!(
            ToolDisplayStyle::for_tool_name("powershell"),
            ToolDisplayStyle::FileOrCommand
        );
        assert_eq!(
            ToolDisplayStyle::for_tool_name("write_file"),
            ToolDisplayStyle::FileDiff
        );
        assert_eq!(
            ToolDisplayStyle::for_tool_name("edit_file"),
            ToolDisplayStyle::FileDiff
        );
        assert_eq!(
            ToolDisplayStyle::for_tool_name("skill"),
            ToolDisplayStyle::Skill
        );
        for name in ["web_search", "fetch_content", "get_search_content"] {
            assert_eq!(ToolDisplayStyle::for_tool_name(name), ToolDisplayStyle::Web);
        }
        assert_eq!(
            ToolDisplayStyle::for_tool_name("questionnaire"),
            ToolDisplayStyle::Questionnaire
        );
        assert_eq!(
            ToolDisplayStyle::for_tool_name("custom"),
            ToolDisplayStyle::DefaultTool
        );
    }

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
