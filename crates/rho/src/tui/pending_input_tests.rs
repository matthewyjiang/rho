use std::collections::VecDeque;
use std::time::Instant;

use ratatui::text::Line;

use super::*;
use crate::tui::{tests::test_app, Entry, StreamKind};

fn prompt(text: &str) -> QueuedPrompt {
    QueuedPrompt {
        prompt: text.into(),
        display_prompt: text.into(),
        paste_segments: Vec::new(),
        media: Vec::new(),
    }
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn alt_up_prioritizes_latest_local_steer_over_follow_up() {
    let mut app = test_app();
    app.pending.push_follow_up(prompt("future turn"));
    app.pending
        .steering_prompts_mut()
        .push_back(prompt("first steer"));
    app.pending
        .steering_prompts_mut()
        .push_back(prompt("latest steer"));

    assert!(app.handle_pending_input_key(key(KeyCode::Up, KeyModifiers::ALT)));

    assert_eq!(app.input_ui.text(), "latest steer");
    assert_eq!(
        *app.pending.steering_prompts(),
        VecDeque::from([prompt("first steer")])
    );
    assert_eq!(
        *app.pending.queued_prompts(),
        VecDeque::from([prompt("future turn")])
    );
}

#[test]
fn alt_up_requests_retraction_for_accepted_steer() {
    let mut app = test_app();
    let id = rho_sdk::SteeringId::new();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: id.clone(),
            prompt: prompt("retract me"),
            delivered: false,
        });

    app.handle_pending_input_key(key(KeyCode::Up, KeyModifiers::ALT));

    assert!(matches!(
        app.pending.input_action(),
        Some(PendingInputAction::EditAccepted {
            id: ref action_id,
            ..
        }) if action_id == &id
    ));
    assert!(app.input_ui.text().is_empty());
    assert_eq!(app.pending.accepted_steering().len(), 1);
}

#[test]
fn alt_up_preserves_nonempty_composer() {
    let mut app = test_app();
    app.input_ui.set_text("draft".to_string());
    app.input_ui.set_cursor(app.input_char_len());
    app.pending.push_follow_up(prompt("future turn"));

    app.handle_pending_input_key(key(KeyCode::Up, KeyModifiers::ALT));

    assert_eq!(app.input_ui.text(), "draft");
    assert_eq!(
        *app.pending.queued_prompts(),
        VecDeque::from([prompt("future turn")])
    );
    assert_eq!(
        app.status(),
        "clear the composer before editing pending input"
    );
}

#[test]
fn applied_event_preserves_selection_of_a_later_pending_item() {
    let mut app = test_app();
    let applied = rho_sdk::SteeringId::new();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: applied.clone(),
            prompt: prompt("first steer"),
            delivered: false,
        });
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: rho_sdk::SteeringId::new(),
            prompt: prompt("second steer"),
            delivered: false,
        });
    app.pending.push_follow_up(prompt("future turn"));
    app.pending.input_panel_mut().selected = 2;

    app.record_applied_steering(&[applied]);

    assert_eq!(app.pending.input_panel().selected, 1);
    let lines = app.pending_input_lines(80);
    assert!(line_text(&lines[2]).contains("▸ NEXT"));
    assert!(matches!(
        app.history.entries(),
        [Entry::User(text)] if text == "first steer"
    ));
}

#[test]
fn backspace_removes_the_selected_follow_up() {
    let mut app = test_app();
    app.pending.push_follow_up(prompt("first"));
    app.pending.push_follow_up(prompt("second"));
    app.select_pending_recall_target();

    app.handle_pending_input_key(key(KeyCode::Char('q'), KeyModifiers::ALT));
    app.handle_pending_input_key(key(KeyCode::Up, KeyModifiers::NONE));
    app.handle_pending_input_key(key(KeyCode::Backspace, KeyModifiers::NONE));

    assert_eq!(
        *app.pending.queued_prompts(),
        VecDeque::from([prompt("second")])
    );
}

#[test]
fn rejected_steering_acceptance_becomes_a_follow_up_without_failing_the_turn() {
    let mut app = test_app();
    let queued = prompt("continue after this turn");
    let request = PendingInputRequest::Accept {
        prompt: queued.clone(),
        receipt: Box::pin(std::future::pending()),
    };
    let completion = PendingInputCompletion::Accepted(Err(rho_sdk::Error::InvalidHostResponse {
        message: "run completed before accepting steering input".into(),
    }));

    let failure = app.finish_pending_input_request(request, completion);

    assert_eq!(failure, None);
    assert!(app.pending.steering_prompts().is_empty());
    assert_eq!(*app.pending.queued_prompts(), VecDeque::from([queued]));
    assert_eq!(
        app.status(),
        "steer queued as follow-up: invalid host response: run completed before accepting steering input"
    );
}

#[test]
fn applied_event_removes_only_matching_steering() {
    let mut app = test_app();
    let applied = rho_sdk::SteeringId::new();
    let pending = rho_sdk::SteeringId::new();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: applied.clone(),
            prompt: prompt("applied"),
            delivered: false,
        });
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: pending.clone(),
            prompt: prompt("pending"),
            delivered: false,
        });

    app.record_applied_steering(&[applied]);

    assert_eq!(app.pending.accepted_steering().len(), 1);
    assert_eq!(app.pending.accepted_steering()[0].id, pending);
    assert!(matches!(
        app.history.entries(),
        [Entry::User(text)] if text == "applied"
    ));
}

