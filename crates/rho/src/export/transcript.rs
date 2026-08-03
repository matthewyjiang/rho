//! Plain Markdown and JSON session transcript renderers.
//!
//! HTML keeps rich formatting in `export.rs`. These formats are for scripts,
//! notes, and machine consumption without embedded styles or scripts.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

use serde::Serialize;

use {
    crate::session::{ExportedMessage, SessionExport},
    rho_providers::model::{ContentBlock, Message},
    rho_tools::tool::{ToolCall, ToolResult},
};

#[derive(Serialize)]
struct JsonTranscript<'a> {
    id: &'a str,
    title: Option<&'a str>,
    cwd: String,
    created_at: u64,
    updated_at: u64,
    exported_at: u64,
    rho_version: &'static str,
    messages: Vec<JsonMessage<'a>>,
}

#[derive(Serialize)]
struct JsonMessage<'a> {
    timestamp: Option<u64>,
    message: &'a Message,
}

pub(super) fn render_markdown(export: &SessionExport) -> String {
    let title = export.title.as_deref().unwrap_or("rho session");
    let mut out = String::new();
    let _ = writeln!(out, "# {title}");
    out.push('\n');
    let _ = writeln!(out, "- session: `{}`", export.id);
    let project = export.cwd.file_name().map_or_else(
        || export.cwd.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let _ = writeln!(out, "- project: {project}");
    let _ = writeln!(
        out,
        "- created: {}",
        super::format_datetime(export.created_at)
    );
    let _ = writeln!(
        out,
        "- updated: {}",
        super::format_datetime(export.updated_at)
    );
    let _ = writeln!(out, "- messages: {}", export.messages.len());
    let _ = writeln!(
        out,
        "- exported: {} by rho v{}",
        super::format_datetime(super::now_unix_secs()),
        env!("CARGO_PKG_VERSION")
    );
    out.push('\n');
    push_markdown_messages(&mut out, &export.messages);
    out
}

pub(super) fn render_json(export: &SessionExport) -> anyhow::Result<String> {
    // Match markdown/HTML: share the workspace directory name, not the full path.
    let cwd = export.cwd.file_name().map_or_else(
        || crate::paths::display(&export.cwd),
        |name| name.to_string_lossy().into_owned(),
    );
    let document = JsonTranscript {
        id: &export.id,
        title: export.title.as_deref(),
        cwd,
        created_at: export.created_at,
        updated_at: export.updated_at,
        exported_at: super::now_unix_secs(),
        rho_version: env!("CARGO_PKG_VERSION"),
        messages: export
            .messages
            .iter()
            .map(|entry| JsonMessage {
                timestamp: entry.timestamp,
                message: &entry.message,
            })
            .collect(),
    };
    Ok(serde_json::to_string_pretty(&document)?)
}

fn push_markdown_messages(out: &mut String, messages: &[ExportedMessage]) {
    let mut results_by_id: HashMap<&str, &ToolResult> = HashMap::new();
    let mut called_ids: HashSet<&str> = HashSet::new();
    for entry in messages {
        match &entry.message {
            Message::ToolResult(result) => {
                results_by_id.entry(result.id.as_str()).or_insert(result);
            }
            Message::Assistant(blocks) => {
                for block in blocks {
                    if let ContentBlock::ToolCall(call) = block {
                        called_ids.insert(call.id.as_str());
                    }
                }
            }
            Message::EnrichedAssistant(message) => {
                for block in &message.content {
                    if let ContentBlock::ToolCall(call) = block {
                        called_ids.insert(call.id.as_str());
                    }
                }
            }
            Message::System(_) | Message::User(_) | Message::AbortedAssistant(_) => {}
        }
    }

    for entry in messages {
        match &entry.message {
            Message::System(text) => {
                out.push_str("## System\n\n");
                push_fenced(out, None, text);
            }
            Message::User(blocks) => {
                out.push_str("## You\n\n");
                push_blocks_markdown(out, blocks, &results_by_id, /*pair_tools*/ false);
            }
            Message::Assistant(blocks) => {
                out.push_str("## Rho\n\n");
                push_blocks_markdown(out, blocks, &results_by_id, /*pair_tools*/ true);
            }
            Message::EnrichedAssistant(message) => {
                out.push_str("## Rho\n\n");
                push_blocks_markdown(
                    out,
                    &message.content,
                    &results_by_id,
                    /*pair_tools*/ true,
                );
            }
            Message::AbortedAssistant(message) => {
                out.push_str("## Rho\n\n");
                push_blocks_markdown(
                    out,
                    &message.content,
                    &results_by_id,
                    /*pair_tools*/ true,
                );
                out.push_str("_Operation aborted_\n\n");
            }
            Message::ToolResult(result) if called_ids.contains(result.id.as_str()) => {}
            Message::ToolResult(result) => {
                out.push_str("### tool result\n\n");
                push_tool_result_markdown(out, None, Some(result));
            }
        }
    }
}

fn push_blocks_markdown(
    out: &mut String,
    blocks: &[ContentBlock],
    results_by_id: &HashMap<&str, &ToolResult>,
    pair_tools: bool,
) {
    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            ContentBlock::Image(image) => {
                let _ = writeln!(
                    out,
                    "![attached image](data:{};base64,{})\n",
                    image.mime_type, image.data
                );
            }
            ContentBlock::ToolCall(call) => {
                let result = if pair_tools {
                    results_by_id.get(call.id.as_str()).copied()
                } else {
                    None
                };
                push_tool_result_markdown(out, Some(call), result);
            }
        }
    }
}

fn push_tool_result_markdown(
    out: &mut String,
    call: Option<&ToolCall>,
    result: Option<&ToolResult>,
) {
    let name = call.map_or("tool result", |call| call.name.as_str());
    let status = match result {
        Some(result) if result.ok => "ok",
        Some(_) => "error",
        None => "no result",
    };
    let _ = writeln!(out, "### `{name}` ({status})\n");
    if let Some(call) = call {
        let arguments = serde_json::to_string_pretty(&call.arguments)
            .unwrap_or_else(|_| call.arguments.to_string());
        out.push_str("**arguments**\n\n");
        push_fenced(out, Some("json"), &arguments);
    }
    if let Some(result) = result {
        out.push_str("**output**\n\n");
        if result.content.is_empty() {
            out.push_str("_(no output)_\n\n");
        } else {
            push_fenced(out, None, &result.content);
        }
    }
}

fn push_fenced(out: &mut String, language: Option<&str>, body: &str) {
    let fence = longest_backtick_run(body).saturating_add(1).max(3);
    let ticks = "`".repeat(fence);
    match language {
        Some(language) => {
            let _ = writeln!(out, "{ticks}{language}");
        }
        None => {
            let _ = writeln!(out, "{ticks}");
        }
    }
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    let _ = writeln!(out, "{ticks}\n");
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}
