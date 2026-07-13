//! Render saved sessions as self-contained HTML transcripts.
//!
//! Consumes [`SessionExport`] data from the session store and produces a
//! single HTML file with embedded styles: user prompts as plain text,
//! assistant text as rendered markdown, and tool calls paired with their
//! outputs in collapsible blocks.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    model::{ContentBlock, ImageContent, Message},
    session::{ExportedMessage, Session, SessionExport},
    tool::{ToolCall, ToolResult},
};

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;

/// Tool outputs at most this many lines render expanded; longer ones start
/// collapsed so the transcript stays skimmable.
const TOOL_OUTPUT_EXPANDED_MAX_LINES: usize = 24;
const TOOL_ARGUMENT_PREVIEW_MAX_CHARS: usize = 80;

/// Load the session matching `id_prefix`, render it, and write the HTML file.
///
/// `path_arg` is the raw `/export` argument: empty selects a default file name
/// in `cwd`, a directory receives the default file name, and anything else is
/// used as the output file path (relative paths resolve against `cwd`).
pub fn write_session_html(cwd: &Path, id_prefix: &str, path_arg: &str) -> anyhow::Result<PathBuf> {
    let export = Session::export_by_id(cwd, id_prefix)?;
    write_export_html(cwd, path_arg, &export)
}

pub(crate) fn write_export_html(
    cwd: &Path,
    path_arg: &str,
    export: &SessionExport,
) -> anyhow::Result<PathBuf> {
    let path = resolve_output_path(cwd, path_arg, &export.id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, render_html(export))?;
    Ok(path)
}

pub(crate) fn resolve_output_path(cwd: &Path, path_arg: &str, session_id: &str) -> PathBuf {
    let short_id: String = session_id.chars().take(8).collect();
    let default_name = format!("rho-session-{short_id}.html");
    let path_arg = path_arg.trim();
    if path_arg.is_empty() {
        return cwd.join(default_name);
    }
    let path = PathBuf::from(path_arg);
    let path = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    if path.is_dir() {
        path.join(default_name)
    } else {
        path
    }
}

pub(crate) fn render_html(export: &SessionExport) -> String {
    let title = export.title.as_deref().unwrap_or("rho session");
    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    let _ = writeln!(html, "<title>{}</title>", escape_html(title));
    html.push_str("<style>\n");
    html.push_str(STYLE);
    html.push_str("</style>\n</head>\n<body>\n");
    push_header(&mut html, export, title);
    html.push_str("<main>\n");
    push_messages(&mut html, &export.messages);
    html.push_str("</main>\n</body>\n</html>\n");
    html
}

fn push_header(html: &mut String, export: &SessionExport, title: &str) {
    html.push_str("<header>\n");
    let _ = writeln!(html, "<h1>{}</h1>", escape_html(title));
    html.push_str("<dl class=\"meta\">\n");
    push_meta_row(html, "Session", &export.id);
    push_meta_row(html, "Workspace", &export.cwd.display().to_string());
    push_meta_row(html, "Created", &format_datetime(export.created_at));
    push_meta_row(html, "Updated", &format_datetime(export.updated_at));
    push_meta_row(html, "Messages", &export.messages.len().to_string());
    let exported = format!(
        "{} by rho v{}",
        format_datetime(now_unix_secs()),
        env!("CARGO_PKG_VERSION")
    );
    push_meta_row(html, "Exported", &exported);
    html.push_str("</dl>\n</header>\n");
}

fn push_meta_row(html: &mut String, label: &str, value: &str) {
    let _ = writeln!(
        html,
        "<div><dt>{}</dt><dd>{}</dd></div>",
        escape_html(label),
        escape_html(value)
    );
}

fn push_messages(html: &mut String, messages: &[ExportedMessage]) {
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
            Message::System(_) | Message::User(_) => {}
        }
    }

    for entry in messages {
        match &entry.message {
            Message::System(text) => push_system(html, text),
            Message::User(blocks) => push_user(html, entry.timestamp, blocks),
            Message::Assistant(blocks) => {
                push_assistant(html, entry.timestamp, blocks, &results_by_id)
            }
            // Rendered inline with the tool call that produced it.
            Message::ToolResult(result) if called_ids.contains(result.id.as_str()) => {}
            Message::ToolResult(result) => push_tool_call(html, None, Some(result)),
        }
    }
}

fn push_system(html: &mut String, text: &str) {
    html.push_str("<details class=\"system\">\n<summary>System prompt</summary>\n");
    let _ = writeln!(html, "<pre>{}</pre>", escape_html(text));
    html.push_str("</details>\n");
}

