use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use pretty_assertions::assert_eq;
use ratatui::layout::Rect;
use tempfile::TempDir;

use super::*;

fn test_app() -> (TempDir, AttachmentApp) {
    let directory = TempDir::new().unwrap();
    let app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        AttachmentDisplaySettings::default(),
        HerdrReporter::default(),
    );
    (directory, app)
}

fn line_text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn parallel_pending_tools_keep_independent_slots() {
    let (_directory, mut app) = test_app();
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
    match app.transcript.last() {
        Some(Entry::Tool(tool)) => {
            assert_eq!(tool.card.header_text(), "● read_file(b.rs)");
        }
        other => panic!("unexpected transcript tail: {other:?}"),
    }
}

#[test]
fn provider_retry_replaces_output_but_preserves_presented_events() {
    let (_directory, mut app) = test_app();
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
    let (_directory, mut app) = test_app();
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
    let (_directory, mut app) = test_app();

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
    let (_directory, mut app) = test_app();

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

// Covers: attach render policy mirrors interactive show_reasoning_output + zen_mode.
// Owner: AttachmentDisplaySettings + history_lines.
#[test]
fn history_lines_follow_display_settings() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        AttachmentDisplaySettings::default(),
        HerdrReporter::default(),
    );
    app.apply_event(AttachmentEvent::Prompt("task".into()));
    app.apply_event(AttachmentEvent::ReasoningDelta("secret plan".into()));
    app.apply_event(AttachmentEvent::AssistantTextDelta("answer".into()));
    app.apply_event(AttachmentEvent::ToolFinished {
        key: None,
        card: rho_tools::tool_card::ToolCard::new(
            rho_tools::tool_card::ToolStatus::Ok,
            rho_tools::tool_card::ToolFamily::Default,
            rho_tools::tool_card::ToolHeader::call("read_file", Some("a.rs".into())),
        ),
    });

    let visible = |app: &AttachmentApp| line_text(&app.history_lines(80, None));

    let full = visible(&app);
    assert!(full.iter().any(|line| line.contains("secret plan")));
    assert!(full.iter().any(|line| line.contains("answer")));
    assert!(full.iter().any(|line| line.contains("read_file")));

    app.display.show_reasoning_output = false;
    let hidden_reasoning = visible(&app);
    assert!(hidden_reasoning
        .iter()
        .all(|line| !line.contains("secret plan")));
    assert!(hidden_reasoning.iter().any(|line| line.contains("answer")));
    assert!(hidden_reasoning
        .iter()
        .any(|line| line.contains("read_file")));
    // Ingest stays complete so journal bookkeeping is unchanged.
    assert!(matches!(
        app.transcript.as_slice(),
        [
            Entry::User(_),
            Entry::Reasoning(_),
            Entry::Assistant(_),
            Entry::Tool(_)
        ]
    ));

    app.display.zen_mode = true;
    app.display.show_reasoning_output = true;
    let zen = visible(&app);
    assert!(zen.iter().all(|line| !line.contains("secret plan")));
    assert!(zen.iter().all(|line| !line.contains("read_file")));
    assert!(zen.iter().any(|line| line.contains("answer")));
    assert!(zen.iter().any(|line| line.contains("task")));
}

// Covers: max_tool_output_lines comes from display settings, not a local constant.
// Owner: AttachmentDisplaySettings + history_lines.
#[test]
fn history_lines_honor_max_tool_output_lines() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        AttachmentDisplaySettings {
            max_tool_output_lines: 1,
            ..AttachmentDisplaySettings::default()
        },
        HerdrReporter::default(),
    );
    let body = (0..5).map(|i| format!("line-{i}")).collect::<Vec<_>>();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: None,
        card: rho_tools::tool_card::ToolCard::new(
            rho_tools::tool_card::ToolStatus::Ok,
            rho_tools::tool_card::ToolFamily::Default,
            rho_tools::tool_card::ToolHeader::call("bash", None),
        )
        .with_body(rho_tools::tool_card::ToolBody::Lines(body)),
    });

    let lines = line_text(&app.history_lines(80, None));
    assert!(lines.iter().any(|line| line.contains("line-0")));
    assert!(lines.iter().all(|line| !line.contains("line-4")));
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

