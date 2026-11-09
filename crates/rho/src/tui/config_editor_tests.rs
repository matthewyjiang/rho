use super::*;
use crate::tui::line_editor::LineEditor;
use crate::tui::text_input::{TextInput, TextInputTarget};

// Covers: number edits clamp to the named max instead of accepting any value
// (1000 concurrent agents), while an in-range zero stays valid.
// Owner: config editor
#[test]
fn number_inputs_clamp_to_named_max() {
    for (key, typed, expected) in [
        (
            ConfigNumberKey::AgentConcurrency,
            "1000",
            crate::config::MAX_AGENT_CONCURRENCY,
        ),
        (ConfigNumberKey::PromptHistoryLimit, "0", 0),
        (
            ConfigNumberKey::PromptHistoryLimit,
            "50000",
            crate::tui::client_settings::MAX_PROMPT_HISTORY_LIMIT,
        ),
    ] {
        let mut input = ConfigNumberInput::new(key, 1);
        input.editor.value = typed.into();
        assert_eq!(input.parsed_value().unwrap(), expected, "{key:?} {typed}");
    }
}

#[test]
fn number_input_accepts_only_ascii_digits() {
    let mut input = ConfigNumberInput::new(ConfigNumberKey::MaxOutputBytes, 42);

    input.insert_text("a1-２3");

    assert_eq!(input.editor.value, "4213");
    assert_eq!(input.editor.cursor, 4);
}

#[test]
fn text_input_strips_line_breaks_and_edits_at_character_cursor() {
    let mut input = TextInput::config_api_key(
        rho_providers::credentials::WebSearchCredential::Exa,
        Some("aé".into()),
    );
    input.editor.cursor = 1;

    input.editor.insert_text("x\ny\r");
    input.editor.delete();

    assert_eq!(input.editor.value, "axy");
    assert_eq!(input.editor.cursor, 3);
    assert!(matches!(
        input.target,
        TextInputTarget::ConfigApiKey(rho_providers::credentials::WebSearchCredential::Exa)
    ));
}

#[test]
fn editor_cursor_navigation_is_unicode_safe() {
    let mut editor = LineEditor::new("aéz");

    editor.move_cursor_left();
    editor.backspace();
    editor.move_cursor_home();
    editor.move_cursor_right();
    editor.insert_char('x');
    editor.move_cursor_end();

    assert_eq!(editor.value, "axz");
    assert_eq!(editor.cursor, 3);
}

// Covers: toggling an agent setting must persist the flipped value for the
// next session, while a client setting changes only the running TUI.
// Owner: config editor
#[test]
fn toggles_persist_agent_settings_and_keep_client_settings_live() {
    type ReadFlag = fn(&crate::config::Config) -> bool;
    let agent_cases: [(&str, ConfigToggle, bool, ReadFlag); 3] = [
        (
            "workspace rewind",
            ConfigToggle::WorkspaceRewind,
            false,
            |config| config.workspace_rewind,
        ),
        (
            "subagents",
            ConfigToggle::EnableSubagents,
            false,
            |config| config.enable_subagents,
        ),
        (
            "xai image generation",
            ConfigToggle::XaiImageGeneration,
            false,
            |config| config.xai_image_generation,
        ),
    ];
    for (case, key, expected, persisted) in agent_cases {
        let dir = tempfile::tempdir().unwrap();
        let repository = ConfigRepository::new(Some(dir.path().join("config.toml")));
        let mut settings = TuiSettings::default();

        assert_eq!(
            toggle(&repository, &mut settings, key).unwrap(),
            expected,
            "{case}"
        );
        assert_eq!(persisted(&repository.load().unwrap()), expected, "{case}");
        assert_eq!(settings, TuiSettings::default(), "{case}");
    }

    type ReadClient = fn(&TuiSettings) -> bool;
    let client_cases: [(&str, ConfigToggle, bool, ReadClient); 5] = [
        (
            "cache miss notices",
            ConfigToggle::CacheMissNotices,
            true,
            |settings| settings.cache_miss_notices,
        ),
        (
            "header hints",
            ConfigToggle::ShowHeaderHints,
            false,
            |settings| settings.show_header_hints,
        ),
        (
            "notifications",
            ConfigToggle::Notifications,
            false,
            |settings| settings.notifications,
        ),
        ("zen mode", ConfigToggle::ZenMode, true, |settings| {
            settings.zen_mode
        }),
        ("vim mode", ConfigToggle::VimMode, true, |settings| {
            settings.keybindings.editing_mode == crate::keybindings::EditingMode::Vim
        }),
    ];
    for (case, key, expected, live) in client_cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let repository = ConfigRepository::new(Some(path.clone()));
        let mut settings = TuiSettings::default();

        assert_eq!(
            toggle(&repository, &mut settings, key).unwrap(),
            expected,
            "{case}"
        );
        assert_eq!(live(&settings), expected, "{case}");
        assert!(!path.exists(), "{case}");
    }
}
