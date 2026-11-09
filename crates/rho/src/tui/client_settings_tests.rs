use pretty_assertions::assert_eq;

use super::{TuiSettings, TUI_SETTINGS};
use crate::config::{default_fits_kind, StreamingMode};
use crate::keybindings::{EditingMode, Keybindings};

fn write(dir: &std::path::Path, relative: &str, text: &str) {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

// Covers: the TUI reads its client settings and its local theme and chords
// from their own files, with defaults for anything unset.
// Owner: TUI settings
#[test]
fn loads_client_and_local_files() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "clients/tui.toml",
        "zen_mode = true\noutput_streaming = \"paragraph\"\nprompt_history_limit = 0\nediting_mode = \"vim\"\n",
    );
    write(
        dir.path(),
        "tui.toml",
        "theme = \" dracula \"\n[keybindings]\nopen_editor = \"ctrl+e\"\n",
    );

    let mut keybindings = toml::from_str::<Keybindings>("open_editor = \"ctrl+e\"\n").unwrap();
    keybindings.editing_mode = EditingMode::Vim;
    assert_eq!(
        TuiSettings::load(dir.path()).unwrap(),
        TuiSettings {
            zen_mode: true,
            output_streaming: StreamingMode::Paragraph,
            prompt_history_limit: 0,
            theme: "dracula".into(),
            keybindings,
            ..TuiSettings::default()
        }
    );
}

// Covers: a missing directory is the defaults, not an error.
// Owner: TUI settings
#[test]
fn missing_files_are_the_defaults() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        TuiSettings::load(&dir.path().join("absent")).unwrap(),
        TuiSettings::default()
    );
}

// Covers: a bad client value fails startup with the key, limit, and value
// instead of being clamped; agent keys and `editing_mode` in the wrong file
// are rejected.
// Owner: TUI settings
#[test]
fn bad_client_settings_are_rejected() {
    for (text, expected) in [
        (
            "prompt_history_limit = 10001\n",
            "`prompt_history_limit`: must be at most 10000; got 10001",
        ),
        (
            "max_tool_output_lines = 0\n",
            "`max_tool_output_lines`: must be at least 1; got 0",
        ),
        ("theme = \"dark\"\n", "unknown key `theme`"),
        ("[behavior]\nrtk = true\n", "unknown key `behavior.rtk`"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "clients/tui.toml", text);
        let path = dir.path().join("clients").join("tui.toml");
        assert_eq!(
            format!("{:#}", TuiSettings::load(dir.path()).unwrap_err()),
            format!("invalid {}:\n{expected}", path.display()),
            "{text}"
        );
    }

    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "tui.toml",
        "[keybindings]\nediting_mode = \"vim\"\n",
    );
    assert!(TuiSettings::load(dir.path()).is_err());
}

// Covers: a declared range or choice list that rejects the setting's own
// default would fail the TUI's startup once the default is written.
// Owner: TUI settings
#[test]
fn every_client_default_fits_its_kind() {
    assert_eq!(
        TUI_SETTINGS
            .iter()
            .map(|setting| setting.spec())
            .filter(|spec| !default_fits_kind(spec))
            .map(|spec| spec.key.to_string())
            .collect::<Vec<_>>(),
        Vec::<String>::new()
    );
}

// Covers: the old ctrl+g jump binding migrates off the editor key, explicit
// distinct bindings are kept, and an explicit collision fails the load.
// Owner: TUI settings
#[test]
fn editor_and_jump_shortcut_conflict_policy() {
    for (chords, expected) in [
        ("jump_to_bottom = \"ctrl+g\"", Some(("ctrl+g", "ctrl+end"))),
        (
            "open_editor = \"alt+e\"\njump_to_bottom = \"ctrl+g\"",
            Some(("alt+e", "ctrl+g")),
        ),
        (
            "open_editor = \"ctrl+g\"\njump_to_bottom = \"ctrl+g\"",
            None,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "tui.toml",
            &format!("[keybindings]\n{chords}\n"),
        );

        let loaded = TuiSettings::load(dir.path()).ok().map(|settings| {
            (
                settings.keybindings.open_editor.to_string(),
                settings.keybindings.jump_to_bottom.to_string(),
            )
        });
        assert_eq!(
            loaded,
            expected.map(|(editor, jump)| (editor.to_owned(), jump.to_owned())),
            "{chords}"
        );
    }
}
