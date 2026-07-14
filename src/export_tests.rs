use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;
use crate::model::{ContentBlock, ImageContent, Message};

const SESSION_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

fn export_with_messages(messages: Vec<ExportedMessage>) -> SessionExport {
    SessionExport {
        id: SESSION_ID.into(),
        cwd: PathBuf::from("/tmp/example-workspace"),
        created_at: 1_700_000_000,
        updated_at: 1_700_000_100,
        title: Some("Fix the login bug".into()),
        messages,
    }
}

fn message(message: Message) -> ExportedMessage {
    ExportedMessage {
        timestamp: Some(1_700_000_050),
        message,
    }
}

fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    })])
}

fn tool_result(id: &str, ok: bool, content: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok,
        content: content.into(),
    })
}

#[test]
fn renders_session_metadata_and_title() {
    let html = render_html(&export_with_messages(vec![]));

    assert!(html.contains("<title>Fix the login bug</title>"));
    assert!(html.contains("<h1>Fix the login bug</h1>"));
    assert!(html.contains(SESSION_ID));
    assert!(html.contains("<dd>example-workspace</dd>"));
    assert!(!html.contains("/tmp/example-workspace"));
    assert!(html.contains(concat!("rho v", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn untitled_session_falls_back_to_generic_title() {
    let mut export = export_with_messages(vec![]);
    export.title = None;

    let html = render_html(&export);

    assert!(html.contains("<title>rho session</title>"));
}

#[test]
fn renders_assistant_text_as_markdown() {
    let export = export_with_messages(vec![message(Message::assistant_text(
        "# Plan\n\nUse **bold** moves:\n\n- step one\n\n```rust\nfn main() {}\n```",
    ))]);

    let html = render_html(&export);

    assert!(html.contains("<h1>Plan</h1>"));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("<li>step one</li>"));
    assert!(html.contains("<code class=\"language-rust\">fn main() {}"));
}

#[test]
fn renders_assistant_latex_as_mathml() {
    let export = export_with_messages(vec![message(Message::assistant_text(
        "Euler's identity is $e^{i\\pi} + 1 = 0$.\n\n$$\\int_0^1 x^2 \\, dx = \\frac{1}{3}$$",
    ))]);

    let html = render_html(&export);

    assert!(html.contains("<math xmlns=\"http://www.w3.org/1998/Math/MathML\">"));
    assert!(html.contains("<msup>"));
    assert!(html.contains("class=\"katex-display\""));
    assert!(html.contains("display=\"block\""));
    assert!(html.contains("<mfrac>"));
    assert!(!html.contains("$e^{i\\pi}"));
}

#[test]
fn leaves_latex_delimiters_literal_in_code() {
    let export = export_with_messages(vec![message(Message::assistant_text(
        "`$x^2$`\n\n```text\n$$y^2$$\n```",
    ))]);

    let html = render_html(&export);

    assert!(html.contains("<code>$x^2$</code>"));
    assert!(html.contains("<code class=\"language-text\">$$y^2$$"));
    assert!(!html.contains("<math"));
}

#[test]
fn escapes_html_inside_latex() {
    let export = export_with_messages(vec![message(Message::assistant_text(
        "$\\text{<script>alert('x')</script>}$",
    ))]);

    let html = render_html(&export);

    assert!(html.contains("<math xmlns=\"http://www.w3.org/1998/Math/MathML\">"));
    assert!(html.contains("&lt;script&gt;alert(’x’)&lt;/script&gt;"));
    assert!(!html.contains("class=\"math-fallback\""));
    assert!(!html.contains("<script>alert"));
}

#[test]
fn escapes_invalid_latex_in_fallback() {
    let export = export_with_messages(vec![message(Message::assistant_text(
        "$\\definitelyUnknown{<script>alert('x')</script>}$",
    ))]);

    let html = render_html(&export);

    assert!(html.contains("<code class=\"math-fallback\">"));
    assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"));
    assert!(!html.contains("<script>alert"));
}

#[test]
fn escapes_raw_html_in_assistant_markdown() {
    let export = export_with_messages(vec![message(Message::assistant_text(
        "before <img src=x onerror=\"alert('x')\"> after\n\n<script>alert('x')</script>",
    ))]);

    let html = render_html(&export);

    assert!(html.contains("&lt;img src=x onerror=\"alert('x')\"&gt;"));
    assert!(html.contains("&lt;script&gt;alert('x')&lt;/script&gt;"));
    assert!(!html.contains("<img src=x"));
    assert!(!html.contains("<script>alert"));
}

#[test]
fn renders_user_text_as_escaped_plain_text() {
    let export = export_with_messages(vec![message(Message::user_text(
        "compare a < b && c > d with **no** markdown",
    ))]);

    let html = render_html(&export);

    assert!(html.contains("a &lt; b &amp;&amp; c &gt; d"));
    assert!(html.contains("**no** markdown"));
    assert!(!html.contains("<strong>no</strong>"));
}

#[test]
fn escapes_html_in_tool_output() {
    let export = export_with_messages(vec![
        message(tool_call(
            "call-1",
            "bash",
            serde_json::json!({"command": "cat page.html"}),
        )),
        message(tool_result("call-1", true, "<script>alert('x')</script>")),
    ]);

    let html = render_html(&export);

    assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"));
    assert!(!html.contains("<script>alert"));
}

#[test]
fn pairs_tool_results_with_their_calls() {
    let export = export_with_messages(vec![
        message(tool_call(
            "call-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        )),
        message(tool_result("call-1", true, "Cargo.toml\nsrc")),
    ]);

    let html = render_html(&export);

    assert_eq!(html.matches("<details class=\"tool\"").count(), 1);
    assert!(html.contains("<span class=\"tool-name\">bash</span>"));
    assert!(html.contains("Cargo.toml\nsrc"));
    assert!(html.contains("<span class=\"status ok\">ok</span>"));
}

#[test]
fn failed_tool_results_render_error_status() {
    let export = export_with_messages(vec![
        message(tool_call(
            "call-1",
            "bash",
            serde_json::json!({"command": "false"}),
        )),
        message(tool_result("call-1", false, "exit status 1")),
    ]);

    let html = render_html(&export);

    assert!(html.contains("<span class=\"status err\">error</span>"));
}

#[test]
fn orphan_tool_results_render_standalone() {
    let export = export_with_messages(vec![message(tool_result("call-9", true, "dangling"))]);

    let html = render_html(&export);

    assert_eq!(html.matches("<details class=\"tool\"").count(), 1);
    assert!(html.contains("<span class=\"tool-name\">tool result</span>"));
    assert!(html.contains("dangling"));
}

#[test]
fn long_tool_output_starts_collapsed() {
    let short_output = tool_result("call-1", true, "one line");
    let long_output = tool_result("call-2", true, &"line\n".repeat(200));
    let export = export_with_messages(vec![
        message(tool_call(
            "call-1",
            "bash",
            serde_json::json!({"command": "true"}),
        )),
        message(short_output),
        message(tool_call(
            "call-2",
            "bash",
            serde_json::json!({"command": "yes"}),
        )),
        message(long_output),
    ]);

    let html = render_html(&export);

    assert_eq!(html.matches("<details class=\"tool\" open>").count(), 1);
    assert_eq!(html.matches("<details class=\"tool\">").count(), 1);
}

#[test]
fn tool_preview_shows_primary_string_argument() {
    let export = export_with_messages(vec![message(tool_call(
        "call-1",
        "bash",
        serde_json::json!({"command": "cargo test --workspace"}),
    ))]);

    let html = render_html(&export);

    assert!(html.contains("<span class=\"tool-preview\">cargo test --workspace</span>"));
}

#[test]
fn tool_preview_falls_back_to_compact_json() {
    let export = export_with_messages(vec![message(tool_call(
        "call-1",
        "edit",
        serde_json::json!({"old": "a", "new": "b"}),
    ))]);

    let html = render_html(&export);

    assert!(html.contains("&quot;old&quot;"));
}

#[test]
fn consecutive_same_role_messages_share_one_role_head() {
    let export = export_with_messages(vec![
        message(Message::assistant_text("first thought")),
        message(Message::assistant_text("second thought")),
    ]);

    let html = render_html(&export);

    assert_eq!(html.matches("<span class=\"role\">rho</span>").count(), 1);
    assert_eq!(
        html.matches("<section class=\"entry assistant cont\">")
            .count(),
        1
    );
}

#[test]
fn role_head_returns_after_speaker_changes() {
    let export = export_with_messages(vec![
        message(Message::user_text("question one")),
        message(Message::assistant_text("answer")),
        message(Message::user_text("question two")),
    ]);

    let html = render_html(&export);

    assert_eq!(html.matches("<span class=\"role\">you</span>").count(), 2);
    assert_eq!(html.matches("<span class=\"role\">rho</span>").count(), 1);
}

#[test]
fn system_prompt_renders_in_collapsed_details() {
    let export = export_with_messages(vec![message(Message::System(
        "You are a helpful agent.".into(),
    ))]);

    let html = render_html(&export);

    assert!(html.contains("<details class=\"system\">"));
    assert!(html.contains("<summary>System prompt</summary>"));
    assert!(html.contains("You are a helpful agent."));
}

#[test]
fn images_render_as_data_uris() {
    let export = export_with_messages(vec![message(Message::User(vec![ContentBlock::Image(
        ImageContent {
            data: "aW1n".into(),
            mime_type: "image/png".into(),
        },
    )]))]);

    let html = render_html(&export);

    assert!(html.contains("src=\"data:image/png;base64,aW1n\""));
}

#[test]
fn message_timestamps_render_in_entry_heads() {
    let export = export_with_messages(vec![message(Message::user_text("hello"))]);

    let html = render_html(&export);

    assert!(html.contains("<time title="));
}

#[test]
fn resolve_output_path_defaults_to_cwd_with_short_id_name() {
    let cwd = PathBuf::from("/tmp/workspace");

    let path = resolve_output_path(&cwd, "", SESSION_ID);

    assert_eq!(path, cwd.join("rho-session-aaaaaaaa.html"));
}

#[test]
fn resolve_output_path_joins_relative_arguments_to_cwd() {
    let cwd = PathBuf::from("/tmp/workspace");

    let path = resolve_output_path(&cwd, "notes/transcript.html", SESSION_ID);

    assert_eq!(path, cwd.join("notes/transcript.html"));
}

#[test]
fn resolve_output_path_keeps_absolute_file_arguments() {
    let cwd = PathBuf::from("/tmp/workspace");

    let path = resolve_output_path(&cwd, "/tmp/out.html", SESSION_ID);

    assert_eq!(path, PathBuf::from("/tmp/out.html"));
}

#[test]
fn resolve_output_path_appends_default_name_inside_directories() {
    let dir = tempfile::tempdir().unwrap();

    let path = resolve_output_path(
        &PathBuf::from("/tmp/workspace"),
        &dir.path().display().to_string(),
        SESSION_ID,
    );

    assert_eq!(path, dir.path().join("rho-session-aaaaaaaa.html"));
}

#[test]
fn write_export_html_creates_parent_directories_and_writes_file() {
    let dir = tempfile::tempdir().unwrap();
    let export = export_with_messages(vec![message(Message::user_text("hello"))]);

    let path = write_export_html(dir.path(), "nested/dir/transcript.html", &export).unwrap();

    assert_eq!(path, dir.path().join("nested/dir/transcript.html"));
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.starts_with("<!DOCTYPE html>"));
    assert!(written.contains("hello"));
}

#[cfg(unix)]
#[test]
fn write_export_html_sets_private_permissions_for_new_and_existing_files() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let export = export_with_messages(vec![]);
    let path = dir.path().join("transcript.html");

    write_export_html(dir.path(), "transcript.html", &export).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    write_export_html(dir.path(), "transcript.html", &export).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
