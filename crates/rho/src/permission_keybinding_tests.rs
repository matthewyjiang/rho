use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;

use super::{Keybindings, ReservedComposerKey};

// Covers: remapping the cycle key must not silently shadow another shortcut,
// including modifier-insensitive Tab/scroll handlers, composer text and editing
// keys, while an omitted cycle key yields Alt+M to an existing binding and a
// remapped cycle key keeps the chord over undo and redo, which run last.
// Owner: keybinding configuration parsing
#[test]
fn permission_cycle_remaps_reject_collisions() {
    let cases = [
        ("cycle_permission_mode = \"alt+m\"", Ok(Some("alt+m"))),
        ("cycle_permission_mode = \"alt+n\"", Ok(Some("alt+n"))),
        ("cycle_permission_mode = \"ctrl+n\"", Ok(Some("ctrl+n"))),
        ("cycle_permission_mode = \"tab\"", Err(())),
        ("cycle_permission_mode = \"ctrl+tab\"", Err(())),
        ("cycle_permission_mode = \"alt+tab\"", Err(())),
        ("cycle_permission_mode = \"ctrl+alt+tab\"", Err(())),
        ("cycle_permission_mode = \"ctrl+shift+tab\"", Err(())),
        ("cycle_permission_mode = \"alt+shift+tab\"", Err(())),
        ("cycle_permission_mode = \"ctrl+alt+shift+tab\"", Err(())),
        ("cycle_permission_mode = \"pageup\"", Err(())),
        ("cycle_permission_mode = \"pagedown\"", Err(())),
        ("cycle_permission_mode = \"alt+pageup\"", Err(())),
        ("cycle_permission_mode = \"ctrl+shift+pagedown\"", Err(())),
        ("cycle_permission_mode = \"m\"", Err(())),
        ("cycle_permission_mode = \"shift+m\"", Err(())),
        ("cycle_permission_mode = \"?\"", Err(())),
        ("cycle_permission_mode = \"é\"", Err(())),
        ("cycle_permission_mode = \"ctrl+p\"", Err(())),
        ("cycle_permission_mode = \"alt+s\"", Err(())),
        ("cycle_permission_mode = \"ctrl+enter\"", Err(())),
        ("cycle_permission_mode = \"shift+tab\"", Err(())),
        ("cycle_permission_mode = \"alt+v\"", Err(())),
        ("cycle_permission_mode = \"enter\"", Err(())),
        ("cycle_permission_mode = \"shift+enter\"", Err(())),
        ("cycle_permission_mode = \"esc\"", Err(())),
        ("cycle_permission_mode = \"alt+esc\"", Err(())),
        ("cycle_permission_mode = \"backspace\"", Err(())),
        ("cycle_permission_mode = \"up\"", Err(())),
        (
            "cycle_permission_mode = \"ctrl+down\"",
            Ok(Some("ctrl+down")),
        ),
        ("", Ok(Some("alt+m"))),
        ("reset_conversation = \"alt+m\"", Ok(None)),
        ("toggle_tool_output = \"alt+m\"", Ok(None)),
        ("undo = \"alt+m\"", Ok(None)),
        ("cycle_permission_mode = \"ctrl+z\"", Ok(Some("ctrl+z"))),
        (
            "undo = \"ctrl+z\"\ncycle_permission_mode = \"ctrl+z\"",
            Ok(Some("ctrl+z")),
        ),
        (
            "reset_conversation = \"alt+m\"\ncycle_permission_mode = \"alt+m\"",
            Err(()),
        ),
        (
            "toggle_tool_output = \"alt+m\"\ncycle_permission_mode = \"alt+n\"",
            Ok(Some("alt+n")),
        ),
    ];
    for (text, expected) in cases {
        let parsed = toml::from_str::<Keybindings>(text)
            .map(|keys| keys.cycle_permission_mode.map(|key| key.to_string()))
            .map_err(|_| ());
        assert_eq!(parsed, expected.map(|key| key.map(str::to_owned)), "{text}");
    }
}

// Covers: terminals report Shift+Tab as either Tab+Shift or BackTab, sometimes
// omitting Shift on BackTab; neither encoding may escape the reserved class.
// Owner: pure composer key classification (BackTab is not a config spelling)
#[test]
fn reserved_tab_events_normalize_shift() {
    for modifiers in [
        KeyModifiers::NONE,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ] {
        for (code, modifiers, expected) in [
            (KeyCode::Tab, modifiers, ReservedComposerKey::TabCompletion),
            (
                KeyCode::Tab,
                modifiers | KeyModifiers::SHIFT,
                ReservedComposerKey::ReasoningCycle,
            ),
            (
                KeyCode::BackTab,
                modifiers,
                ReservedComposerKey::ReasoningCycle,
            ),
            (
                KeyCode::BackTab,
                modifiers | KeyModifiers::SHIFT,
                ReservedComposerKey::ReasoningCycle,
            ),
        ] {
            let key = KeyEvent::new(code, modifiers);
            assert_eq!(
                ReservedComposerKey::from_key(key),
                Some(expected),
                "{key:?}"
            );
        }
    }
}
