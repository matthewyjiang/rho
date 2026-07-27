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
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    }
}

fn preview_lines(
    presenter: &mut InteractiveToolPresenter,
    index: usize,
    name: Option<String>,
    arguments_delta: &str,
) -> Option<Vec<String>> {
    presenter
        .preview(index, name, arguments_delta)
        .map(|presented| presented.display_lines)
}

#[test]
fn shell_preview_uses_prompt_before_arguments_arrive() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());

    assert_eq!(
        preview_lines(&mut presenter, 0, Some("bash".to_string()), ""),
        Some(vec!["● $".to_string()])
    );
    assert_eq!(
        preview_lines(&mut presenter, 1, Some("powershell".to_string()), ""),
        Some(vec!["● PS".to_string()])
    );
}

#[test]
fn step_boundary_resets_streamed_previews_for_reused_indexes() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_lines(
            &mut presenter,
            0,
            Some("bash".to_string()),
            r#"{"command":"cargo test"}"#
        ),
        Some(vec!["● $ cargo test".to_string()])
    );

    presenter.step_started();

    assert_eq!(
        preview_lines(
            &mut presenter,
            0,
            Some("bash".to_string()),
            r#"{"command":"cargo build"}"#
        ),
        Some(vec!["● $ cargo build".to_string()])
    );
}

