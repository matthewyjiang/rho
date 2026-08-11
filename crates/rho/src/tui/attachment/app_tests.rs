use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

#[test]
fn parallel_pending_tools_keep_independent_slots() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );
    let card_a = rho_tools::tool_card::ToolCard::new(
        rho_tools::tool_card::ToolStatus::Running,
        rho_tools::tool_card::ToolFamily::FileCommand,
        rho_tools::tool_card::ToolHeader::call("read_file", Some("a.rs".into())),
    );
    let card_b = rho_tools::tool_card::ToolCard::new(
        rho_tools::tool_card::ToolStatus::Running,
        rho_tools::tool_card::ToolFamily::FileCommand,
        rho_tools::tool_card::ToolHeader::call("read_file", Some("b.rs".into())),
    );
    app.apply_event(AttachmentEvent::ToolStarted {
        key: Some("call-a".into()),
        card: card_a.clone(),
    });
    app.apply_event(AttachmentEvent::ToolStarted {
        key: Some("call-b".into()),
        card: card_b.clone(),
    });
    app.apply_event(AttachmentEvent::ToolUpdated {
        key: Some("call-a".into()),
        card: card_a
            .clone()
            .with_facts(vec![rho_tools::tool_card::ToolFact::Meta {
                text: "reading".into(),
            }]),
    });
    assert_eq!(
        app.pending_order,
        vec!["call-a".to_string(), "call-b".to_string()]
    );
    assert_eq!(app.pending_tools.len(), 2);

    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-b".into()),
        card: card_b
            .clone()
            .with_facts(vec![rho_tools::tool_card::ToolFact::Count {
                label: "lines".into(),
                value: 2,
                detail: None,
            }]),
    });
    assert_eq!(app.pending_order, vec!["call-a".to_string()]);
    assert!(app.pending_tools.contains_key("call-a"));
    assert!(!app.pending_tools.contains_key("call-b"));
    assert!(matches!(
        app.transcript.last(),
        Some(Entry::Tool(tool)) if tool.card.header_text() == "● read_file(b.rs)"
    ));
}

#[test]
fn provider_retry_replaces_output_but_preserves_presented_events() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );
    app.apply_event(AttachmentEvent::Prompt("delegated task".into()));
    app.apply_event(AttachmentEvent::StepStarted);
    app.apply_event(AttachmentEvent::AssistantTextDelta("discard me".into()));
    app.apply_event(AttachmentEvent::Notice("keep notice".into()));
    app.apply_event(AttachmentEvent::ToolFinished {
        key: None,
        card: rho_tools::tool_card::ToolCard::new(
            rho_tools::tool_card::ToolStatus::Ok,
            rho_tools::tool_card::ToolFamily::Default,
            rho_tools::tool_card::ToolHeader::call("keep tool", None),
        ),
    });
    app.apply_event(AttachmentEvent::ReasoningDelta("discard reasoning".into()));
    app.apply_event(AttachmentEvent::ProviderStreamReset);
    app.apply_event(AttachmentEvent::AssistantTextDelta("keep me".into()));

    assert!(matches!(
        app.transcript.as_slice(),
        [
            Entry::User(prompt),
            Entry::Notice(notice),
            Entry::Tool(tool),
            Entry::Assistant(answer)
        ] if prompt == "delegated task"
            && notice == "keep notice"
            && tool.card.header_text() == "✓ keep tool"
            && answer == "keep me"
    ));
}

#[test]
fn attached_view_ignores_prompt_input() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );
    app.apply_event(AttachmentEvent::Prompt("delegated task".into()));

    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('x'),
        KeyModifiers::NONE,
    )));
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));

    assert_eq!(app.transcript.len(), 1);
    assert!(!app.should_quit);
    assert!(matches!(
        &app.transcript[0],
        Entry::User(prompt) if prompt == "delegated task"
    ));
}

#[test]
fn provider_retry_preserves_failed_attempt_usage() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );

    app.apply_event(AttachmentEvent::StepStarted);
    app.apply_event(AttachmentEvent::Usage(ModelUsage {
        input_tokens: Some(100),
        output_tokens: Some(10),
        cache_read_tokens: Some(50),
        ..ModelUsage::default()
    }));
    app.apply_event(AttachmentEvent::ProviderStreamReset);
    app.apply_event(AttachmentEvent::Usage(ModelUsage {
        input_tokens: Some(40),
        output_tokens: Some(4),
        cache_read_tokens: Some(20),
        ..ModelUsage::default()
    }));

    assert_eq!(
        app.run_usage.current(),
        Some(&ModelUsage {
            input_tokens: Some(140),
            output_tokens: Some(14),
            cache_read_tokens: Some(70),
            total_tokens: Some(224),
            ..ModelUsage::default()
        })
    );
}

