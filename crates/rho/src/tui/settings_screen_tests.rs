use pretty_assertions::assert_eq;

use super::SettingsUpdate;
use crate::tui::{
    advisor_status::AdvisorStatus, daemon_settings::tests::settings, streaming_mode::StreamingMode,
    tests::test_app,
};

// Covers: once the daemon's settings are read, every advisor surface shows
// them, not the in-process runtime the TUI started with, which `/advisor`
// and `/config` never change in a daemon session.
// Owner: interactive TUI settings screen (pure state).
#[test]
fn advisor_surfaces_follow_the_daemon_settings() {
    let mut app = test_app();
    let before = app.advisor_status();
    app.show_daemon_settings(SettingsUpdate::Loaded(Box::new(settings())));

    assert_eq!(
        (before, app.advisor_status()),
        (
            AdvisorStatus::Off,
            AdvisorStatus::Reviewing {
                model: "openai/gpt-5.5".into()
            }
        )
    );
}

// Covers: the streaming shortcut's unsaved mode survives reading the saved
// settings again, as after a daemon restart, instead of reverting to the
// saved one.
// Owner: interactive TUI settings screen (pure state).
#[test]
fn reading_the_settings_again_keeps_the_shortcuts_streaming_mode() {
    let mut app = test_app();
    app.set_streaming_mode(StreamingMode::Off);
    app.show_daemon_settings(SettingsUpdate::Loaded(Box::new(settings())));

    assert_eq!(app.streams.mode, StreamingMode::Off);
}
