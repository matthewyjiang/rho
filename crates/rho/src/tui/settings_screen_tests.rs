use pretty_assertions::assert_eq;

use super::{ConfigPage, SettingsUpdate};
use crate::tui::{
    advisor_status::AdvisorStatus, daemon_settings::tests::settings,
    model_names::tests::with_catalog_names, streaming_mode::StreamingMode, tests::test_app,
};
use crate::tui::{daemon_settings::SettingsNamespace, ComposerMode};

// Covers: once the daemon's settings are read, every advisor surface shows
// them, not the config the TUI started with, which `/advisor` and `/config`
// never change.
// Owner: interactive TUI settings screen (pure state).
#[test]
fn advisor_surfaces_follow_the_daemon_settings() {
    let mut app = test_app();
    let before = app.advisor_status();
    app.show_daemon_settings(SettingsUpdate::Loaded(Box::new(settings())));

    assert_eq!(
        (
            before,
            with_catalog_names(&[("openai", "gpt-5.5", "GPT-5.5")], || app.advisor_status())
        ),
        (
            AdvisorStatus::Off,
            AdvisorStatus::Reviewing {
                model: "GPT-5.5".into()
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

// Covers: a settings read that fails, as after a daemon restart, drops the
// settings read before, whose revision the restarted daemon no longer
// tracks and whose changes no longer arrive, instead of showing them stale.
// Owner: interactive TUI settings screen (pure state).
#[test]
fn a_failed_settings_read_drops_the_earlier_settings() {
    let mut app = test_app();
    app.show_daemon_settings(SettingsUpdate::Loaded(Box::new(settings())));
    app.show_daemon_settings(SettingsUpdate::Unreadable("connection lost".into()));

    assert_eq!(app.advisor_status(), AdvisorStatus::Off);
}

// Covers: a settings read that fails while `/config` shows one of their
// pages leaves that page, whose rows no longer save, rather than showing it
// stale.
// Owner: interactive TUI settings screen (pure state).
#[test]
fn a_failed_settings_read_leaves_an_open_settings_page() {
    let mut app = test_app();
    app.show_daemon_settings(SettingsUpdate::Loaded(Box::new(settings())));
    let category = app
        .config_page_picker(&ConfigPage::Category {
            namespace: SettingsNamespace::Agent,
            id: "context".into(),
        })
        .expect("a category of the settings read");
    app.input_ui.set_composer(ComposerMode::Picker(category));
    let page = |app: &crate::tui::App| match app.input_ui.composer() {
        ComposerMode::Picker(picker) => picker.config_page(),
        _ => None,
    };
    let before = page(&app);
    app.show_daemon_settings(SettingsUpdate::Unreadable("connection lost".into()));

    assert_eq!(
        (before, page(&app)),
        (
            Some(ConfigPage::Category {
                namespace: SettingsNamespace::Agent,
                id: "context".into(),
            }),
            None
        )
    );
}
