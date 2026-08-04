use ratatui::{backend::TestBackend, Terminal};

use super::*;
use crate::tui::activity::ActivityStatus;

#[test]
fn provider_stream_reset_clears_attempt_owned_tool_previews() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    app.handle_agent_event(
        ViewModelEvent::ToolCallUpdated {
            index: 0,
            call_id: Some(rho_sdk::ToolCallId::from_string("stale-call").unwrap()),
            card: Some(rho_tools::tool_card::ToolCard::new(
                rho_tools::tool_card::ToolStatus::Running,
                rho_tools::tool_card::ToolFamily::Default,
                rho_tools::tool_card::ToolHeader::call("stale preview", None),
            )),
        },
        &mut terminal,
    )
    .unwrap();
    assert_eq!(app.turn.tool_calls().live_entries().count(), 1);

    app.handle_agent_event(
        ViewModelEvent::ProviderStreamReset(crate::tui::activity::ProviderRetryHint {
            reason: rho_sdk::ProviderStreamResetReason::InvalidResponse,
        }),
        &mut terminal,
    )
    .unwrap();

    assert_eq!(app.turn.tool_calls().live_entries().count(), 0);
}

// Covers: zen mode must keep the live activity rail, not only subagent rows.
// Owner: pure activity-status policy.
#[test]
fn zen_mode_keeps_activity_status_while_turn_is_busy() {
    let mut app = test_app();
    app.info.runtime.zen_mode = true;
    app.turn.enter_provider_turn();
    app.turn.set_activity_phase(ActivityPhase::Thinking);

    assert!(!app.info.runtime.shows_work_chrome());
    assert_eq!(
        app.activity_status(),
        Some(ActivityStatus::Parent {
            phase: ActivityPhase::Thinking,
            retry: None,
        })
    );
}

// Covers: zen must not arm Thinking... on step start or reasoning deltas.
// Owner: pure reasoning-placeholder policy.
#[test]
fn zen_mode_does_not_show_thinking_placeholder_during_reasoning() {
    let mut app = test_app();
    app.info.runtime.zen_mode = true;
    app.info.runtime.show_reasoning_output = true;

    assert!(!app.info.runtime.shows_thinking_placeholder());
    assert!(!app.info.runtime.displays_reasoning_output());

    app.turn
        .reasoning_phase_mut()
        .begin_step(app.info.runtime.shows_thinking_placeholder());
    assert!(!app.turn.reasoning_phase().hidden_placeholder());

    app.turn
        .reasoning_phase_mut()
        .on_reasoning_delta(app.info.runtime.shows_thinking_placeholder());
    assert!(app.turn.reasoning_phase().has_started());
    assert!(!app.turn.reasoning_phase().hidden_placeholder());

    // Non-zen with reasoning text off still wants Thinking...
    app.info.runtime.zen_mode = false;
    app.info.runtime.show_reasoning_output = false;
    assert!(app.info.runtime.shows_thinking_placeholder());
    app.turn
        .reasoning_phase_mut()
        .begin_step(app.info.runtime.shows_thinking_placeholder());
    assert!(app.turn.reasoning_phase().hidden_placeholder());
}
