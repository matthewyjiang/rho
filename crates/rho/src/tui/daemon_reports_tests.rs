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
    let opened: Vec<_> = [Some(SessionReport::Hooks), Some(SessionReport::Context), None]
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
