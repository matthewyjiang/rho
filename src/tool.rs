use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub ok: bool,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolRgb(pub u8, pub u8, pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolDisplayStyle {
    pub foreground: ToolRgb,
    pub success_background: ToolRgb,
    pub failure_background: ToolRgb,
}

impl ToolDisplayStyle {
    pub const fn new(
        foreground: ToolRgb,
        success_background: ToolRgb,
        failure_background: ToolRgb,
    ) -> Self {
        Self {
            foreground,
            success_background,
            failure_background,
        }
    }

    pub const fn default_tool() -> Self {
        Self::new(
            ToolRgb(255, 215, 0),
            ToolRgb(48, 45, 30),
            ToolRgb(95, 36, 36),
        )
    }

    pub const fn file_or_command() -> Self {
        Self::new(
            ToolRgb(255, 255, 255),
            ToolRgb(25, 75, 45),
            ToolRgb(95, 36, 36),
        )
    }

    pub const fn skill() -> Self {
        Self::new(
            ToolRgb(255, 255, 255),
            ToolRgb(92, 80, 140),
            ToolRgb(95, 36, 36),
        )
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
        ToolDisplayStyle::default_tool()
    }

    fn display_command(&self, _args: &Value) -> Option<String> {
        None
    }

    fn display_content(&self, _args: &Value, _ctx: &ToolContext) -> Option<String> {
        None
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
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.insert(tool.spec().name, Box::new(tool));
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
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
    let path = resolve_path(cwd, path);
    path.strip_prefix(cwd)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
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