fn long_body_card() -> rho_tools::tool_card::ToolCard {
    rho_tools::tool_card::ToolCard::new(
        rho_tools::tool_card::ToolStatus::Ok,
        rho_tools::tool_card::ToolFamily::FileCommand,
        rho_tools::tool_card::ToolHeader::shell("$", Some("seq 20".into())),
    )
    .with_body(rho_tools::tool_card::ToolBody::Lines(
        (0..20).map(|i| format!("out-{i}")).collect(),
    ))
}

fn short_body_card() -> rho_tools::tool_card::ToolCard {
    rho_tools::tool_card::ToolCard::new(
        rho_tools::tool_card::ToolStatus::Ok,
        rho_tools::tool_card::ToolFamily::Default,
        rho_tools::tool_card::ToolHeader::call("echo", None),
    )
    .with_body(rho_tools::tool_card::ToolBody::Lines(vec!["ok".into()]))
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn click_card(app: &mut AttachmentApp, column: u16, row: u16) {
    app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
    app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
}

fn sync_view(app: &mut AttachmentApp, width: u16, height: u16) {
    let lines = app.history_lines(width as usize, None);
    app.sync_history_geometry(Rect::new(0, 4, width, height), lines.len(), width as usize);
}

fn transcript_tool(app: &AttachmentApp, index: usize) -> &ToolEntry {
    match app.transcript.get(index) {
        Some(Entry::Tool(tool)) => tool,
        other => panic!("expected tool at {index}, got {other:?}"),
    }
}

// Covers: click expands and collapses an over-budget finished card
// Owner: attach event loop
#[test]
fn click_toggles_finished_over_budget_card() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-1".into()),
        card: long_body_card(),
    });
    sync_view(&mut app, 80, 30);
    let collapsed_len = app.history_lines(80, None).len();

    click_card(&mut app, 8, 5);
    assert!(transcript_tool(&app, 0).expanded);
    assert!(app.history_lines(80, None).len() > collapsed_len);

    click_card(&mut app, 8, 5);
    assert!(!transcript_tool(&app, 0).expanded);
    assert_eq!(app.history_lines(80, None).len(), collapsed_len);
}

// Covers: under-budget cards ignore click
// Owner: attach event loop
#[test]
fn click_ignores_under_budget_card() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-1".into()),
        card: short_body_card(),
    });
    sync_view(&mut app, 80, 20);
    click_card(&mut app, 8, 5);
    assert!(!transcript_tool(&app, 0).expanded);
}

/// Bold flag plus ink color of every text cell in `row`: the look the hover
/// lift changes without changing text.
fn row_look(
    terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
    row: u16,
) -> Vec<(bool, ratatui::style::Color)> {
    (0..80u16)
        .filter_map(|column| {
            let cell = &terminal.backend().buffer()[(column, row)];
            (!cell.symbol().trim().is_empty()).then_some((
                cell.modifier.contains(ratatui::style::Modifier::BOLD),
                cell.fg,
            ))
        })
        .collect()
}

// Covers: hover lift derives from the pointer at draw time, so it follows the
// pointer off-card and re-anchors after a click toggle reshapes the card
// Owner: attach draw
#[test]
fn draw_hover_lift_follows_pointer_and_survives_toggle() {
    // Pin the global theme: terminal mode without a palette sample lifts named
    // ink to bold, guaranteeing an observable change on the body row.
    let _guard = crate::tui::theme::theme_test_lock();
    Theme::apply_committed("terminal");
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-1".into()),
        card: long_body_card(),
    });
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 30)).expect("test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("baseline draw");

    let baseline = row_look(&terminal, 5);
    assert!(!baseline.is_empty(), "card body row has text cells");

    // Pointer over the header, outside the history viewport: no lift.
    app.handle_event(mouse(MouseEventKind::Moved, 8, 1));
    terminal.draw(|frame| app.draw(frame)).expect("header draw");
    assert_eq!(
        row_look(&terminal, 5),
        baseline,
        "pointer off-card must not lift"
    );

    // Pointer over the card: lift.
    app.handle_event(mouse(MouseEventKind::Moved, 8, 5));
    terminal.draw(|frame| app.draw(frame)).expect("hover draw");
    assert_ne!(
        row_look(&terminal, 5),
        baseline,
        "pointer on-card must lift"
    );

    // Click expands the card; the next draw re-anchors the lift to the
    // reshaped card under the stationary pointer instead of dropping it.
    click_card(&mut app, 8, 5);
    terminal.draw(|frame| app.draw(frame)).expect("toggle draw");
    assert!(transcript_tool(&app, 0).expanded);
    assert_ne!(
        row_look(&terminal, 5),
        baseline,
        "lift must re-anchor after toggle"
    );
}

