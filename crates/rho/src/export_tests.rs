use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;
use rho_providers::model::{ContentBlock, Message};

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
fn escapes_html_in_assistant_latex_and_tool_output() {
    let export = export_with_messages(vec![
        message(Message::assistant_text(
            "before <img src=x onerror=\"alert('x')\"> after\n\n$\\text{<script>alert('x')</script>}$\n\n$\\definitelyUnknown{<script>alert('x')</script>}$",
        )),
        message(tool_call(
            "call-1",
            "bash",
            serde_json::json!({"command": "cat page.html"}),
        )),
        message(tool_result("call-1", true, "<script>alert('x')</script>")),
    ]);

    let html = render_html(&export);

    assert!(html.contains("&lt;img src=x onerror=\"alert('x')\"&gt;"));
    assert!(!html.contains("<img src=x"));
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("&lt;script&gt;"));
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
fn pairs_tool_results_and_renders_orphans_standalone() {
    let export = export_with_messages(vec![
        message(tool_call(
            "call-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        )),
        message(tool_result("call-1", true, "Cargo.toml\nsrc")),
        message(tool_result("call-9", true, "dangling")),
    ]);

    let html = render_html(&export);

    assert_eq!(html.matches("<details class=\"tool\"").count(), 2);
    assert!(html.contains("Cargo.toml\nsrc"));
    assert!(html.contains("dangling"));
}

#[test]
fn message_timestamps_render_in_entry_heads() {
    let export = export_with_messages(vec![message(Message::user_text("hello"))]);

    let html = render_html(&export);

    assert!(html.contains("<time title="));
}

#[test]
fn resolve_output_path_covers_default_relative_absolute_and_directory() {
    let cwd = PathBuf::from("/tmp/workspace");
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(
        resolve_output_path(&cwd, "", SESSION_ID),
        cwd.join("rho-session-aaaaaaaa.html")
    );
    assert_eq!(
        resolve_output_path(&cwd, "notes/transcript.html", SESSION_ID),
        cwd.join("notes/transcript.html")
    );
    assert_eq!(
        resolve_output_path(&cwd, "/tmp/out.html", SESSION_ID),
        PathBuf::from("/tmp/out.html")
    );
    assert_eq!(
        resolve_output_path(
            &cwd,
            &dir.path().display().to_string(),
            SESSION_ID,
        ),
        dir.path().join("rho-session-aaaaaaaa.html")
    );
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

#[test]
fn preserves_ambiguous_currency_and_shell_variables() {
    let export = export_with_messages(vec![message(Message::assistant_text(
        "Costs range from $5-$10 and the path is $HOME/$PATH.",
    ))]);

    let html = render_html(&export);

    assert!(html.contains("Costs range from $5-$10 and the path is $HOME/$PATH."));
    assert!(!html.contains("<math"));
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