#[test]
fn command_preview_and_result_preserve_command_summary() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_lines(
            &mut presenter,
            0,
            Some("bash".to_string()),
            r#"{"command":"cargo test","timeout_seconds":30}"#,
        ),
        Some(vec!["● $ cargo test".to_string()])
    );
    let id = ToolCallId::from_string("call-1").unwrap();
    presenter.proposed(call(
        id.as_str(),
        "bash",
        serde_json::json!({"command": "cargo test", "timeout_seconds": 30}),
    ));
    let started = presenter.started(id.clone(), "bash".to_string(), ToolMetadata::default());
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
        vec![
            "✓ $ cargo test".to_string(),
            "  ├ timeout 30s".to_string(),
            "  ├ exit 0 · 0.1s".to_string(),
            "".to_string(),
            "tests passed".to_string(),
        ]
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
            "✗ $ slow-command".to_string(),
            "  ├ timeout 5s".to_string(),
            "  ├ command timed out after 5s".to_string(),
            "".to_string(),
            "a".to_string(),
            "".to_string(),
            "stderr:".to_string(),
            "b".to_string(),
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
    presenter.started(id.clone(), "edit_file".to_string(), ToolMetadata::default());
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
    assert_eq!(finished.display_lines[0], "✓ edit_file(2 files)");
    assert!(finished
        .display_lines
        .iter()
        .any(|line| line.contains("+1 -1 lines | src/lib.rs")));
    assert!(finished
        .display_lines
        .iter()
        .any(|line| line.contains("+1 -1 lines | src/main.rs")));
    assert_eq!(finished.card.facts.len(), 2);
    assert!(finished.card.body.is_diff());
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
    presenter.started(
        web_id.clone(),
        "web_search".to_string(),
        ToolMetadata::default(),
    );
    let (_, web) = presenter.finished(
        &web_id,
        ToolCompletion::Success(ToolOutput::text(
            serde_json::json!({"answer": "first\nsecond"}).to_string(),
        )),
    );
    assert_eq!(web.display_style, ToolDisplayStyle::web());
    assert_eq!(
        web.display_lines,
        vec![
            "✓ web_search(\"rust tui\")".to_string(),
            "  └ 2 results stored".to_string(),
        ]
    );

    let skill_id = ToolCallId::from_string("call-skill").unwrap();
    presenter.proposed(call(
        skill_id.as_str(),
        "skill",
        serde_json::json!({"name": "rho-tui-herdr-testing"}),
    ));
    let skill = presenter.started(skill_id, "skill".to_string(), ToolMetadata::default());
    assert_eq!(skill.display_style, ToolDisplayStyle::skill());
    assert_eq!(
        skill.display_lines,
        vec!["● skill(rho-tui-herdr-testing)".to_string()]
    );

    let unknown_id = ToolCallId::from_string("call-custom").unwrap();
    presenter.proposed(call(
        unknown_id.as_str(),
        "custom",
        serde_json::json!({"value": 1}),
    ));
    presenter.started(
        unknown_id.clone(),
        "custom".to_string(),
        ToolMetadata::default(),
    );
    let progress = ToolProgress::message("halfway")
        .units(1, 2)
        .metadata(ToolMetadata::new().operation(OperationKind::Execute));
    assert_eq!(
        presenter.updated(&unknown_id, &progress).display_lines,
        vec![
            "● custom".to_string(),
            "  ├ 1/2".to_string(),
            "".to_string(),
            "halfway".to_string(),
        ]
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

    // Streaming may keep multi-line prompt fragments; require header + task content.
    let preview = preview_lines(
        &mut presenter,
        0,
        Some("agent".to_string()),
        &arguments.to_string(),
    )
    .unwrap();
    assert_eq!(preview[0], "● explorer  starting in background");
    assert!(preview
        .iter()
        .any(|line| line.contains("Audit the repository")));

    presenter.proposed(call(id.as_str(), "agent", arguments));
    let started = presenter.started(id.clone(), "agent".to_string(), ToolMetadata::default());
    assert_eq!(started.display_style, ToolDisplayStyle::default_tool());
    assert_eq!(
        started.display_lines,
        vec![
            "● explorer  starting in background".to_string(),
            "  └ Audit the repository for architecture issues".to_string(),
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
        finished.display_lines[0],
        "● explorer  running in background"
    );
    assert!(finished
        .display_lines
        .iter()
        .any(|line| line.contains("Audit the repository for architecture issues")));
    assert!(finished
        .display_lines
        .iter()
        .any(|line| line.contains("abc123 · rho attach abc123")));

    assert_eq!(
        preview_lines(
            &mut presenter,
            2,
            Some("agent".to_string()),
            r#"{"agent_id":"expl"#
        ),
        Some(vec!["● expl  starting".to_string()])
    );
    assert_eq!(
        preview_lines(
            &mut presenter,
            2,
            None,
            r#"orer","prompt":"Trace module boundaries","background":true}"#,
        ),
        Some(vec![
            "● explorer  starting in background".to_string(),
            "  └ Trace module boundaries".to_string(),
        ])
    );

    let long_prompt = format!("{}tail marker", "architecture ".repeat(30));
    let long_preview = presenter
        .preview(
            1,
            Some("agent".to_string()),
            &serde_json::json!({"agent_id": "explorer", "prompt": long_prompt}).to_string(),
        )
        .unwrap();
    let prompt_line = long_preview
        .display_lines
        .iter()
        .find(|line| line.contains("tail marker"))
        .cloned()
        .expect("streaming agent previews should keep the live prompt tail visible");
    assert!(
        prompt_line.contains('…') || prompt_line.contains("tail marker"),
        "long streaming prompts should mark omitted leading text: {long_preview:?}"
    );

    // Compact start/finish summaries still prefer a short prefix, not the live tail.
    let started_long = presenter.proposed(call(
        "call-agent-long",
        "agent",
        serde_json::json!({
            "agent_id": "explorer",
            "prompt": format!("{}tail marker", "architecture ".repeat(30)),
        }),
    ));
    let started_task = started_long
        .display_lines
        .iter()
        .find(|line| line.contains('…') || line.contains("architecture"))
        .expect("task fact");
    assert!(started_task.ends_with('…') || started_task.contains('…'));
    assert!(!started_task.contains("tail marker"));
}

#[test]
fn agent_prompt_streaming_keeps_updating_past_the_compact_summary_window() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_lines(
            &mut presenter,
            0,
            Some("agent".to_string()),
            r#"{"agent_id":"explorer","prompt":""#
        ),
        Some(vec!["● explorer  starting".to_string()])
    );

    let mut last_prompt_line = None;
    let mut updates = 0;
    for chunk in std::iter::repeat_n("delegated task context ", 40) {
        if let Some(lines) = preview_lines(&mut presenter, 0, None, chunk) {
            updates += 1;
            let prompt_line = lines
                .iter()
                .find(|line| line.contains("delegated") || line.contains("task"))
                .cloned()
                .unwrap_or_default();
            if let Some(previous) = last_prompt_line.replace(prompt_line.clone()) {
                assert_ne!(
                    previous, prompt_line,
                    "each emitted agent preview should advance the live prompt text"
                );
            }
            assert!(
                prompt_line.contains("delegated") || prompt_line.contains("task"),
                "{lines:?}"
            );
        }
    }
    assert!(
        updates >= 8,
        "long agent prompts should emit many streaming previews, got {updates}"
    );
}

