use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ToolCall,
    tool::{OperationKind, ToolMetadata, ToolOutput, ToolProgress},
    ToolCallId, ToolCompletion,
};
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};

use super::{InteractiveToolPresenter, ToolPresentation};

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    }
}

fn header(presented: &ToolPresentation) -> String {
    presented.card.header_text()
}

fn preview_header(
    presenter: &mut InteractiveToolPresenter,
    index: usize,
    name: Option<String>,
    arguments_delta: &str,
) -> Option<String> {
    presenter
        .preview(index, name, arguments_delta)
        .map(|presented| header(&presented))
}

fn preview_card(
    presenter: &mut InteractiveToolPresenter,
    index: usize,
    name: Option<String>,
    arguments_delta: &str,
) -> Option<ToolCard> {
    presenter
        .preview(index, name, arguments_delta)
        .map(|presented| presented.card)
}

fn card_contains(card: &ToolCard, needle: &str) -> bool {
    if card.header_text().contains(needle) {
        return true;
    }
    if card
        .facts
        .iter()
        .any(|fact| fact.plain_text().contains(needle))
    {
        return true;
    }
    match &card.body {
        ToolBody::None => false,
        ToolBody::Lines(lines) | ToolBody::DiffLines(lines) => {
            lines.iter().any(|line| line.contains(needle))
        }
    }
}

fn fact_texts(card: &ToolCard) -> Vec<String> {
    card.facts.iter().map(ToolFact::plain_text).collect()
}

#[test]
fn shell_preview_uses_prompt_before_arguments_arrive() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());

    assert_eq!(
        preview_header(&mut presenter, 0, Some("bash".to_string()), ""),
        Some("● $".to_string())
    );
    assert_eq!(
        preview_header(&mut presenter, 1, Some("powershell".to_string()), ""),
        Some("● PS".to_string())
    );
}

#[test]
fn step_boundary_resets_streamed_previews_for_reused_indexes() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_header(
            &mut presenter,
            0,
            Some("bash".to_string()),
            r#"{"command":"cargo test"}"#
        ),
        Some("● $ cargo test".to_string())
    );

    presenter.step_started();

    assert_eq!(
        preview_header(
            &mut presenter,
            0,
            Some("bash".to_string()),
            r#"{"command":"cargo build"}"#
        ),
        Some("● $ cargo build".to_string())
    );
}

#[test]
fn shell_previews_track_each_streamed_argument_delta() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_header(
            &mut presenter,
            0,
            Some("bash".to_string()),
            r#"{"command":""#
        ),
        Some("● $".to_string())
    );

    let mut headers = Vec::new();
    for delta in ["cargo", " test", " --all", "\"}"] {
        if let Some(text) = preview_header(&mut presenter, 0, None, delta) {
            headers.push(text);
        }
    }

    // Every delta that changes the visible command renders; the closing delta
    // adds nothing new, so it is suppressed instead of repeating the card.
    assert_eq!(
        headers,
        vec![
            "● $ cargo".to_string(),
            "● $ cargo test".to_string(),
            "● $ cargo test --all".to_string(),
        ]
    );
}

#[test]
fn file_write_preview_shows_its_path_while_arguments_stream() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_header(
            &mut presenter,
            0,
            Some("write_file".to_string()),
            r#"{"path":"notes.md""#
        ),
        Some("● write_file(notes.md)".to_string())
    );
    assert_eq!(
        preview_header(&mut presenter, 0, None, r#","content":"first line"#),
        None,
        "a streamed file body must not churn the card once the path is known"
    );
}

