//! Recovered-session syntax warmup for interactive startup.
//!
//! Collects fence tokens and structured tool-call paths on the startup thread
//! (cheap string work), then inflates the bat dump and those grammars on a
//! blocking worker so the first resume paint can stay off syntect.

use std::collections::HashSet;
use std::path::Path;

use rho_providers::model::{ContentBlock, Message};
use serde_json::Value;

use super::markdown::opening_fence_info_token;
use super::syntax::{
    syntax_name_for_language, syntax_name_for_path, warm_syntax_set, BlockHighlighter,
};

/// Soft cap on distinct grammars compiled during startup warmup.
///
/// TypeScript alone is ~140ms; a long tail of unique extensions should not
/// compile the whole dump on the worker.
const MAX_WARMED_SYNTAXES: usize = 12;

/// Fence languages and tool-call paths worth compiling after the dump loads.
#[derive(Debug, Default, PartialEq, Eq)]
struct SyntaxWarmupPlan {
    tokens: Vec<String>,
    paths: Vec<String>,
}

impl SyntaxWarmupPlan {
    fn from_messages(messages: &[Message]) -> Self {
        let mut plan = Self::default();
        let mut seen_tokens = HashSet::new();
        let mut seen_paths = HashSet::new();
        for message in messages {
            match message {
                Message::User(blocks) | Message::Assistant(blocks) => {
                    plan.collect_blocks(blocks, &mut seen_tokens, &mut seen_paths);
                }
                Message::EnrichedAssistant(message) => {
                    plan.collect_blocks(&message.content, &mut seen_tokens, &mut seen_paths);
                }
                Message::AbortedAssistant(message) => {
                    plan.collect_blocks(&message.content, &mut seen_tokens, &mut seen_paths);
                    for call in &message.tool_calls {
                        if let Ok(arguments) = serde_json::from_str::<Value>(&call.arguments) {
                            plan.push_tool_path(&arguments, &mut seen_paths);
                        }
                    }
                }
                Message::ToolResult(_) | Message::System(_) => {}
            }
        }
        plan
    }

    fn collect_blocks(
        &mut self,
        blocks: &[ContentBlock],
        seen_tokens: &mut HashSet<String>,
        seen_paths: &mut HashSet<String>,
    ) {
        for block in blocks {
            match block {
                ContentBlock::Text(text) => self.collect_tokens(text, seen_tokens),
                ContentBlock::ToolCall(call) => self.push_tool_path(&call.arguments, seen_paths),
                ContentBlock::Image(_) => {}
            }
        }
    }

    fn collect_tokens(&mut self, text: &str, seen: &mut HashSet<String>) {
        for line in text.lines() {
            let Some(token) = opening_fence_info_token(line) else {
                continue;
            };
            if !should_warmup_token(&token) || !seen.insert(token.clone()) {
                continue;
            }
            self.tokens.push(token);
        }
    }

    fn push_tool_path(&mut self, arguments: &Value, seen: &mut HashSet<String>) {
        let Some(path) = tool_argument_path(arguments) else {
            return;
        };
        if !should_warmup_path(path) || !seen.insert(path.to_string()) {
            return;
        }
        self.paths.push(path.to_string());
    }
}

/// Load the dump and recovered grammars off the UI thread.
pub(crate) fn spawn_syntax_warmup(messages: &[Message]) -> tokio::task::JoinHandle<()> {
    let plan = SyntaxWarmupPlan::from_messages(messages);
    tokio::task::spawn_blocking(move || {
        let _span = tracing::info_span!("startup.syntax_set").entered();
        warm_plan(plan);
    })
}

fn warm_plan(plan: SyntaxWarmupPlan) {
    warm_syntax_set();
    for (_, highlighter) in planned_warmups(&plan) {
        warm_highlighter(highlighter);
    }
}

/// Resolve tokens then paths through [`WarmBudget`]. Filters already ran in
/// [`SyntaxWarmupPlan::from_messages`]; this is only identity dedup and cap.
fn planned_warmups(plan: &SyntaxWarmupPlan) -> Vec<(&'static str, BlockHighlighter)> {
    let mut budget = WarmBudget::default();
    let mut planned = Vec::new();
    // Shell headers can appear in every session, so compile both card dialects
    // before the first command reaches the UI thread.
    for token in ["bash", "powershell"] {
        let Some(name) = budget.claim(syntax_name_for_language(token)) else {
            continue;
        };
        if let Some(highlighter) = BlockHighlighter::for_language(token) {
            planned.push((name, highlighter));
        }
    }
    for token in &plan.tokens {
        let Some(name) = budget.claim(syntax_name_for_language(token)) else {
            continue;
        };
        if let Some(highlighter) = BlockHighlighter::for_language(token) {
            planned.push((name, highlighter));
        }
    }
    for path in &plan.paths {
        let Some(name) = budget.claim(syntax_name_for_path(path)) else {
            continue;
        };
        if let Some(highlighter) = BlockHighlighter::for_path(path) {
            planned.push((name, highlighter));
        }
    }
    planned
}

#[derive(Default)]
struct WarmBudget {
    seen: HashSet<&'static str>,
}

impl WarmBudget {
    fn claim(&mut self, name: Option<&'static str>) -> Option<&'static str> {
        let name = name?;
        if self.seen.len() >= MAX_WARMED_SYNTAXES || !self.seen.insert(name) {
            return None;
        }
        Some(name)
    }
}

fn warm_highlighter(mut highlighter: BlockHighlighter) {
    // Force a typical source context. An empty line often stays in the
    // grammar's initial state and misses the compile first paint pays for.
    let _ = highlighter.highlight_line("x = 1");
}

fn tool_argument_path(arguments: &Value) -> Option<&str> {
    arguments
        .get("path")
        .or_else(|| arguments.get("file_path"))
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
}

fn should_warmup_token(token: &str) -> bool {
    !matches!(
        token,
        "md" | "markdown" | "mermaid" | "text" | "plaintext" | "plain"
    )
}

fn should_warmup_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| should_warmup_token(&ext.to_ascii_lowercase()))
}

#[cfg(test)]
#[path = "syntax_warmup_tests.rs"]
mod tests;