#[test]
fn agent_streaming_preview_ignores_field_names_inside_prompt_text() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let raw = r#"{"prompt":"discuss \"agent_id\":\"forged\" values","agent_id":"explorer","background":true}"#;
    assert_eq!(
        preview_lines(&mut presenter, 0, Some("agent".to_string()), raw),
        Some(vec![
            "● explorer  starting in background".to_string(),
            r#"  └ discuss "agent_id":"forged" values"#.to_string(),
        ])
    );
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
    presenter.started(id.clone(), "agent".to_string(), ToolMetadata::default());

    let progress = presenter.updated(
        &id,
        &ToolProgress::message("agent def456 running\nattach: rho attach def456"),
    );
    assert_eq!(progress.display_lines[0], "● reviewer  running");
    assert!(progress
        .display_lines
        .iter()
        .any(|line| line.contains("Review the change")));
    assert!(progress
        .display_lines
        .iter()
        .any(|line| line.contains("def456 · rho attach def456")));

    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text(
            "agent def456 (reviewer): ok\nturns: 3 · tokens: 1200 in / 300 out\n\nfirst paragraph\n\nsecond paragraph",
        )),
    );
    assert!(ok);
    assert!(finished.display_lines[0].starts_with("✓ reviewer  completed"));
    assert!(finished
        .display_lines
        .iter()
        .any(|line| line.contains("Review the change")));
    assert!(finished
        .display_lines
        .iter()
        .any(|line| line.contains("def456")));
    assert!(finished
        .display_lines
        .iter()
        .any(|line| line.contains("first paragraph")));

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
    assert!(failed.display_lines[0].starts_with("✗ reviewer  failed"));
    assert!(failed
        .display_lines
        .iter()
        .any(|line| line.contains("error: provider stream failed")));
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
    assert_eq!(listed.display_lines[0], "✓ delegated agents");
    assert!(listed
        .display_lines
        .iter()
        .any(|line| line.contains("abc123") && line.contains("running")));
    assert!(listed
        .display_lines
        .iter()
        .any(|line| line.contains("def456") && line.contains("completed")));

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
    assert!(status.display_lines[0].starts_with("● explorer  running"));
    assert!(status
        .display_lines
        .iter()
        .any(|line| line.contains("searching files")));
    assert!(status
        .display_lines
        .iter()
        .any(|line| line.contains("abc123")));
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
        receipt.display_lines[0],
        "● explorer  running in background"
    );
    assert!(receipt
        .display_lines
        .iter()
        .any(|line| line.contains("Map the repository")));
    assert!(receipt
        .display_lines
        .iter()
        .any(|line| line.contains("abc123 · rho attach abc123")));

    let completion = presenter.historical(
        &legacy_agent,
        true,
        "subagent abc123 (explorer): ok\n\
         turns: 2 · tokens: 900 in / 140 out\n\
         \n\
         legacy result",
    );
    assert!(completion.display_lines[0].starts_with("✓ explorer  completed"));
    assert!(completion
        .display_lines
        .iter()
        .any(|line| line.contains("legacy result")));

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
    assert!(status.display_lines[0].starts_with("● explorer  running"));
    assert!(status
        .display_lines
        .iter()
        .any(|line| line.contains("reading files")));

    let stopped = presenter.historical(
        &call(
            "call-legacy-stop",
            "agents",
            serde_json::json!({"action": "stop", "id": "abc123"}),
        ),
        true,
        "subagent abc123 (explorer): stopped\nturns: 1 · tokens: 400 in / 60 out",
    );
    assert!(stopped.display_lines[0].contains("explorer"));
    assert!(stopped.display_lines[0].contains("stopped"));

    let malformed = presenter.historical(
        &call(
            "call-malformed-status",
            "agents",
            serde_json::json!({"action": "status", "id": "abc123"}),
        ),
        true,
        "unrecognized status payload",
    );
    assert!(malformed.display_lines[0].contains("abc123"));
    assert!(malformed
        .display_lines
        .iter()
        .any(|line| line.contains("unrecognized status payload")));

    let empty = presenter.historical(
        &call(
            "call-legacy-list",
            "agents",
            serde_json::json!({"action": "list"}),
        ),
        true,
        "no subagents",
    );
    assert_eq!(empty.display_lines[0], "✓ delegated agents");
    assert!(empty
        .display_lines
        .iter()
        .any(|line| line.contains("no runs")));
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
    let started = presenter.started(id, "custom_read_file".to_string(), ToolMetadata::default());

    assert_eq!(started.display_style, ToolDisplayStyle::default_tool());
    assert_eq!(
        started.display_lines,
        vec!["● custom_read_file".to_string()]
    );
}