#[test]
fn command_preview_and_result_preserve_command_summary() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_header(
            &mut presenter,
            0,
            Some("bash".to_string()),
            r#"{"command":"cargo test","timeout_seconds":30}"#,
        ),
        Some("● $ cargo test".to_string())
    );
    let id = ToolCallId::from_string("call-1").unwrap();
    presenter.proposed(call(
        id.as_str(),
        "bash",
        serde_json::json!({"command": "cargo test", "timeout_seconds": 30}),
    ));
    let started = presenter.started(id.clone(), "bash".to_string(), ToolMetadata::default());
    assert_eq!(started.card.family, ToolFamily::FileCommand);
    assert_eq!(started.card.status, ToolStatus::Running);
    assert_eq!(
        started.card.header,
        ToolHeader::shell("$", Some("cargo test".into()))
    );

    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text(
            "stdout:\ntests passed\n\nstderr:\nwarning\n\ntime: 0.1s  exit code: 0",
        )),
    );
    assert!(ok);
    assert_eq!(finished.card.family, ToolFamily::FileCommand);
    assert_eq!(finished.card.status, ToolStatus::Ok);
    assert_eq!(
        finished.card.header,
        ToolHeader::shell("$", Some("cargo test".into()))
    );
    assert_eq!(
        finished.card.facts,
        vec![
            ToolFact::Meta {
                text: "timeout 30s".into()
            },
            ToolFact::Exit {
                code: 0,
                duration_ms: Some(100),
            },
        ]
    );
    assert_eq!(
        finished.card.body,
        ToolBody::Lines(vec!["tests passed".into()])
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

    assert_eq!(header(&finished), "✗ $ slow-command");
    assert_eq!(
        finished.card.facts,
        vec![
            ToolFact::Meta {
                text: "timeout 5s".into()
            },
            ToolFact::Error {
                text: "command timed out after 5s".into()
            },
        ]
    );
    assert_eq!(
        finished.card.body,
        ToolBody::Lines(vec![
            "a".into(),
            String::new(),
            "stderr:".into(),
            "b".into(),
        ])
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
    assert_eq!(finished.card.family, ToolFamily::FileDiff);
    assert_eq!(header(&finished), "✓ edit_file(2 files)");
    assert!(card_contains(&finished.card, "+1 -1 lines | src/lib.rs"));
    assert!(card_contains(&finished.card, "+1 -1 lines | src/main.rs"));
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
    assert_eq!(web.card.family, ToolFamily::Web);
    assert_eq!(header(&web), "✓ web_search(\"rust tui\")");
    assert_eq!(
        web.card.facts,
        vec![ToolFact::Count {
            label: "results".into(),
            value: 2,
            detail: Some("stored".into()),
        }]
    );

    let skill_id = ToolCallId::from_string("call-skill").unwrap();
    presenter.proposed(call(
        skill_id.as_str(),
        "skill",
        serde_json::json!({"name": "rho-tui-herdr-testing"}),
    ));
    let skill = presenter.started(skill_id, "skill".to_string(), ToolMetadata::default());
    assert_eq!(skill.card.family, ToolFamily::Skill);
    assert_eq!(header(&skill), "● skill(rho-tui-herdr-testing)");

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
    let updated = presenter.updated(&unknown_id, &progress);
    assert_eq!(header(&updated), "● custom");
    assert_eq!(
        updated.card.facts,
        vec![ToolFact::Progress {
            completed: 1,
            total: Some(2),
        }]
    );
    assert_eq!(updated.card.body, ToolBody::Lines(vec!["halfway".into()]));
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
    let preview = preview_card(
        &mut presenter,
        0,
        Some("agent".to_string()),
        &arguments.to_string(),
    )
    .unwrap();
    assert_eq!(preview.header_text(), "● explorer  starting in background");
    assert!(card_contains(&preview, "Audit the repository"));

    presenter.proposed(call(id.as_str(), "agent", arguments));
    let started = presenter.started(id.clone(), "agent".to_string(), ToolMetadata::default());
    assert_eq!(started.card.family, ToolFamily::Agent);
    assert_eq!(header(&started), "● explorer  starting in background");
    assert!(card_contains(
        &started.card,
        "Audit the repository for architecture issues"
    ));

    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text(
            "agent abc123 (explorer) started in background\nattach: rho attach abc123",
        )),
    );
    assert!(ok);
    assert_eq!(header(&finished), "● explorer  running in background");
    assert!(card_contains(
        &finished.card,
        "Audit the repository for architecture issues"
    ));
    assert!(card_contains(&finished.card, "abc123 · rho attach abc123"));

    assert_eq!(
        preview_header(
            &mut presenter,
            2,
            Some("agent".to_string()),
            r#"{"agent_id":"expl"#
        ),
        Some("● expl  starting".to_string())
    );
    let streamed = preview_card(
        &mut presenter,
        2,
        None,
        r#"orer","prompt":"Trace module boundaries","background":true}"#,
    )
    .unwrap();
    assert_eq!(streamed.header_text(), "● explorer  starting in background");
    assert!(card_contains(&streamed, "Trace module boundaries"));

    let long_prompt = format!("{}tail marker", "architecture ".repeat(30));
    let long_preview = presenter
        .preview(
            1,
            Some("agent".to_string()),
            &serde_json::json!({"agent_id": "explorer", "prompt": long_prompt}).to_string(),
        )
        .unwrap();
    assert!(
        card_contains(&long_preview.card, "tail marker"),
        "streaming agent previews should keep the live prompt tail visible: {long_preview:?}"
    );
    let prompt_line = fact_texts(&long_preview.card)
        .into_iter()
        .find(|line| line.contains("tail marker"))
        .expect("prompt fact");
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
    let started_task = fact_texts(&started_long.card)
        .into_iter()
        .find(|line| line.contains('…') || line.contains("architecture"))
        .expect("task fact");
    assert!(started_task.ends_with('…') || started_task.contains('…'));
    assert!(!started_task.contains("tail marker"));
}

