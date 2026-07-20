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
fn agent_tools_use_status_first_presentations() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let id = ToolCallId::from_string("call-agent").unwrap();
    let arguments = serde_json::json!({
        "agent_id": "explorer",
        "background": true,
        "prompt": "Audit the repository\nfor architecture issues"
    });

    assert_eq!(
        presenter.preview(0, Some("agent".into()), &arguments.to_string()),
        Some(vec![
            "● explorer  starting in background".into(),
            "  Audit the repository for architecture issues".into(),
        ])
    );
    presenter.proposed(call(id.as_str(), "agent", arguments));
    let started = presenter.started(id.clone(), "agent".into(), ToolMetadata::default());
    assert_eq!(started.display_style, ToolDisplayStyle::default_tool());
    assert_eq!(
        started.display_lines,
        vec![
            "● explorer  starting in background",
            "  Audit the repository for architecture issues",
        ]
    );

    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text(
            "agent abc123 (explorer) started in background\nattach: rho attach abc123",
        )),
    );
    assert!(ok);
    assert_eq!(
        finished.display_lines,
        vec![
            "● explorer  running in background",
            "  Audit the repository for architecture issues",
            "",
            "  abc123 · rho attach abc123",
        ]
    );

    assert_eq!(
        presenter.preview(2, Some("agent".into()), r#"{"agent_id":"expl"#),
        Some(vec!["● expl  starting".into()])
    );
    assert_eq!(
        presenter.preview(
            2,
            None,
            r#"orer","prompt":"Trace module boundaries","background":true}"#,
        ),
        Some(vec![
            "● explorer  starting in background".into(),
            "  Trace module boundaries".into(),
        ])
    );

    let long_prompt = format!("{}tail marker", "architecture ".repeat(30));
    let long_preview = presenter
        .preview(
            1,
            Some("agent".into()),
            &serde_json::json!({"agent_id": "explorer", "prompt": long_prompt}).to_string(),
        )
        .unwrap();
    assert!(long_preview[1].ends_with('…'));
    assert!(!long_preview[1].contains("tail marker"));
}

#[test]
fn agent_progress_and_completion_keep_task_state_and_result_distinct() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let id = ToolCallId::from_string("call-agent-foreground").unwrap();
    presenter.proposed(call(
        id.as_str(),
        "agent",
        serde_json::json!({
            "agent_id": "reviewer",
            "prompt": "Review the change",
            "background": false
        }),
    ));
    presenter.started(id.clone(), "agent".into(), ToolMetadata::default());

    assert_eq!(
        presenter.updated(
            &id,
            &ToolProgress::message("agent def456 running\nattach: rho attach def456")
        ),
        vec![
            "● reviewer  running",
            "  Review the change",
            "",
            "  def456 · rho attach def456",
        ]
    );

    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text(
            "agent def456 (reviewer): ok\nturns: 3 · tokens: 1200 in / 300 out\n\nfirst paragraph\n\nsecond paragraph",
        )),
    );
    assert!(ok);
    assert_eq!(
        finished.display_lines,
        vec![
            "✓ reviewer  completed · 3 turns",
            "  Review the change",
            "",
            "  def456 · 1200 in / 300 out",
            "",
            "first paragraph",
            "",
            "second paragraph",
        ]
    );

    let failed = presenter.historical(
        &call(
            "call-agent-failed",
            "agent",
            serde_json::json!({"agent_id": "reviewer", "prompt": "Review the change"}),
        ),
        false,
        "agent def456 (reviewer): error\n\
         turns: 2 · tokens: 800 in / 120 out\n\
         error: provider stream failed\n\
         this delegated task did not complete; treat its work as unverified",
    );
    assert_eq!(
        failed.display_lines,
        vec![
            "✗ reviewer  failed · 2 turns",
            "  Review the change",
            "error: provider stream failed",
            "this delegated task did not complete; treat its work as unverified",
            "",
            "  def456 · 800 in / 120 out",
        ]
    );
}

