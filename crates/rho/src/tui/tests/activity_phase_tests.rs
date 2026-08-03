use ratatui::{backend::TestBackend, Terminal};

use super::*;

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
            retry_after: None,
        }),
        &mut terminal,
    )
    .unwrap();

    assert_eq!(app.turn.tool_calls().live_entries().count(), 0);
}
