use super::*;
use crate::tui::line_editor::LineEditor;
use crate::tui::text_input::{TextInput, TextInputTarget};

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

    let mutation = toggle(&repository, ConfigToggle::EnableSubagents).unwrap();

    assert_eq!(mutation, ConfigMutation::EnableSubagents(false));
    assert!(!repository.load().unwrap().enable_subagents);
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
