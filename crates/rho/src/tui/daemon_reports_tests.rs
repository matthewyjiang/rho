use pretty_assertions::assert_eq;
use rho_protocol::SessionReport;

use crate::tui::{tests::test_app, ComposerMode, PanelOverlay};

// Covers: a report's answer opens its overlay only while the person still
// waits on that report. One they asked for before another, or before
// moving on to a prompt, arrives late and must not open over what they are
// doing now (#1818).
// Owner: daemon TUI report overlays.
#[test]
fn only_the_awaited_report_opens_its_overlay() {
    let hooks = serde_json::json!({
        "enabled": false,
        "files": [],
        "hooks": [],
        "recent_activity": [],
    });
    let opened: Vec<_> = [
        Some(SessionReport::Hooks),
        Some(SessionReport::Context),
        None,
    ]
    .into_iter()
    .map(|awaited| {
        let mut app = test_app();
        app.awaited_daemon_report = awaited;
        app.show_daemon_report(SessionReport::Hooks, Ok(hooks.clone()));
        matches!(
            app.input_ui.composer(),
            ComposerMode::Panel(PanelOverlay::Hooks(_))
        )
    })
    .collect();

    assert_eq!(opened, vec![true, false, false]);
}

// Covers: `/info`'s read of a daemon session's compaction, which the daemon
// refuses during a turn, goes again exactly when a turn ended: opened in a
// turn, refused because one began after it was sent, or refused for a turn
// that already ended before the refusal arrived. A refusal with no turn
// ended since waits and never asks in a loop.
// Owner: daemon TUI report overlays.
#[test]
fn a_busy_compaction_read_asks_again_when_a_turn_ends() {
    use super::InfoCompactionRead::{Asked, AwaitingRest, Idle};
    let asked = Asked { turn_rested: false };
    let steps = [
        super::InfoCompactionRead::opened(/*in_turn*/ true),
        super::InfoCompactionRead::opened(/*in_turn*/ false),
        AwaitingRest.turn_rested(),
        asked.refused_as_busy(),
        asked.turn_rested(),
        Asked { turn_rested: true }.refused_as_busy(),
        Idle.turn_rested(),
        Idle.refused_as_busy(),
    ];

    assert_eq!(
        steps,
        [
            (AwaitingRest, false),
            (asked, true),
            (asked, true),
            (AwaitingRest, false),
            (Asked { turn_rested: true }, false),
            (asked, true),
            (Idle, false),
            (Idle, false),
        ]
    );
}