#[test]
fn agents_list_and_status_share_the_agent_state_language() {
    let presenter = InteractiveToolPresenter::new("/workspace".into());
    let listed = presenter.historical(
        &call(
            "call-agents-list",
            "agents",
            serde_json::json!({"action": "list"}),
        ),
        true,
        "abc123  explorer  running  18s  Auditing repository structure\n\
         def456  reviewer  ok  51s  Review finished",
    );
    assert_eq!(
        listed.display_lines,
        vec![
            "delegated agents",
            "● abc123  explorer  running  18s  Auditing repository structure",
            "✓ def456  reviewer  completed  51s  Review finished",
        ]
    );

    let status = presenter.historical(
        &call(
            "call-agents-status",
            "agents",
            serde_json::json!({"action": "status", "id": "abc123"}),
        ),
        true,
        "agent abc123 (explorer): running\n\
         elapsed: 1m 30s · turns: 3 · tokens: 1200 in / 300 out\n\
         activity: searching files\n\
         latest: first paragraph\n\
         \n\
         second paragraph\n\
         attach: rho attach abc123",
    );
    assert_eq!(
        status.display_lines,
        vec![
            "● explorer  running · 1m 30s · 3 turns",
            "  searching files",
            "  first paragraph",
            "",
            "  second paragraph",
            "",
            "  abc123 · 1200 in / 300 out",
            "  rho attach abc123",
        ]
    );
}

#[test]
fn historical_legacy_agent_output_keeps_terminal_state() {
    let presenter = InteractiveToolPresenter::new("/workspace".into());
    let legacy_agent = call(
        "call-legacy-agent",
        "agent",
        serde_json::json!({"preset": "explorer", "prompt": "Map the repository", "background": true}),
    );
    let receipt = presenter.historical(
        &legacy_agent,
        true,
        "subagent abc123 (explorer) started in background\nattach: rho attach abc123",
    );
    assert_eq!(
        receipt.display_lines,
        vec![
            "● explorer  running in background",
            "  Map the repository",
            "",
            "  abc123 · rho attach abc123",
        ]
    );

    let completion = presenter.historical(
        &legacy_agent,
        true,
        "subagent abc123 (explorer): ok\n\
         turns: 2 · tokens: 900 in / 140 out\n\
         \n\
         legacy result",
    );
    assert_eq!(
        completion.display_lines,
        vec![
            "✓ explorer  completed · 2 turns",
            "  Map the repository",
            "",
            "  abc123 · 900 in / 140 out",
            "",
            "legacy result",
        ]
    );

    let status = presenter.historical(
        &call(
            "call-legacy-status",
            "agents",
            serde_json::json!({"action": "status", "id": "abc123"}),
        ),
        true,
        "subagent abc123 (explorer): running\n\
         elapsed: 12s · turns: 1 · tokens: 400 in / 60 out\n\
         activity: reading files\n\
         attach: rho attach abc123",
    );
    assert_eq!(
        status.display_lines,
        vec![
            "● explorer  running · 12s · 1 turn",
            "  reading files",
            "",
            "  abc123 · 400 in / 60 out",
            "  rho attach abc123",
        ]
    );

    let stopped = presenter.historical(
        &call(
            "call-legacy-stop",
            "agents",
            serde_json::json!({"action": "stop", "id": "abc123"}),
        ),
        true,
        "subagent abc123 (explorer): stopped\nturns: 1 · tokens: 400 in / 60 out",
    );
    assert_eq!(
        stopped.display_lines,
        vec![
            "■ explorer  stopped · 1 turn",
            "",
            "  abc123 · 400 in / 60 out",
        ]
    );

    let malformed = presenter.historical(
        &call(
            "call-malformed-status",
            "agents",
            serde_json::json!({"action": "status", "id": "abc123"}),
        ),
        true,
        "unrecognized status payload",
    );
    assert_eq!(
        malformed.display_lines,
        vec!["○ abc123  status result", "", "unrecognized status payload",]
    );

    let empty = presenter.historical(
        &call(
            "call-legacy-list",
            "agents",
            serde_json::json!({"action": "list"}),
        ),
        true,
        "no subagents",
    );
    assert_eq!(empty.display_lines, vec!["delegated agents", "  no runs"]);
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