// Covers: under-budget cards are neither click-toggleable nor hover-liftable
// Owner: attach draw
#[test]
fn draw_hover_lift_skips_untoggleable_card() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-1".into()),
        card: short_body_card(),
    });
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 30)).expect("test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("baseline draw");
    let baseline = row_look(&terminal, 5);

    app.handle_event(mouse(MouseEventKind::Moved, 8, 5));
    terminal.draw(|frame| app.draw(frame)).expect("hover draw");
    assert_eq!(
        row_look(&terminal, 5),
        baseline,
        "under-budget card must not lift"
    );
}

// Covers: releasing a scrollbar drag over a card must not toggle
// Owner: attach event loop
#[test]
fn scrollbar_drag_release_does_not_toggle() {
    let (_directory, mut app) = test_app();
    for i in 0..8 {
        app.apply_event(AttachmentEvent::ToolFinished {
            key: Some(format!("call-{i}")),
            card: long_body_card(),
        });
    }
    sync_view(&mut app, 80, 8);
    app.scroll
        .reveal(std::time::Instant::now(), HISTORY_SCROLLBAR_REVEAL_DURATION);

    app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), 79, 4));
    assert!(app.scroll.drag().is_some());
    app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 8, 5));
    assert!(app
        .transcript
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool(tool) => Some(tool.expanded),
            _ => None,
        })
        .all(|expanded| !expanded));
}

// Covers: releasing a card-to-card drag must not toggle the release target
// Owner: attach event loop
#[test]
fn card_to_card_drag_release_does_not_toggle() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-1".into()),
        card: long_body_card(),
    });
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-2".into()),
        card: long_body_card(),
    });
    sync_view(&mut app, 80, 40);
    let first_height = match app.transcript.first() {
        Some(entry) => HistoryItem::Transcript { index: 0, entry }
            .paint_lines(80, app.display.max_tool_output_lines())
            .len(),
        None => panic!("expected first transcript tool"),
    };
    let second_row = 4u16.saturating_add(u16::try_from(first_height).expect("card height"));

    app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), 8, 5));
    app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 8, second_row));
    assert!(!transcript_tool(&app, 0).expanded);
    assert!(!transcript_tool(&app, 1).expanded);
}

// Covers: leaving a card then releasing on it again is a drag, not a click
// Owner: attach event loop
#[test]
fn drag_away_and_back_does_not_toggle() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-1".into()),
        card: long_body_card(),
    });
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-2".into()),
        card: long_body_card(),
    });
    sync_view(&mut app, 80, 40);
    let first_height = match app.transcript.first() {
        Some(entry) => HistoryItem::Transcript { index: 0, entry }
            .paint_lines(80, app.display.max_tool_output_lines())
            .len(),
        None => panic!("expected first transcript tool"),
    };
    let second_row = 4u16.saturating_add(u16::try_from(first_height).expect("card height"));

    app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), 8, 5));
    app.handle_event(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        8,
        second_row,
    ));
    app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 8, 5));
    assert!(!transcript_tool(&app, 0).expanded);
    assert!(!transcript_tool(&app, 1).expanded);
}

// Covers: dragging inside one card is not a click
// Owner: attach event loop
#[test]
fn intra_card_drag_does_not_toggle() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("call-1".into()),
        card: long_body_card(),
    });
    sync_view(&mut app, 80, 30);

    app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), 8, 5));
    app.handle_event(mouse(MouseEventKind::Drag(MouseButton::Left), 8, 6));
    app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 8, 6));
    assert!(!transcript_tool(&app, 0).expanded);
}