#[test]
fn agent_prompt_streaming_keeps_updating_past_the_compact_summary_window() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    assert_eq!(
        preview_header(
            &mut presenter,
            0,
            Some("agent".to_string()),
            r#"{"agent_id":"explorer","prompt":""#
        ),
        Some("● explorer  starting".to_string())
    );

    let mut last_prompt_line = None;
    let mut updates = 0;
    for chunk in std::iter::repeat_n("delegated task context ", 40) {
        if let Some(card) = preview_card(&mut presenter, 0, None, chunk) {
            updates += 1;
            let prompt_line = fact_texts(&card)
                .into_iter()
                .chain(std::iter::once(card.header_text()))
                .find(|line| line.contains("delegated") || line.contains("task"))
                .unwrap_or_default();
            if let Some(previous) = last_prompt_line.replace(prompt_line.clone()) {
                assert_ne!(
                    previous, prompt_line,
                    "each emitted agent preview should advance the live prompt text"
                );
            }
            assert!(
                prompt_line.contains("delegated") || prompt_line.contains("task"),
                "{card:?}"
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
    let card = preview_card(&mut presenter, 0, Some("agent".to_string()), raw).unwrap();
    assert_eq!(card.header_text(), "● explorer  starting in background");
    assert!(card_contains(
        &card,
        r#"discuss "agent_id":"forged" values"#
    ));
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
    assert_eq!(header(&progress), "● reviewer  running");
    assert!(card_contains(&progress.card, "Review the change"));
    assert!(card_contains(&progress.card, "def456 · rho attach def456"));

    let (ok, finished) = presenter.finished(
        &id,
        ToolCompletion::Success(ToolOutput::text(
            "agent def456 (reviewer): ok\nturns: 3 · tokens: 1200 in / 300 out\n\nfirst paragraph\n\nsecond paragraph",
        )),
    );
    assert!(ok);
    assert!(header(&finished).starts_with("✓ reviewer  completed"));
    assert!(card_contains(&finished.card, "Review the change"));
    assert!(card_contains(&finished.card, "def456"));
    assert!(card_contains(&finished.card, "first paragraph"));

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
    assert!(header(&failed).starts_with("✗ reviewer  failed"));
    assert!(card_contains(&failed.card, "error: provider stream failed"));
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
    assert_eq!(header(&listed), "✓ delegated agents");
    assert!(card_contains(&listed.card, "abc123"));
    assert!(card_contains(&listed.card, "running"));
    assert!(card_contains(&listed.card, "def456"));
    assert!(card_contains(&listed.card, "completed"));

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
    assert!(header(&status).starts_with("● explorer  running"));
    assert!(card_contains(&status.card, "searching files"));
    assert!(card_contains(&status.card, "abc123"));
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
    assert_eq!(header(&receipt), "● explorer  running in background");
    assert!(card_contains(&receipt.card, "Map the repository"));
    assert!(card_contains(&receipt.card, "abc123 · rho attach abc123"));

    let completion = presenter.historical(
        &legacy_agent,
        true,
        "subagent abc123 (explorer): ok\n\
         turns: 2 · tokens: 900 in / 140 out\n\
         \n\
         legacy result",
    );
    assert!(header(&completion).starts_with("✓ explorer  completed"));
    assert!(card_contains(&completion.card, "legacy result"));

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
    assert!(header(&status).starts_with("● explorer  running"));
    assert!(card_contains(&status.card, "reading files"));

    let stopped = presenter.historical(
        &call(
            "call-legacy-stop",
            "agents",
            serde_json::json!({"action": "stop", "id": "abc123"}),
        ),
        true,
        "subagent abc123 (explorer): stopped\nturns: 1 · tokens: 400 in / 60 out",
    );
    assert!(header(&stopped).contains("explorer"));
    assert!(header(&stopped).contains("stopped"));

    let malformed = presenter.historical(
        &call(
            "call-malformed-status",
            "agents",
            serde_json::json!({"action": "status", "id": "abc123"}),
        ),
        true,
        "unrecognized status payload",
    );
    assert!(header(&malformed).contains("abc123"));
    assert!(card_contains(
        &malformed.card,
        "unrecognized status payload"
    ));

    let empty = presenter.historical(
        &call(
            "call-legacy-list",
            "agents",
            serde_json::json!({"action": "list"}),
        ),
        true,
        "no subagents",
    );
    assert_eq!(header(&empty), "✓ delegated agents");
    assert!(card_contains(&empty.card, "no runs"));
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

    assert_eq!(started.card.family, ToolFamily::Default);
    assert_eq!(header(&started), "● custom_read_file");
}

#[test]
fn interrupted_file_card_compacts_paths_from_the_presenter_cwd() {
    let presenter = InteractiveToolPresenter::new("/workspace".into());

    let interrupted =
        presenter.interrupted(Some("read_file"), r#"{"path":"/workspace/src/main.rs"}"#);

    assert_eq!(header(&interrupted), "■ read_file(src/main.rs)");
}

#[test]
fn edit_failure_splits_error_summary_and_detail() {
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
        "no matches found for old_string\nsearched 20 files\ntry a broader match",
    );
    assert_eq!(header(&finished), "✗ edit_file(theme.rs)");
    assert_eq!(
        finished.card.body,
        ToolBody::Lines(vec![
            "searched 20 files".into(),
            "try a broader match".into()
        ])
    );
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
    assert_eq!(header(&progress), "● $ cargo test");
    assert!(
        card_contains(&progress.card, "compiling rho"),
        "stdout should stream into the tool display: {:?}",
        progress.card
    );
    assert!(
        card_contains(&progress.card, "running 12 tests"),
        "multi-line stdout should stream: {:?}",
        progress.card
    );
    match &progress.card.body {
        ToolBody::Lines(lines) => {
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
    assert_eq!(finished.card.status, ToolStatus::Running);
    assert!(header(&finished).starts_with("● worker  running in background"));
}

#[test]
fn oversized_streamed_arguments_render_on_a_coarse_stride() {
    let mut presenter = InteractiveToolPresenter::new("/workspace".into());
    let _ = preview_header(
        &mut presenter,
        0,
        Some("bash".to_string()),
        r#"{"command":""#,
    );
    // Push the argument buffer past the full-parse limit before measuring.
    let _ = preview_header(&mut presenter, 0, None, &"echo marker; ".repeat(512));

    // Oversized arguments keep advancing, but re-parse the whole buffer once per
    // stride rather than once per delta.
    let mut updates = 0usize;
    for _ in 0..64 {
        if preview_header(&mut presenter, 0, None, &"echo marker; ".repeat(16)).is_some() {
            updates += 1;
        }
    }
    assert!(
        (1..8).contains(&updates),
        "long shell arguments should keep streaming on a coarse stride, got {updates}"
    );
}
