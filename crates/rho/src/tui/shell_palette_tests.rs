use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::{
    super::{palette::ActivePalette, tests::test_app, App},
    shell_quote,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A shell-mode app over a workspace holding `files`, with `typed` in the
/// composer and the cursor at its end.
fn shell_app(files: &[&str], typed: &str) -> (App, tempfile::TempDir) {
    let workspace = tempdir().unwrap();
    for file in files {
        let path = workspace.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    let mut app = test_app();
    app.info.runtime.cwd = workspace.path().to_path_buf();
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text(typed);
    (app, workspace)
}

fn open_paths(app: &mut App) -> Option<Vec<String>> {
    match app.active_palette() {
        Some(ActivePalette::ShellPath(matches)) => Some(matches.as_slice().to_vec()),
        _ => None,
    }
}

// Covers: Tab in shell mode must complete the word under the cursor like a
// shell does: a lone match lands at once, several open a list, and nothing
// leaves the composer alone. Quoting is part of the contract because a
// completed path the shell then splits is worse than no completion.
// Owner: TUI shell palette policy.
#[test]
fn tab_completes_word_under_cursor() {
    struct Case {
        name: &'static str,
        files: &'static [&'static str],
        typed: &'static str,
        text_after_tab: &'static str,
        palette_after_tab: Option<&'static [&'static str]>,
    }
    let cases = [
        Case {
            name: "single match inserts with a trailing space",
            files: &["notes.md"],
            typed: "cat not",
            text_after_tab: "cat notes.md ",
            palette_after_tab: None,
        },
        Case {
            name: "several matches open the list and keep the text",
            files: &["src/a.rs", "src/b.rs"],
            typed: "cat src/",
            text_after_tab: "cat src/",
            palette_after_tab: Some(&["src/a.rs", "src/b.rs"]),
        },
        Case {
            name: "no match leaves the composer alone",
            files: &["notes.md"],
            typed: "cat zzz",
            text_after_tab: "cat zzz",
            palette_after_tab: None,
        },
        Case {
            name: "a path with spaces is quoted",
            files: &["my notes.md"],
            typed: "cat my",
            text_after_tab: "cat 'my notes.md' ",
            palette_after_tab: None,
        },
    ];

    for case in cases {
        let (mut app, _workspace) = shell_app(case.files, case.typed);
        assert!(app.handle_shell_palette_key(key(KeyCode::Tab)).unwrap());
        assert_eq!(app.input_ui.text(), case.text_after_tab, "{}", case.name);
        assert_eq!(
            open_paths(&mut app).as_deref(),
            case.palette_after_tab
                .map(|paths| paths
                    .iter()
                    .map(|path| (*path).to_string())
                    .collect::<Vec<_>>())
                .as_deref(),
            "{}",
            case.name
        );
        assert!(app.input_ui.shell_mode().is_some(), "{}", case.name);
    }
}

// Covers: the open list must follow the composer. Typing narrows it, Down
// moves the pick, Enter inserts it, and Esc closes the list without leaving
// shell mode (the same key one level up would exit the mode).
// Owner: TUI shell palette policy.
#[test]
fn open_list_narrows_navigates_accepts_and_dismisses() {
    let (mut app, _workspace) = shell_app(&["src/alpha.rs", "src/beta.rs"], "cat src/");
    assert!(app.handle_shell_palette_key(key(KeyCode::Tab)).unwrap());
    assert_eq!(
        open_paths(&mut app),
        Some(vec!["src/alpha.rs".into(), "src/beta.rs".into()])
    );

    app.insert_input_char('b');
    assert_eq!(open_paths(&mut app), Some(vec!["src/beta.rs".into()]));

    app.backspace_input();
    assert!(app.handle_shell_palette_key(key(KeyCode::Down)).unwrap());
    assert!(app.handle_shell_palette_key(key(KeyCode::Enter)).unwrap());
    assert_eq!(app.input_ui.text(), "cat src/beta.rs ");
    assert_eq!(open_paths(&mut app), None);

    app.insert_input_text("src/");
    assert!(app.handle_shell_palette_key(key(KeyCode::Tab)).unwrap());
    assert!(open_paths(&mut app).is_some());
    assert!(app.handle_shell_palette_key(key(KeyCode::Esc)).unwrap());
    assert_eq!(open_paths(&mut app), None);
    assert!(app.input_ui.shell_mode().is_some());
    assert_eq!(app.input_ui.text(), "cat src/beta.rs src/");
}

// Covers: Tab outside shell mode is not ours; the `@` palette and picker
// filters own it there.
// Owner: TUI shell palette policy.
#[test]
fn tab_outside_shell_mode_is_ignored() {
    let mut app = test_app();
    app.insert_input_text("cat src/");
    assert!(!app.handle_shell_palette_key(key(KeyCode::Tab)).unwrap());
    assert_eq!(app.input_ui.text(), "cat src/");
}

#[test]
fn shell_quote_wraps_only_unsafe_paths() {
    let cases = [
        ("src/main.rs", "src/main.rs"),
        ("docs/my notes.md", "'docs/my notes.md'"),
        ("it's.txt", "'it'\\''s.txt'"),
        ("a$b", "'a$b'"),
    ];
    for (input, expected) in cases {
        assert_eq!(shell_quote(input), expected, "{input}");
    }
}