// Covers: a pending card that finishes during a stationary click still expands
// Owner: attach event loop
#[test]
fn pending_finish_during_click_still_toggles() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolStarted {
        key: Some("live".into()),
        card: long_body_card(),
    });
    sync_view(&mut app, 80, 30);

    app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), 8, 5));
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("live".into()),
        card: long_body_card(),
    });
    app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 8, 5));
    assert!(transcript_tool(&app, 0).expanded);
}

// Covers: finishing the pressed pending card still expands when another pending remains
// Owner: attach event loop
#[test]
fn pending_finish_during_click_still_toggles_with_sibling_pending() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolStarted {
        key: Some("first".into()),
        card: long_body_card(),
    });
    app.apply_event(AttachmentEvent::ToolStarted {
        key: Some("second".into()),
        card: long_body_card(),
    });
    sync_view(&mut app, 80, 40);
    let first_height = HistoryItem::Pending {
        key: "first",
        tool: &app.pending_tools["first"],
    }
    .paint_lines(80, app.display.max_tool_output_lines())
    .len();
    let second_row = 4u16.saturating_add(u16::try_from(first_height).expect("card height"));

    app.handle_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        8,
        second_row,
    ));
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("second".into()),
        card: long_body_card(),
    });
    app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 8, second_row));
    assert!(transcript_tool(&app, 0).expanded);
    assert!(!app.pending_tools["first"].expanded);
}

// Covers: provider reset must not drop a click on a tool that just finished
// Owner: attach event loop
#[test]
fn pending_finish_then_reset_still_toggles_pressed_card() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::Prompt("task".into()));
    app.apply_event(AttachmentEvent::StepStarted);
    app.apply_event(AttachmentEvent::AssistantTextDelta("retry this".into()));
    app.apply_event(AttachmentEvent::ToolStarted {
        key: Some("live".into()),
        card: long_body_card(),
    });
    sync_view(&mut app, 80, 40);
    let pending_row = {
        let mut start = 0usize;
        for item in app.history_items(None) {
            let height = item
                .paint_lines(80, app.display.max_tool_output_lines())
                .len();
            if matches!(item, HistoryItem::Pending { key: "live", .. }) {
                break;
            }
            start = start.saturating_add(height);
        }
        4u16.saturating_add(u16::try_from(start).expect("pending row"))
    };

    app.handle_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        8,
        pending_row,
    ));
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("live".into()),
        card: long_body_card(),
    });
    app.apply_event(AttachmentEvent::ProviderStreamReset);
    app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 8, pending_row));
    assert!(transcript_tool(&app, 1).expanded);
}

// Covers: focus change or resize must not complete a card press
// Owner: attach event loop
#[test]
fn interrupted_press_does_not_toggle() {
    let interrupts = [Event::FocusLost, Event::FocusGained, Event::Resize(80, 24)];
    for interrupt in interrupts {
        let (_directory, mut app) = test_app();
        app.apply_event(AttachmentEvent::ToolFinished {
            key: Some("call-1".into()),
            card: long_body_card(),
        });
        sync_view(&mut app, 80, 30);
        app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), 8, 5));
        let label = format!("{interrupt:?}");
        app.handle_event(interrupt);
        app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 8, 5));
        assert!(!transcript_tool(&app, 0).expanded, "{label}");
    }
}

// Covers: ctrl+o expands latest pending, then last finished card, accordion-style
// Owner: attach event loop
#[test]
fn ctrl_o_prefers_pending_and_collapses_others() {
    let (_directory, mut app) = test_app();
    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("done".into()),
        card: long_body_card(),
    });
    app.apply_event(AttachmentEvent::ToolStarted {
        key: Some("live".into()),
        card: long_body_card(),
    });
    sync_view(&mut app, 80, 40);

    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(app.pending_tools["live"].expanded);
    assert!(!transcript_tool(&app, 0).expanded);

    app.apply_event(AttachmentEvent::ToolFinished {
        key: Some("live".into()),
        card: long_body_card(),
    });
    assert!(transcript_tool(&app, 1).expanded);
    assert!(!transcript_tool(&app, 0).expanded);

    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(!transcript_tool(&app, 0).expanded);
    assert!(!transcript_tool(&app, 1).expanded);

    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(!transcript_tool(&app, 0).expanded);
    assert!(transcript_tool(&app, 1).expanded);
}
