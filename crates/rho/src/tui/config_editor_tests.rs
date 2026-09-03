use super::*;
use crate::tui::line_editor::LineEditor;
use crate::tui::text_input::{TextInput, TextInputTarget};

// Covers: concurrent-agent edits clamp to the named max instead of accepting 1000.
// Owner: config editor
#[test]
fn agent_concurrency_clamps_to_named_max() {
    let mut over_max = ConfigNumberInput::new(ConfigNumberKey::AgentConcurrency, 1);
    over_max.value = "1000".into();
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
    over_max.value = "50000".into();
    assert_eq!(
        over_max.parsed_value().unwrap(),
        crate::config::MAX_PROMPT_HISTORY_LIMIT
    );
}

#[test]
fn number_input_accepts_only_ascii_digits() {
    let mut input = ConfigNumberInput::new(ConfigNumberKey::MaxOutputBytes, 42);

    input.insert_text("a1-２3");

    assert_eq!(input.value, "4213");
    assert_eq!(input.cursor, 4);
}

#[test]
fn text_input_strips_line_breaks_and_edits_at_character_cursor() {
    let mut input = TextInput::config_api_key(ConfigTextKey::Exa, Some("aé".into()));
    input.editor.cursor = 1;

    input.editor.insert_text("x\ny\r");
    input.editor.delete();

    assert_eq!(input.editor.value, "axy");
    assert_eq!(input.editor.cursor, 3);
    assert!(matches!(
        input.target,
        TextInputTarget::ConfigApiKey(ConfigTextKey::Exa)
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

#[test]
fn subagent_toggle_persists_for_the_next_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let repository = ConfigRepository::new(Some(path));

    let enabled = toggle(&repository, ConfigToggle::EnableSubagents).unwrap();

    assert!(!enabled);
    assert!(!repository.load().unwrap().enable_subagents);
}

// Covers: toggling cache miss notices must persist for the next session.
// Owner: config editor
#[test]
fn cache_miss_notices_toggle_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let repository = ConfigRepository::new(Some(path));

    let enabled = toggle(&repository, ConfigToggle::CacheMissNotices).unwrap();

    assert!(enabled);
    assert!(repository.load().unwrap().cache_miss_notices);
}

#[test]
fn zen_mode_toggle_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let repository = ConfigRepository::new(Some(path));

    let enabled = toggle(&repository, ConfigToggle::ZenMode).unwrap();

    assert!(enabled);
    assert!(repository.load().unwrap().zen_mode);
}

#[test]
fn xai_image_generation_toggle_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let repository = ConfigRepository::new(Some(path));

    let enabled = toggle(&repository, ConfigToggle::XaiImageGeneration).unwrap();

    assert!(!enabled);
    assert!(!repository.load().unwrap().xai_image_generation);
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