// Covers: a rejected retraction must wait for SteeringApplied before changing the transcript.
// Owner: interactive TUI pending-input
#[test]
fn already_applied_retraction_keeps_the_live_stream_intact() {
    let mut app = test_app();
    let id = rho_sdk::SteeringId::new();
    app.pending
        .accepted_steering_mut()
        .push_back(AcceptedSteering {
            id: id.clone(),
            prompt: prompt("keep me"),
            delivered: false,
        });
    app.streams.current_stream_kind = Some(StreamKind::Assistant);
    app.streams
        .push_delta(StreamKind::Assistant, "held assistant tail", Instant::now());
    let request = PendingInputRequest::Retract {
        action: PendingInputAction::DiscardAccepted { id },
        receipt: Box::pin(std::future::pending()),
    };
    let completion =
        PendingInputCompletion::Retracted(Ok(rho_sdk::SteeringRetraction::AlreadyApplied));

    let failure = app.finish_pending_input_request(request, completion);

    assert_eq!(failure, None);
    assert_eq!(app.pending.accepted_steering().len(), 1);
    assert!(app.history.entries().is_empty());
    assert!(!app.streams.hold.is_empty());
    assert_eq!(app.streams.current_stream_kind, Some(StreamKind::Assistant));
}

// Covers: a late or empty SteeringApplied must not cut a live post-steer stream
// Owner: interactive TUI pending-input
#[test]
fn late_applied_event_does_not_cut_a_live_stream() {
    let mut app = test_app();
    app.streams.current_stream_kind = Some(StreamKind::Assistant);
    app.streams
        .push_delta(StreamKind::Assistant, "post-steer reply", Instant::now());

    app.record_applied_steering(&[rho_sdk::SteeringId::new()]);

    assert!(app.history.entries().is_empty());
    assert!(!app.streams.hold.is_empty());
    assert_eq!(app.streams.current_stream_kind, Some(StreamKind::Assistant));
}

// Covers: with a subagent, a live process, and a queued follow-up all on screen,
// the rendered frame must place pending input *below* both activity rails so a
// queued prompt sits next to the composer it will feed. This walks the real
// frame, so it fails if the paint order and the layout geometry ever disagree.
// Owner: interactive TUI pending-input
#[test]
fn pending_input_renders_below_subagent_and_process_rails() {
    use crate::{
        subagent::{RunState, RunStatus},
        tools::process::{LiveProcessSummary, State},
    };

    let mut app = test_app();
    let now = Instant::now();
    app.subagent_panel.ingest(
        vec![crate::tools::agent::SubagentSnapshot {
            id: "run-1".into(),
            agent_id: "explorer".into(),
            title: None,
            elapsed: std::time::Duration::from_secs(3),
            done: false,
            status: RunStatus {
                state: RunState::Running,
                last_activity: Some("read".into()),
                ..RunStatus::default()
            },
        }],
        now,
    );
    app.process_panel.ingest(
        vec![LiveProcessSummary {
            process_id: "proc-1".into(),
            command: "cargo build".into(),
            state: State::Running,
            elapsed_seconds: 5,
            quiet_seconds: None,
            exit_code: None,
        }],
        now,
    );
    app.pending.push_follow_up(prompt("queued follow up"));

    let rendered = app
        .active_lines_at_for_height(80, 24, now)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    let row_of = |needle: &str| {
        rendered
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} missing from frame:\n{}", rendered.join("\n")))
    };

    let subagent_row = row_of("explorer");
    let process_row = row_of("cargo build");
    let pending_row = row_of("queued follow up");
    assert!(
        subagent_row < process_row && process_row < pending_row,
        "expected subagent < process < pending, got {subagent_row} / {process_row} / \
         {pending_row}:\n{}",
        rendered.join("\n")
    );
}

// Covers: the activity tree closes on the last rail. With pending input painted
// underneath, the process rail must still terminate with `└` rather than `├`.
// Owner: interactive TUI pending-input
#[test]
fn process_rail_closes_the_tree_even_with_pending_input_below() {
    use crate::tools::process::{LiveProcessSummary, State};

    let mut app = test_app();
    let now = Instant::now();
    app.process_panel.ingest(
        vec![LiveProcessSummary {
            process_id: "proc-1".into(),
            command: "cargo build".into(),
            state: State::Running,
            elapsed_seconds: 5,
            quiet_seconds: None,
            exit_code: None,
        }],
        now,
    );
    app.pending.push_follow_up(prompt("queued follow up"));

    let rendered = app
        .active_lines_at_for_height(80, 24, now)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    let process_line = rendered
        .iter()
        .find(|line| line.contains("cargo build"))
        .unwrap_or_else(|| panic!("process row missing:\n{}", rendered.join("\n")));

    assert!(
        process_line.contains(crate::tui::activity::tree_connector(/*is_last*/ true)),
        "process rail must close the tree, got {process_line:?}"
    );
}