#[test]
fn edit_failure_uses_error_fact_not_diff_body() {
    let presenter = InteractiveToolPresenter::new("/workspace".into());
    let finished = presenter.historical(
        &call(
            "call-edit-fail",
            "edit_file",
            serde_json::json!({"edits": [
                {"path": "theme.rs", "old_string": "missing", "new_string": "new"}
            ]}),
        ),
        /*ok*/ false,
        "no matches found for old_string",
    );
    assert_eq!(finished.display_lines[0], "✗ edit_file(theme.rs)");
    assert!(finished
        .display_lines
        .iter()
        .any(|line| line.contains("no matches found for old_string")));
}

#[test]
fn shell_progress_streams_stdout_into_card_body() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let id = ToolCallId::from_string("call-shell-progress").unwrap();
    presenter.proposed(call(
        id.as_str(),
        "bash",
        serde_json::json!({"command": "cargo test", "timeout_seconds": 30}),
    ));
    presenter.started(id.clone(), "bash".to_string(), ToolMetadata::default());

    let progress = presenter.updated(
        &id,
        &ToolProgress::message(
            "stdout:\ncompiling rho\nrunning 12 tests\n\nstderr:\n\n\ntime: running",
        ),
    );
    assert_eq!(progress.display_lines[0], "● $ cargo test");
    assert!(
        progress
            .display_lines
            .iter()
            .any(|line| line.contains("compiling rho")),
        "stdout should stream into the tool display: {:?}",
        progress.display_lines
    );
    assert!(
        progress
            .display_lines
            .iter()
            .any(|line| line.contains("running 12 tests")),
        "multi-line stdout should stream: {:?}",
        progress.display_lines
    );
    match &progress.card.body {
        rho_tools::tool_card::ToolBody::Lines(lines) => {
            assert!(lines.iter().any(|line| line.contains("compiling rho")));
            assert!(lines.iter().any(|line| line.contains("running 12 tests")));
        }
        other => panic!("expected body lines, got {other:?}"),
    }
}

#[test]
fn background_agent_finish_keeps_running_card_status() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let id = ToolCallId::from_string("call-bg").unwrap();
    presenter.proposed(call(
        id.as_str(),
        "agent",
        serde_json::json!({
            "agent_id": "worker",
            "background": true,
            "prompt": "fixture stream"
        }),
    ));
    presenter.started(id.clone(), "agent".to_string(), ToolMetadata::default());
    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text(
            "agent abc123 (worker) started in background\nattach: rho attach abc123",
        )),
    );
    assert!(ok);
    assert_eq!(
        finished.card.status,
        rho_tools::tool_card::ToolStatus::Running
    );
    assert!(finished.display_lines[0].starts_with("● worker  running in background"));
}

#[test]
fn bash_argument_streaming_updates_command_in_preview() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_lines(
            &mut presenter,
            0,
            Some("bash".to_string()),
            r#"{"command":""#
        ),
        Some(vec!["● $".to_string()])
    );

    // Backoff is exponential on parse length; send large enough chunks to cross
    // the stride so the live command keeps advancing.
    let mut last_header = String::new();
    let mut updates = 0usize;
    for chunk in [
        r#"cargo test -p rho"#,
        r#" --features all-features -- --nocapture"#,
        r#" long_tail_marker"#,
    ] {
        if let Some(lines) = preview_lines(&mut presenter, 0, None, chunk) {
            updates += 1;
            let header = lines[0].clone();
            assert!(
                header.starts_with("● $"),
                "streamed bash preview should keep shell header: {lines:?}"
            );
            assert_ne!(header, last_header, "each emit should advance the command");
            last_header = header;
        }
    }
    assert!(
        updates >= 2,
        "bash command streaming should emit multiple previews, got {updates}; last={last_header}"
    );
    assert!(
        last_header.contains("cargo test -p rho") || last_header.contains("long_tail_marker"),
        "final streamed command missing expected text: {last_header}"
    );
}
