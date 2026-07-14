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
    io::Write as _,
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

#[path = "export/markdown.rs"]
mod markdown;

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
    let rendered = render_html(export);
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    set_private_file_permissions(&file)?;
    file.write_all(rendered.as_bytes())?;
    Ok(path)
}

fn set_private_file_permissions(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
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
    html.push_str("</main>\n<script>\n");
    html.push_str(SCRIPT);
    html.push_str("</script>\n</body>\n</html>\n");
    html
}

fn push_header(html: &mut String, export: &SessionExport, title: &str) {
    html.push_str("<header>\n");
    html.push_str(
        "<p class=\"brand\"><span class=\"brand-mark\">ρ</span> session transcript</p>\n",
    );
    let _ = writeln!(html, "<h1>{}</h1>", escape_html(title));
    html.push_str("<dl class=\"meta\">\n");
    push_meta_row(html, "session", &export.id);
    // Transcripts are meant to be shared; the workspace directory name
    // identifies the project without leaking the local filesystem layout.
    let project = export.cwd.file_name().map_or_else(
        || export.cwd.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    push_meta_row(html, "project", &project);
    push_meta_row(html, "created", &format_datetime(export.created_at));
    push_meta_row(html, "updated", &format_datetime(export.updated_at));
    push_meta_row(html, "messages", &export.messages.len().to_string());
    let exported = format!(
        "{} by rho v{}",
        format_datetime(now_unix_secs()),
        env!("CARGO_PKG_VERSION")
    );
    push_meta_row(html, "exported", &exported);
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

    // Consecutive turns by the same speaker share one role head so the
    // transcript reads as exchanges, not a stack of identical blocks.
    let mut previous_role: Option<&str> = None;
    for entry in messages {
        match &entry.message {
            Message::System(text) => {
                push_system(html, text);
                previous_role = None;
            }
            Message::User(blocks) => {
                let continuation = previous_role == Some("user");
                push_user(html, entry.timestamp, blocks, continuation);
                previous_role = Some("user");
            }
            Message::Assistant(blocks) => {
                let continuation = previous_role == Some("assistant");
                push_assistant(html, entry.timestamp, blocks, &results_by_id, continuation);
                previous_role = Some("assistant");
            }
            // Rendered inline with the tool call that produced it; the
            // speaker run continues across it.
            Message::ToolResult(result) if called_ids.contains(result.id.as_str()) => {}
            Message::ToolResult(result) => {
                push_tool_call(html, None, Some(result));
                previous_role = None;
            }
        }
    }
}

fn push_system(html: &mut String, text: &str) {
    html.push_str("<details class=\"system\">\n<summary>System prompt</summary>\n");
    let _ = writeln!(html, "<pre>{}</pre>", escape_html(text));
    html.push_str("</details>\n");
}

fn push_user(
    html: &mut String,
    timestamp: Option<u64>,
    blocks: &[ContentBlock],
    continuation: bool,
) {
    let cont_class = if continuation { " cont" } else { "" };
    let _ = writeln!(html, "<section class=\"entry user{cont_class}\">");
    if !continuation {
        push_entry_head(html, "you", timestamp);
    }
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
    continuation: bool,
) {
    let cont_class = if continuation { " cont" } else { "" };
    let _ = writeln!(html, "<section class=\"entry assistant{cont_class}\">");
    if !continuation {
        push_entry_head(html, "rho", timestamp);
    }
    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                let _ = writeln!(
                    html,
                    "<div class=\"markdown\">{}</div>",
                    markdown::to_html(text)
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

/// Argument fields that make a good one-line summary, in priority order.
const PREVIEW_KEYS: [&str; 8] = [
    "command",
    "file_path",
    "path",
    "pattern",
    "query",
    "url",
    "description",
    "prompt",
];

fn argument_preview(call: &ToolCall) -> String {
    let picked = call.arguments.as_object().and_then(|fields| {
        PREVIEW_KEYS
            .iter()
            .find_map(|key| fields.get(*key).and_then(|value| value.as_str()))
            .or_else(|| {
                (fields.len() == 1)
                    .then(|| fields.values().next().and_then(|value| value.as_str()))
                    .flatten()
            })
    });
    let full = match picked {
        Some(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
        None => serde_json::to_string(&call.arguments).unwrap_or_default(),
    };
    let mut preview: String = full.chars().take(TOOL_ARGUMENT_PREVIEW_MAX_CHARS).collect();
    if preview.chars().count() < full.chars().count() {
        preview.push('…');
    }
    preview
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
  --bg: oklch(100% 0 0);
  --surface: oklch(96.5% 0.004 20);
  --well: oklch(93% 0.006 20);
  --ink: oklch(24% 0.012 20);
  --muted: oklch(50% 0.02 20);
  --line: oklch(90% 0.008 20);
  --brand: oklch(42% 0.125 18);
  --user-bg: oklch(96.6% 0.013 18);
  --ok: oklch(52% 0.12 150);
  --err: oklch(50% 0.17 27);
  --font-sans: ui-sans-serif, system-ui, "Segoe UI", Roboto, sans-serif;
  --font-mono: ui-monospace, "SF Mono", "Cascadia Code", Menlo, Consolas, monospace;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: oklch(15.5% 0 0);
    --surface: oklch(20.5% 0 0);
    --well: oklch(12.5% 0 0);
    --ink: oklch(90% 0.006 20);
    --muted: oklch(67% 0.012 20);
    --line: oklch(28.5% 0 0);
    --brand: oklch(72% 0.1 18);
    --user-bg: oklch(23% 0.02 18);
    --ok: oklch(72% 0.13 150);
    --err: oklch(70% 0.15 27);
  }
}
* { box-sizing: border-box; }
::selection { background: color-mix(in oklab, var(--brand) 22%, var(--bg)); }
body {
  margin: 0 auto;
  padding: 2.75rem 1.25rem 5rem;
  max-width: 45rem;
  background: var(--bg);
  color: var(--ink);
  font: 16px/1.65 var(--font-sans);
}
header { margin-bottom: 2.75rem; }
header .brand {
  margin: 0 0 1rem;
  font: 500 0.8125rem/1 var(--font-mono);
  color: var(--brand);
}
header .brand-mark { font-size: 1.0625rem; margin-right: 0.125rem; }
header h1 {
  margin: 0 0 1.25rem;
  font-size: 1.75rem;
  font-weight: 700;
  line-height: 1.25;
  letter-spacing: -0.015em;
  text-wrap: balance;
}
dl.meta {
  margin: 0;
  padding: 0.875rem 0;
  border-block: 1px solid var(--line);
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(19rem, 1fr));
  gap: 0.375rem 2rem;
  font-size: 0.8125rem;
}
dl.meta div { display: flex; gap: 0.75rem; min-width: 0; align-items: baseline; }
dl.meta dt { color: var(--muted); flex: none; width: 5.25rem; }
dl.meta dd { margin: 0; overflow-wrap: anywhere; font: 0.75rem/1.6 var(--font-mono); }
.controls { display: flex; gap: 0.5rem; margin-top: 1rem; }
.controls button {
  font: 500 0.75rem/1 var(--font-mono);
  color: var(--muted);
  background: none;
  border: 1px solid var(--line);
  border-radius: 999px;
  padding: 0.4375rem 0.875rem;
  cursor: pointer;
  transition: color 0.15s, border-color 0.15s, background-color 0.15s;
}
.controls button:hover { color: var(--ink); border-color: var(--muted); }
.controls button:active { background: var(--surface); }
.controls button:focus-visible { outline: 2px solid var(--brand); outline-offset: 2px; }
main { display: flex; flex-direction: column; gap: 1.25rem; }
.entry.user {
  background: var(--user-bg);
  border-radius: 12px;
  padding: 0.875rem 1.125rem 1rem;
}
main > .entry.user:not(:first-child) { margin-top: 1.25rem; }
.entry.cont { margin-top: -0.5rem; }
.entry-head {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  gap: 1rem;
  margin-bottom: 0.375rem;
}
.entry-head .role { font: 600 0.75rem/1 var(--font-mono); }
.entry.user .role { color: var(--brand); }
.entry.assistant .role { color: var(--muted); }
.entry-head time { font: 0.6875rem/1 var(--font-mono); color: var(--muted); }
.plain { white-space: pre-wrap; overflow-wrap: anywhere; }
img.attachment {
  max-width: 100%;
  border: 1px solid var(--line);
  border-radius: 8px;
  margin: 0.5rem 0;
}
pre {
  background: var(--surface);
  border-radius: 8px;
  padding: 0.75rem 0.875rem;
  overflow-x: auto;
  font: 0.8125rem/1.55 var(--font-mono);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
code {
  background: var(--surface);
  border-radius: 4px;
  padding: 0.125rem 0.3125rem;
  font: 0.875em var(--font-mono);
}
pre code { background: none; padding: 0; font: inherit; }
details.system, details.tool {
  background: var(--surface);
  border-radius: 10px;
  overflow: hidden;
}
details.tool { margin: 0.625rem 0; }
details.system > summary, details.tool > summary {
  cursor: pointer;
  list-style: none;
  display: flex;
  align-items: baseline;
  gap: 0.625rem;
  padding: 0.5625rem 0.875rem;
  font: 0.75rem/1.5 var(--font-mono);
  color: var(--muted);
  transition: background-color 0.15s ease-out;
}
details.system > summary::-webkit-details-marker,
details.tool > summary::-webkit-details-marker { display: none; }
details.system > summary:hover, details.tool > summary:hover { background: var(--well); }
details.system > summary:focus-visible, details.tool > summary:focus-visible {
  outline: 2px solid var(--brand);
  outline-offset: -2px;
}
details.system > summary::before, details.tool > summary::before {
  content: "";
  flex: none;
  align-self: center;
  width: 0.3125rem;
  height: 0.3125rem;
  border-right: 1.5px solid currentColor;
  border-bottom: 1.5px solid currentColor;
  transform: rotate(-45deg);
  transition: transform 0.15s ease-out;
}
details[open] > summary::before { transform: rotate(45deg); }
details.tool > summary .tool-name { font-weight: 600; color: var(--ink); }
details.tool > summary .tool-preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
details.tool > summary .status {
  flex: none;
  display: inline-flex;
  align-items: center;
  gap: 0.375rem;
  font-size: 0.6875rem;
  font-weight: 600;
}
details.tool > summary .status::before {
  content: "";
  width: 0.375rem;
  height: 0.375rem;
  border-radius: 50%;
  background: currentColor;
}
.status.ok { color: var(--ok); }
.status.err { color: var(--err); }
.status.pending { color: var(--muted); }
.tool-body { padding: 0.25rem 0.875rem 0.75rem; border-top: 1px solid var(--line); }
.tool-body pre { background: var(--well); margin: 0; }
.tool-section {
  font: 600 0.6875rem/1 var(--font-mono);
  color: var(--muted);
  margin: 0.625rem 0 0.375rem;
}
.tool-output.empty { color: var(--muted); font-style: italic; }
details.system > pre { margin: 0 0.875rem 0.75rem; background: var(--well); }
.markdown > :first-child { margin-top: 0; }
.markdown > :last-child { margin-bottom: 0; }
.markdown h1, .markdown h2, .markdown h3 { margin: 1.5rem 0 0.5rem; line-height: 1.3; }
.markdown h1 { font-size: 1.25rem; }
.markdown h2 { font-size: 1.125rem; }
.markdown h3 { font-size: 1rem; }
.markdown blockquote {
  margin: 0.5rem 0;
  padding: 0.25rem 1rem;
  border-left: 1px solid var(--muted);
  color: var(--muted);
}
.markdown table { border-collapse: collapse; display: block; overflow-x: auto; }
.markdown th, .markdown td { border: 1px solid var(--line); padding: 0.375rem 0.75rem; }
.markdown th { background: var(--surface); }
.markdown a { color: var(--brand); text-underline-offset: 2px; }
.markdown math[display="block"] {
  display: block;
  max-width: 100%;
  margin: 1rem 0;
  overflow-x: auto;
  overflow-y: hidden;
}
.markdown .math-fallback { color: var(--err); }
.markdown hr { border: none; border-top: 1px solid var(--line); }
@media (prefers-reduced-motion: reduce) {
  * { transition: none !important; }
}
@media print {
  body { max-width: none; padding: 0; }
  .controls { display: none; }
  details.system, details.tool { break-inside: avoid; }
}
"#;

/// Injects the expand/collapse-all controls; without JavaScript the
/// transcript renders complete and the controls simply never appear.
const SCRIPT: &str = r#"
(function () {
  var tools = document.querySelectorAll("details.tool");
  if (tools.length === 0) return;
  var controls = document.createElement("div");
  controls.className = "controls";
  [["expand all", true], ["collapse all", false]].forEach(function (pair) {
    var button = document.createElement("button");
    button.type = "button";
    button.textContent = pair[0];
    button.addEventListener("click", function () {
      tools.forEach(function (tool) { tool.open = pair[1]; });
    });
    controls.appendChild(button);
  });
  document.querySelector("header").appendChild(controls);
})();
"#;
