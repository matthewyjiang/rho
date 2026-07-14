use super::{display_lines, parse_partial_json, StreamedToolCallPreview};
use crate::{
    tool::{ToolContext, ToolRegistry},
    tools::{edit_file::EditFile, read_file::ReadFile},
};

fn context() -> ToolContext {
    ToolContext {
        cwd: std::env::current_dir().unwrap(),
        max_output_bytes: 12_000,
    }
}

#[test]
fn displays_argument_values_before_json_is_complete() {
    let mut tools = ToolRegistry::new();
    tools.register(ReadFile);

    assert_eq!(
        display_lines(
            Some("read_file"),
            r#"{"path":"src/main"#,
            &tools,
            &context(),
        ),
        vec!["read_file", "src/main"]
    );
    assert_eq!(
        display_lines(
            Some("read_file"),
            r#"{"path":"src/main.rs","offset":"#,
            &tools,
            &context(),
        ),
        vec!["read_file", "src/main.rs"]
    );
}

#[test]
fn name_only_previews_do_not_parse_large_streamed_values() {
    let mut tools = ToolRegistry::new();
    tools.register(EditFile);
    let arguments = format!(
        r#"{{"edits":[{{"path":"src/main.rs","new_string":"{}"#,
        "x".repeat(100_000)
    );

    assert_eq!(
        display_lines(Some("edit_file"), &arguments, &tools, &context()),
        vec!["edit_file"]
    );
}

#[test]
fn argument_previews_are_throttled_as_the_stream_grows() {
    let mut tools = ToolRegistry::new();
    tools.register(ReadFile);
    let mut preview = StreamedToolCallPreview::default();

    assert_eq!(
        preview.update(Some("read_file"), r#"{"path":"a"#, &tools, &context(),),
        Some(vec!["read_file".into(), "a".into()])
    );
    assert_eq!(
        preview.update(Some("read_file"), r#"{"path":"abc"#, &tools, &context(),),
        None
    );
    assert_eq!(
        preview.update(
            Some("read_file"),
            r#"{"path":"abcdefghijklmnop"#,
            &tools,
            &context(),
        ),
        Some(vec!["read_file".into(), "abcdefghijklmnop".into()])
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn command_preview_uses_streamed_arguments() {
    let mut tools = ToolRegistry::new();
    tools.register(crate::tools::bash::Bash::new(/*rtk_enabled*/ false));

    assert_eq!(
        display_lines(Some("bash"), r#"{"command":"cargo te"#, &tools, &context(),),
        vec!["bash cargo te"]
    );
}

#[cfg(windows)]
#[test]
fn command_preview_uses_streamed_arguments() {
    let mut tools = ToolRegistry::new();
    tools.register(crate::tools::powershell::PowerShell::new(
        /*rtk_enabled*/ false,
    ));

    assert_eq!(
        display_lines(
            Some("powershell"),
            r#"{"command":"cargo te"#,
            &tools,
            &context(),
        ),
        vec!["powershell cargo te"]
    );
}

#[test]
fn completes_streaming_strings_and_nested_values() {
    assert_eq!(
        parse_partial_json(r#"{"path":"src/main"#),
        Some(serde_json::json!({"path": "src/main"}))
    );
    assert_eq!(
        parse_partial_json(r#"{"edits":[{"path":"src/main.rs","new_string":"hel"#),
        Some(serde_json::json!({
            "edits": [{"path": "src/main.rs", "new_string": "hel"}]
        }))
    );
}

#[test]
fn keeps_complete_fields_when_the_next_value_has_not_started() {
    assert_eq!(
        parse_partial_json(r#"{"path":"src/main.rs","offset":"#),
        Some(serde_json::json!({"path": "src/main.rs"}))
    );
}

#[test]
fn preserves_partial_escape_sequences() {
    assert_eq!(
        parse_partial_json(r#"{"content":"line\nnext\\"#),
        Some(serde_json::json!({"content": "line\nnext\\"}))
    );
}