fn push_user(html: &mut String, timestamp: Option<u64>, blocks: &[ContentBlock]) {
    html.push_str("<section class=\"entry user\">\n");
    push_entry_head(html, "You", timestamp);
    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                let _ = writeln!(html, "<div class=\"plain\">{}</div>", escape_html(text));
            }
            ContentBlock::Image(image) => push_image(html, image),
            // User messages never carry tool calls; keep the transcript
            // complete if one ever appears.
            ContentBlock::ToolCall(call) => push_tool_call(html, Some(call), None),
        }
    }
    html.push_str("</section>\n");
}

fn push_assistant(
    html: &mut String,
    timestamp: Option<u64>,
    blocks: &[ContentBlock],
    results_by_id: &HashMap<&str, &ToolResult>,
) {
    html.push_str("<section class=\"entry assistant\">\n");
    push_entry_head(html, "Assistant", timestamp);
    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                let _ = writeln!(
                    html,
                    "<div class=\"markdown\">{}</div>",
                    markdown_to_html(text)
                );
            }
            ContentBlock::Image(image) => push_image(html, image),
            ContentBlock::ToolCall(call) => push_tool_call(
                html,
                Some(call),
                results_by_id.get(call.id.as_str()).copied(),
            ),
        }
    }
    html.push_str("</section>\n");
}

fn push_entry_head(html: &mut String, role: &str, timestamp: Option<u64>) {
    html.push_str("<div class=\"entry-head\">");
    let _ = write!(html, "<span class=\"role\">{}</span>", escape_html(role));
    if let Some(timestamp) = timestamp {
        let _ = write!(
            html,
            "<time title=\"{}\">{}</time>",
            escape_html(&format_datetime(timestamp)),
            escape_html(&format_clock(timestamp))
        );
    }
    html.push_str("</div>\n");
}

fn push_image(html: &mut String, image: &ImageContent) {
    let _ = writeln!(
        html,
        "<img class=\"attachment\" alt=\"attached image\" src=\"data:{};base64,{}\">",
        escape_html(&image.mime_type),
        escape_html(&image.data)
    );
}

fn push_tool_call(html: &mut String, call: Option<&ToolCall>, result: Option<&ToolResult>) {
    let (status_class, status_label) = match result {
        Some(result) if result.ok => ("ok", "ok"),
        Some(_) => ("err", "error"),
        None => ("pending", "no result"),
    };
    let output_lines = result.map_or(0, |result| result.content.lines().count());
    let open_attribute = if output_lines <= TOOL_OUTPUT_EXPANDED_MAX_LINES {
        " open"
    } else {
        ""
    };
    let name = call.map_or("tool result", |call| call.name.as_str());

    let _ = write!(html, "<details class=\"tool\"{open_attribute}>\n<summary>");
    let _ = write!(
        html,
        "<span class=\"tool-name\">{}</span>",
        escape_html(name)
    );
    if let Some(call) = call {
        let _ = write!(
            html,
            "<span class=\"tool-preview\">{}</span>",
            escape_html(&argument_preview(call))
        );
    }
    let _ = writeln!(
        html,
        "<span class=\"status {status_class}\">{status_label}</span></summary>"
    );
    html.push_str("<div class=\"tool-body\">\n");
    if let Some(call) = call {
        html.push_str("<div class=\"tool-section\">arguments</div>\n");
        let arguments = serde_json::to_string_pretty(&call.arguments)
            .unwrap_or_else(|_| call.arguments.to_string());
        let _ = writeln!(
            html,
            "<pre class=\"tool-args\">{}</pre>",
            escape_html(&arguments)
        );
    }
    if let Some(result) = result {
        html.push_str("<div class=\"tool-section\">output</div>\n");
        if result.content.is_empty() {
            html.push_str("<pre class=\"tool-output empty\">(no output)</pre>\n");
        } else {
            let _ = writeln!(
                html,
                "<pre class=\"tool-output\">{}</pre>",
                escape_html(&result.content)
            );
        }
    }
    html.push_str("</div>\n</details>\n");
}

fn argument_preview(call: &ToolCall) -> String {
    let compact = serde_json::to_string(&call.arguments).unwrap_or_default();
    let mut preview: String = compact
        .chars()
        .take(TOOL_ARGUMENT_PREVIEW_MAX_CHARS)
        .collect();
    if preview.len() < compact.len() {
        preview.push('…');
    }
    preview
}

