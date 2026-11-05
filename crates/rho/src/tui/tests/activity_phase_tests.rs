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
            background: crate::tui::activity::BackgroundCounts::default(),
        })
    );
}
