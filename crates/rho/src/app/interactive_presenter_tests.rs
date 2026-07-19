use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ToolCall,
    tool::{OperationKind, ToolMetadata, ToolOutput, ToolProgress},
    ToolCallId, ToolCompletion,
};

use super::InteractiveToolPresenter;
use rho_tools::tool::ToolDisplayStyle;

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}

#[test]
fn shell_preview_uses_prompt_before_arguments_arrive() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());

    assert_eq!(
        presenter.preview(0, Some("bash".into()), ""),
        Some(vec!["$".into()])
    );
    assert_eq!(
        presenter.preview(1, Some("powershell".into()), ""),
        Some(vec!["PS".into()])
    );
}

#[test]
fn step_boundary_resets_streamed_previews_for_reused_indexes() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        presenter.preview(0, Some("bash".into()), r#"{"command":"cargo test"}"#),
        Some(vec!["$ cargo test".into()])
    );

    presenter.step_started();

    assert_eq!(
        presenter.preview(0, Some("bash".into()), r#"{"command":"cargo build"}"#),
        Some(vec!["$ cargo build".into()])
    );
}

#[test]
fn command_preview_and_result_preserve_command_summary() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        presenter.preview(
            0,
            Some("bash".into()),
            r#"{"command":"cargo test","timeout_seconds":30}"#,
        ),
        Some(vec!["$ cargo test".into()])
    );
    let id = ToolCallId::from_string("call-1").unwrap();
    presenter.proposed(call(
        id.as_str(),
        "bash",
        serde_json::json!({"command": "cargo test", "timeout_seconds": 30}),
    ));
    let started = presenter.started(id.clone(), "bash".into(), ToolMetadata::default());
    assert_eq!(started.command.as_deref(), Some("cargo test"));
    assert_eq!(started.display_style, ToolDisplayStyle::file_or_command());

    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text(
            "stdout:\ntests passed\n\nstderr:\nwarning\n\ntime: 0.1s  exit code: 0",
        )),
    );
    assert!(ok);
    assert_eq!(
        finished.display_lines,
        vec!["$ cargo test", "timeout: 30s", "", "tests passed"]
    );
}

#[test]
fn shell_result_preserves_stderr_like_stdout_and_timeout_notice() {
    let presenter = InteractiveToolPresenter::new("/workspace".into());
    let call = call(
        "call-timeout",
        "bash",
        serde_json::json!({"command": "slow-command", "timeout_seconds": 5}),
    );
    let finished = presenter.historical(
        &call,
        /*ok*/ false,
        "command timed out after 5s\n\nstdout:\na\n\nstderr:\nb\n\nstderr:\nwarning",
    );

    assert_eq!(
        finished.display_lines,
        vec![
            "$ slow-command",
            "timeout: 5s",
            "command timed out after 5s",
            "",
            "a\n\nstderr:\nb",
        ]
    );
}

#[test]
fn file_results_use_structured_paths_and_compact_diff() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let id = ToolCallId::from_string("call-edit").unwrap();
    presenter.proposed(call(
        id.as_str(),
        "edit_file",
        serde_json::json!({"edits": [
            {"path": "src/lib.rs", "old_string": "old", "new_string": "new"},
            {"path": "src/main.rs", "old_string": "before", "new_string": "after"}
        ]}),
    ));
    presenter.started(id.clone(), "edit_file".into(), ToolMetadata::default());
    let metadata = ToolMetadata::new()
        .operation(OperationKind::Write)
        .affected_path("src/lib.rs")
        .affected_path("src/main.rs")
        .diff("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-before\n+after\n");
    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text("raw diff").metadata(metadata)),
    );

    assert!(ok);
    assert_eq!(finished.display_style, ToolDisplayStyle::file_diff());
    assert_eq!(
        finished.display_lines,
        vec![
            "edit_file src/lib.rs, src/main.rs",
            "src/lib.rs\n-old\n+new\n\nsrc/main.rs\n-before\n+after"
        ]
    );
}

#[test]
fn web_skill_progress_and_unknown_tools_have_explicit_presentations() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let web_id = ToolCallId::from_string("call-web").unwrap();
    presenter.proposed(call(
        web_id.as_str(),
        "web_search",
        serde_json::json!({"queries": ["rust tui"]}),
    ));
    presenter.started(web_id.clone(), "web_search".into(), ToolMetadata::default());
    let (_, web) = presenter.finished(
        &web_id,
        ToolCompletion::Success(ToolOutput::text(
            serde_json::json!({"answer": "first\nsecond"}).to_string(),
        )),
    );
    assert_eq!(web.display_style, ToolDisplayStyle::web());
    assert_eq!(
        web.display_lines,
        vec!["web search: 2 results stored for \"rust tui\""]
    );

    let skill_id = ToolCallId::from_string("call-skill").unwrap();
    presenter.proposed(call(
        skill_id.as_str(),
        "skill",
        serde_json::json!({"name": "rho-tui-herdr-testing"}),
    ));
    let skill = presenter.started(skill_id, "skill".into(), ToolMetadata::default());
    assert_eq!(skill.display_style, ToolDisplayStyle::skill());
    assert_eq!(skill.display_lines, vec!["skill rho-tui-herdr-testing"]);

    let unknown_id = ToolCallId::from_string("call-custom").unwrap();
    presenter.proposed(call(
        unknown_id.as_str(),
        "custom",
        serde_json::json!({"value": 1}),
    ));
    presenter.started(unknown_id.clone(), "custom".into(), ToolMetadata::default());
    let progress = ToolProgress::message("halfway")
        .units(1, 2)
        .metadata(ToolMetadata::new().operation(OperationKind::Execute));
    assert_eq!(
        presenter.updated(&unknown_id, &progress),
        vec!["custom", "halfway", "progress: 1/2"]
    );
}

#[test]
fn exact_tool_names_do_not_use_suffix_inference() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let id = ToolCallId::from_string("call-custom").unwrap();
    presenter.proposed(call(
        id.as_str(),
        "custom_read_file",
        serde_json::json!({"path": "secret"}),
    ));
    let started = presenter.started(id, "custom_read_file".into(), ToolMetadata::default());

    assert_eq!(started.display_style, ToolDisplayStyle::default_tool());
    assert_eq!(started.display_lines, vec!["custom_read_file"]);
}