fn markdown_to_html(text: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(text, options);
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    rendered
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn format_datetime(unix_secs: u64) -> String {
    local_datetime(unix_secs).map_or_else(
        || unix_secs.to_string(),
        |time| time.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}

fn format_clock(unix_secs: u64) -> String {
    local_datetime(unix_secs).map_or_else(
        || unix_secs.to_string(),
        |time| time.format("%H:%M:%S").to_string(),
    )
}

fn local_datetime(unix_secs: u64) -> Option<chrono::DateTime<chrono::Local>> {
    let timestamp = i64::try_from(unix_secs).ok()?;
    Some(chrono::DateTime::from_timestamp(timestamp, 0)?.with_timezone(&chrono::Local))
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #f6f7f9;
  --card-bg: #ffffff;
  --text: #1c2128;
  --muted: #59636e;
  --border: #d8dee4;
  --code-bg: #eef1f4;
  --user-accent: #0969da;
  --assistant-accent: #8250df;
  --tool-accent: #57606a;
  --ok: #1a7f37;
  --err: #cf222e;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #14171b;
    --card-bg: #1d2229;
    --text: #e6e9ec;
    --muted: #9aa4af;
    --border: #333b45;
    --code-bg: #14181d;
    --user-accent: #539bf5;
    --assistant-accent: #b083f0;
    --tool-accent: #909aa4;
    --ok: #57ab5a;
    --err: #f47067;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0 auto;
  padding: 2rem 1rem 4rem;
  max-width: 56rem;
  background: var(--bg);
  color: var(--text);
  font: 16px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif;
}
header h1 { margin: 0 0 0.75rem; font-size: 1.5rem; }
dl.meta {
  margin: 0 0 2rem;
  padding: 1rem 1.25rem;
  background: var(--card-bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
  gap: 0.25rem 1.5rem;
  font-size: 0.875rem;
}
dl.meta div { display: flex; gap: 0.5rem; min-width: 0; }
dl.meta dt { color: var(--muted); flex: none; width: 6rem; }
dl.meta dd { margin: 0; overflow-wrap: anywhere; }
main { display: flex; flex-direction: column; gap: 1rem; }
.entry {
  background: var(--card-bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 0.75rem 1.25rem 1rem;
}
.entry.user { border-left: 3px solid var(--user-accent); }
.entry.assistant { border-left: 3px solid var(--assistant-accent); }
.entry-head {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  margin-bottom: 0.5rem;
}
.entry-head .role {
  font-size: 0.75rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--muted);
}
.entry.user .role { color: var(--user-accent); }
.entry.assistant .role { color: var(--assistant-accent); }
.entry-head time { font-size: 0.75rem; color: var(--muted); }
.plain { white-space: pre-wrap; overflow-wrap: anywhere; }
img.attachment {
  max-width: 100%;
  border: 1px solid var(--border);
  border-radius: 6px;
  margin: 0.5rem 0;
}
pre {
  background: var(--code-bg);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 0.75rem;
  overflow-x: auto;
  font: 0.8125rem/1.5 ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
code {
  background: var(--code-bg);
  border-radius: 4px;
  padding: 0.125rem 0.3rem;
  font: 0.875em ui-monospace, "SF Mono", Menlo, Consolas, monospace;
}
pre code { background: none; padding: 0; }
details.system, details.tool {
  background: var(--card-bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 0;
  overflow: hidden;
}
details.system > summary, details.tool > summary {
  cursor: pointer;
  padding: 0.6rem 1.25rem;
  font-size: 0.8125rem;
  color: var(--muted);
  display: flex;
  align-items: baseline;
  gap: 0.75rem;
}
details.tool { border-left: 3px solid var(--tool-accent); margin: 0.5rem 0; }
details.tool > summary .tool-name {
  font: 600 0.8125rem ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  color: var(--text);
}
details.tool > summary .tool-preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font: 0.75rem ui-monospace, "SF Mono", Menlo, Consolas, monospace;
}
details.tool > summary .status {
  flex: none;
  font-size: 0.6875rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
.status.ok { color: var(--ok); }
.status.err { color: var(--err); }
.status.pending { color: var(--muted); }
.tool-body { padding: 0 1.25rem 0.75rem; }
.tool-section {
  font-size: 0.6875rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--muted);
  margin: 0.5rem 0 0.25rem;
}
.tool-output.empty { color: var(--muted); font-style: italic; }
details.system > pre { margin: 0 1.25rem 0.75rem; }
.markdown > :first-child { margin-top: 0; }
.markdown > :last-child { margin-bottom: 0; }
.markdown h1, .markdown h2, .markdown h3 { margin: 1.25rem 0 0.5rem; line-height: 1.3; }
.markdown h1 { font-size: 1.25rem; }
.markdown h2 { font-size: 1.125rem; }
.markdown h3 { font-size: 1rem; }
.markdown blockquote {
  margin: 0.5rem 0;
  padding: 0.25rem 1rem;
  border-left: 3px solid var(--border);
  color: var(--muted);
}
.markdown table { border-collapse: collapse; display: block; overflow-x: auto; }
.markdown th, .markdown td { border: 1px solid var(--border); padding: 0.375rem 0.75rem; }
.markdown th { background: var(--code-bg); }
.markdown a { color: var(--user-accent); }
.markdown hr { border: none; border-top: 1px solid var(--border); }
"#;