#[test]
fn multi_step_usage_replaces_live_run_snapshot() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );

    app.apply_event(AttachmentEvent::StepStarted);
    app.apply_event(AttachmentEvent::Usage(ModelUsage {
        input_tokens: Some(100),
        output_tokens: Some(20),
        cache_read_tokens: Some(50),
        ..ModelUsage::default()
    }));
    app.apply_event(AttachmentEvent::StepStarted);
    app.apply_event(AttachmentEvent::Usage(ModelUsage {
        input_tokens: Some(200),
        output_tokens: Some(60),
        cache_read_tokens: Some(150),
        ..ModelUsage::default()
    }));

    assert_eq!(
        app.run_usage.current(),
        Some(&ModelUsage {
            input_tokens: Some(200),
            output_tokens: Some(60),
            cache_read_tokens: Some(150),
            ..ModelUsage::default()
        })
    );
}

#[test]
fn herdr_state_follows_attached_subagent_state() {
    let status = |state| RunStatus {
        state,
        last_activity: Some("working".into()),
        ..RunStatus::default()
    };

    assert_eq!(
        herdr_status("abc123", &status(RunState::Starting)).0,
        HerdrState::Working
    );
    assert_eq!(
        herdr_status("abc123", &status(RunState::Running)).0,
        HerdrState::Working
    );
    assert_eq!(
        herdr_status("abc123", &status(RunState::Ok)).0,
        HerdrState::Idle
    );
    assert_eq!(
        herdr_status("abc123", &status(RunState::Stopped)).0,
        HerdrState::Idle
    );
    assert_eq!(
        herdr_status("abc123", &status(RunState::Error)).0,
        HerdrState::Blocked
    );
}

#[test]
fn status_token_fallback_uses_run_status_totals() {
    let summary = activity_metrics_line(
        "assistant text",
        None,
        None,
        Some(&RunStatus {
            input_tokens: Some(1_200),
            output_tokens: Some(300),
            ..RunStatus::default()
        }),
    );
    assert_eq!(summary, "assistant text · tokens in 1.2K · out 300");
}

#[test]
fn identity_line_includes_provider_model_runtime_elapsed_and_cost() {
    use rho_providers::model::{
        display_name::ModelDisplayNameCacheGuard,
        models_dev::{
            with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests,
            ModelMetadata,
        },
    };

    let catalog = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(catalog.path().to_path_buf(), || {
        let _names = ModelDisplayNameCacheGuard::new();
        write_cached_model_metadata_for_tests(
            "openai",
            "gpt-5.5",
            &ModelMetadata {
                display_name: Some("GPT-5.5".into()),
                reasoning_metadata_complete: true,
                ..ModelMetadata::default()
            },
        );
        let line = identity_line(
            Some(&RunStatus {
                agent_id: Some("explorer".into()),
                provider: Some("openai".into()),
                model: Some("gpt-5.5".into()),
                runtime: Some(crate::agent::AgentRuntime::Rho),
                started_at: Some(1_000),
                finished_at: Some(1_065),
                turns: 3,
                total_cost_usd: Some(0.0388),
                claude_session_id: Some("sess-1".into()),
                ..RunStatus::default()
            }),
            None,
            /* now_unix_secs */ 9_999,
        );
        assert_eq!(
            line,
            "openai/gpt-5.5 (GPT-5.5) · rho · turn 3 · 1m 05s · claude sess-1 · $0.039"
        );
    });
}

#[test]
fn identity_line_handles_partial_model_fields() {
    // A Rho status needs both provider and model before it can name one.
    assert_eq!(
        identity_line(
            Some(&RunStatus {
                model: Some("gpt-5.5".into()),
                turns: 1,
                ..RunStatus::default()
            }),
            None,
            /* now_unix_secs */ 0,
        ),
        "turn 1"
    );
    // An unpinned Claude run still names the runtime honestly.
    assert_eq!(
        identity_line(
            Some(&RunStatus {
                provider: Some("claude-code".into()),
                runtime: Some(crate::agent::AgentRuntime::ClaudeCli),
                turns: 2,
                ..RunStatus::default()
            }),
            None,
            /* now_unix_secs */ 0,
        ),
        "claude-code (no model pinned; Claude Code chooses) · claude-cli · turn 2"
    );
    assert_eq!(identity_line(None, None, 0), "");
}

#[test]
fn header_title_line_names_run_and_agent() {
    let line = header_title_line("abc123", "explorer", "running", None);
    assert_eq!(line.to_string(), "rho  attach abc123 · explorer · running");
}

#[test]
fn format_run_cost_prefers_status_total_via_shared_usd_helper() {
    assert_eq!(
        format_run_cost(
            &RunStatus {
                total_cost_usd: Some(0.0388),
                ..RunStatus::default()
            },
            Some(&ModelUsage {
                cost_usd_micros: Some(99),
                ..ModelUsage::default()
            }),
        )
        .as_deref(),
        Some("$0.039")
    );
    assert_eq!(
        format_run_cost(
            &RunStatus::default(),
            Some(&ModelUsage {
                cost_usd_micros: Some(12_500),
                ..ModelUsage::default()
            }),
        )
        .as_deref(),
        Some("$0.013")
    );
}
