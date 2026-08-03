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

// Covers: path resolution picks format from extension and keeps workspace writes explicit.
// Owner: pure unit
#[test]
fn resolve_output_target_covers_extension_relative_absolute_and_directory() {
    let home = tempfile::tempdir().unwrap();
    let export_dir = home.path().join("exports");

    let cwd = PathBuf::from("/tmp/workspace");
    let dir = tempfile::tempdir().unwrap();
    let export = export_with_messages(vec![]);
    let stamp = format_file_stamp(export.updated_at);
    let default_name = format!("rho-session-aaaaaaaa-{stamp}-fix-the-login-bug.html");

    let (default_path, default_format) = resolve_output_target_with(
        &cwd,
        &ExportWriteOptions {
            path_arg: "",
            format: None,
            force: false,
        },
        &export,
        || Ok(export_dir.clone()),
    )
    .unwrap();
    assert_eq!(default_format, ExportFormat::Html);
    assert_eq!(default_path, export_dir.join(&default_name));

    let (md_path, md_format) = resolve_output_target(
        &cwd,
        &ExportWriteOptions {
            path_arg: "notes/transcript.md",
            format: None,
            force: false,
        },
        &export,
    )
    .unwrap();
    assert_eq!(md_format, ExportFormat::Markdown);
    assert_eq!(md_path, cwd.join("notes/transcript.md"));

    let (abs_path, abs_format) = resolve_output_target(
        &cwd,
        &ExportWriteOptions {
            path_arg: "/tmp/out.json",
            format: None,
            force: false,
        },
        &export,
    )
    .unwrap();
    assert_eq!(abs_format, ExportFormat::Json);
    assert_eq!(abs_path, PathBuf::from("/tmp/out.json"));

    let (dir_path, dir_format) = resolve_output_target(
        &cwd,
        &ExportWriteOptions {
            path_arg: &dir.path().display().to_string(),
            format: Some(ExportFormat::Markdown),
            force: false,
        },
        &export,
    )
    .unwrap();
    assert_eq!(dir_format, ExportFormat::Markdown);
    assert_eq!(
        dir_path,
        dir.path()
            .join(format!("rho-session-aaaaaaaa-{stamp}-fix-the-login-bug.md"))
    );

    let mismatch = resolve_output_target(
        &cwd,
        &ExportWriteOptions {
            path_arg: "notes.md",
            format: Some(ExportFormat::Html),
            force: false,
        },
        &export,
    );
    assert!(mismatch.is_err());

    let unknown_with_format = resolve_output_target(
        &cwd,
        &ExportWriteOptions {
            path_arg: "notes.txt",
            format: Some(ExportFormat::Html),
            force: false,
        },
        &export,
    );
    assert!(unknown_with_format.is_err());

    let (stem_path, stem_format) = resolve_output_target(
        &cwd,
        &ExportWriteOptions {
            path_arg: "notes",
            format: Some(ExportFormat::Markdown),
            force: false,
        },
        &export,
    )
    .unwrap();
    assert_eq!(stem_format, ExportFormat::Markdown);
    assert_eq!(stem_path, cwd.join("notes.md"));

    let unknown = resolve_output_target(
        &cwd,
        &ExportWriteOptions {
            path_arg: "notes.txt",
            format: None,
            force: false,
        },
        &export,
    );
    assert!(unknown.is_err());
}

// Covers: markdown/json renderers keep tool pairing and refuse silent overwrite.
// Owner: pure unit
#[test]
fn markdown_and_json_export_and_overwrite_guard() {
    let export = export_with_messages(vec![
        message(Message::user_text("please list files")),
        message(tool_call(
            "call-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        )),
        message(tool_result("call-1", true, "Cargo.toml")),
    ]);

    let markdown = render_export(&export, ExportFormat::Markdown).unwrap();
    assert!(markdown.contains("# Fix the login bug"));
    assert!(markdown.contains("## You"));
    assert!(markdown.contains("please list files"));
    assert!(markdown.contains("### `bash` (ok)"));
    assert!(markdown.contains("Cargo.toml"));

    let json = render_export(&export, ExportFormat::Json).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["id"], SESSION_ID);
    assert_eq!(value["title"], "Fix the login bug");
    assert_eq!(value["cwd"], "example-workspace");
    assert_eq!(value["messages"].as_array().unwrap().len(), 3);
    let cwd = value["cwd"].as_str().unwrap();
    assert!(!cwd.contains('/'));
    assert!(!cwd.contains('\\'));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("once.md");
    write_export(
        dir.path(),
        &ExportWriteOptions {
            path_arg: "once.md",
            format: None,
            force: false,
        },
        &export,
    )
    .unwrap();
    assert!(path.is_file());
    let refused = write_export(
        dir.path(),
        &ExportWriteOptions {
            path_arg: "once.md",
            format: None,
            force: false,
        },
        &export,
    );
    assert!(refused.is_err());
    write_export(
        dir.path(),
        &ExportWriteOptions {
            path_arg: "once.md",
            format: None,
            force: true,
        },
        &export,
    )
    .unwrap();
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
