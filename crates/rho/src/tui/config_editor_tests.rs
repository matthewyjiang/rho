use super::*;
use crate::tui::line_editor::LineEditor;
use crate::tui::text_input::{TextInput, TextInputTarget};

// Covers: concurrent-agent edits clamp to the named max instead of accepting 1000.
// Owner: config editor
#[test]
fn agent_concurrency_clamps_to_named_max() {
    let mut over_max = ConfigNumberInput::new(ConfigNumberKey::AgentConcurrency, 1);
    over_max.editor.value = "1000".into();
    assert_eq!(
        over_max.parsed_value().unwrap(),
        crate::config::MAX_AGENT_CONCURRENCY
    );
}

// Covers: prompt history limit 0 is valid and the max is clamped in parse.
// Owner: config editor
#[test]
fn prompt_history_limit_allows_zero_and_clamps_max() {
    let zero = ConfigNumberInput::new(ConfigNumberKey::PromptHistoryLimit, 0);
    assert_eq!(zero.parsed_value().unwrap(), 0);

    let mut over_max = ConfigNumberInput::new(ConfigNumberKey::PromptHistoryLimit, 0);
    over_max.editor.value = "50000".into();
    assert_eq!(
        over_max.parsed_value().unwrap(),
        crate::config::MAX_PROMPT_HISTORY_LIMIT
    );
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

// Covers: toggling a config flag must persist the flipped value for the next session.
// Owner: config editor
#[test]
fn toggles_persist_for_the_next_session() {
    type ReadFlag = fn(&crate::config::Config) -> bool;
    let cases: [(&str, ConfigToggle, bool, ReadFlag); 4] = [
        (
            "subagents",
            ConfigToggle::EnableSubagents,
            false,
            |config| config.enable_subagents,
        ),
        (
            "cache miss notices",
            ConfigToggle::CacheMissNotices,
            true,
            |config| config.cache_miss_notices,
        ),
        ("zen mode", ConfigToggle::ZenMode, true, |config| {
            config.zen_mode
        }),
        (
            "xai image generation",
            ConfigToggle::XaiImageGeneration,
            false,
            |config| config.xai_image_generation,
        ),
    ];
    for (case, key, expected, persisted) in cases {
        let dir = tempfile::tempdir().unwrap();
        let repository = ConfigRepository::new(Some(dir.path().join("config.toml")));

        assert_eq!(toggle(&repository, key).unwrap(), expected, "{case}");
        assert_eq!(persisted(&repository.load().unwrap()), expected, "{case}");
    }
}

#[test]
fn editor_preserves_legacy_web_search_key_when_store_is_unavailable() {
    let store_error = rho_providers::credentials::CredentialError::StoreUnavailable("test".into());

    let (value, error) = resolve_web_search_editor_value(Err(store_error), Some("legacy-key"));

    assert_eq!(value.as_deref(), Some("legacy-key"));
    assert!(matches!(
        error,
        Some(rho_providers::credentials::CredentialError::StoreUnavailable(message)) if message == "test"
    ));
}
