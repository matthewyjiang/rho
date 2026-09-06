use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::{
    super::{file_picker::FilePaletteEntry, palette::ActivePalette, tests::test_app, App},
    shell_quote, shell_word_candidates_in, ShellFamily,
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
    app.info
        .services
        .config_repository
        .update(|config| config.inline_shell = "bash".into())
        .unwrap();
    app.info.runtime.cwd = workspace.path().to_path_buf();
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text(typed);
    (app, workspace)
}

// Covers: the lister must resolve the directory part of a word the way the
// shell will, or completion writes a path the command then cannot open.
// Absolute, `~/`, and `../` each take a different resolver branch, and a
// naive `rsplit_once('/')` conflates "no slash" with "leading slash".
// Owner: TUI shell palette candidate policy.
#[test]
fn candidates_resolve_the_directory_part_like_a_shell() {
    let workspace = tempdir().unwrap();
    let home = tempdir().unwrap();
    let cwd = workspace.path().join("work");
    std::fs::create_dir_all(cwd.join("src")).unwrap();
    std::fs::write(cwd.join("src/lib.rs"), "").unwrap();
    std::fs::write(workspace.path().join("sibling.txt"), "").unwrap();
    std::fs::write(home.path().join("dotfile.rc"), "").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(cwd.join("src"), cwd.join("linked")).unwrap();

    let absolute_src = cwd.join("src").display().to_string();
    let absolute_word = format!("{absolute_src}/li");
    let absolute_expected = format!("{absolute_src}/lib.rs");
    let cases: &[(&str, &str, &[&str])] = &[
        ("bare word lists cwd", "sr", &["src/"]),
        ("dir word lists inside it", "src/li", &["src/lib.rs"]),
        ("parent dir", "../sib", &["../sibling.txt"]),
        ("home dir", "~/dot", &["~/dotfile.rc"]),
        (
            "absolute dir stays absolute",
            &absolute_word,
            &[&absolute_expected],
        ),
        ("nonexistent dir offers nothing", "nope/", &[]),
        #[cfg(unix)]
        ("symlinked dir is a dir", "link", &["linked/"]),
    ];
    for (name, word, expected) in cases {
        assert_eq!(
            shell_word_candidates_in(&cwd, word, Some(home.path())).as_slice(),
            *expected,
            "{name}: `{word}`"
        );
    }
}

/// Workspace paths the open palette offers, or `None` while it is closed.
fn open_paths(app: &mut App) -> Option<Vec<String>> {
    match app.active_palette() {
        Some(ActivePalette::File(matches)) => Some(
            matches
                .rows(0, matches.len())
                .map(|(_, entry)| match entry {
                    FilePaletteEntry::WorkspaceFile(path) => path,
                    FilePaletteEntry::McpResource(resource) => {
                        panic!("shell completion offered a resource: {resource:?}")
                    }
                })
                .collect(),
        ),
        _ => None,
    }
}

// Covers: Tab in shell mode must complete the word under the cursor like a
// shell does: one component at a time, a lone match lands at once, several
// open a list, and nothing leaves the composer alone. Quoting is part of the
// contract because a completed path the shell then splits is worse than no
// completion.
// Owner: TUI shell palette policy.
#[test]
fn tab_completes_one_component_of_the_word_under_cursor() {
    struct Case {
        name: &'static str,
        files: &'static [&'static str],
        typed: &'static str,
        text_after_tab: &'static str,
        palette_after_tab: Option<&'static [&'static str]>,
    }
    let cases = [
        Case {
            name: "literal tilde directory does not expand to home",
            files: &["~/notes.md"],
            typed: "cat ",
            text_after_tab: "cat ./~/",
            palette_after_tab: None,
        },
        Case {
            name: "literal named-home entry does not expand",
            files: &["~root"],
            typed: "cat ",
            text_after_tab: "cat ./~root ",
            palette_after_tab: None,
        },
        Case {
            name: "single file inserts with a trailing space",
            files: &["notes.md"],
            typed: "cat not",
            text_after_tab: "cat notes.md ",
            palette_after_tab: None,
        },
        Case {
            name: "single directory inserts with a slash and no space",
            files: &["crates/rho/lib.rs"],
            typed: "ls cra",
            text_after_tab: "ls crates/",
            palette_after_tab: None,
        },
        Case {
            name: "a directory word lists its own entries, not the whole tree",
            files: &["src/a.rs", "src/nested/deep.rs"],
            typed: "cat src/",
            text_after_tab: "cat src/",
            palette_after_tab: Some(&["src/a.rs", "src/nested/"]),
        },
        Case {
            name: "a bare word lists the cwd, not the whole tree",
            files: &["src/a.rs", "README.md"],
            typed: "ls ",
            text_after_tab: "ls ",
            palette_after_tab: Some(&["README.md", "src/"]),
        },
        Case {
            name: "hidden entries appear only for a dot-prefixed component",
            files: &[".env", "env.txt"],
            typed: "cat .",
            text_after_tab: "cat .env ",
            palette_after_tab: None,
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
        assert!(app.handle_file_palette_key(key(KeyCode::Tab)).unwrap());
        assert_eq!(app.input_ui.text(), case.text_after_tab, "{}", case.name);
        assert_eq!(
            open_paths(&mut app),
            case.palette_after_tab
                .map(|paths| paths.iter().map(|path| (*path).to_string()).collect()),
            "{}",
            case.name
        );
        assert!(app.input_ui.shell_mode().is_some(), "{}", case.name);
    }
}

// Covers: repeated Tab walks a nested path one directory per press and ends
// on the file with a space, instead of jumping to the leaf in one go.
// Owner: TUI shell palette policy.
#[test]
fn repeated_tab_descends_one_directory_per_press() {
    let (mut app, _workspace) = shell_app(&["crates/rho/src/lib.rs"], "cat cr");
    let expected = [
        "cat crates/",
        "cat crates/rho/",
        "cat crates/rho/src/",
        "cat crates/rho/src/lib.rs ",
    ];
    for text in expected {
        assert!(app.handle_file_palette_key(key(KeyCode::Tab)).unwrap());
        assert_eq!(app.input_ui.text(), text);
        assert_eq!(open_paths(&mut app), None, "each step had one match");
    }
}

// Covers: the open list must follow the composer. Typing narrows it, Down
// moves the pick, Enter inserts it, and Esc closes the list without leaving
// shell mode (the same key one level up would exit the mode).
// Owner: TUI shell palette policy.
#[test]
fn open_list_narrows_navigates_accepts_and_dismisses() {
    let (mut app, _workspace) = shell_app(&["src/alpha.rs", "src/beta.rs"], "cat src/");
    assert!(app.handle_file_palette_key(key(KeyCode::Tab)).unwrap());
    assert_eq!(
        open_paths(&mut app),
        Some(vec!["src/alpha.rs".into(), "src/beta.rs".into()])
    );

    app.insert_input_char('b');
    assert_eq!(open_paths(&mut app), Some(vec!["src/beta.rs".into()]));

    app.backspace_input();
    assert!(app.handle_file_palette_key(key(KeyCode::Down)).unwrap());
    assert!(app.handle_file_palette_key(key(KeyCode::Enter)).unwrap());
    assert_eq!(app.input_ui.text(), "cat src/beta.rs ");
    assert_eq!(open_paths(&mut app), None);

    app.insert_input_text("src/");
    assert!(app.handle_file_palette_key(key(KeyCode::Tab)).unwrap());
    assert!(open_paths(&mut app).is_some());
    assert!(app.handle_file_palette_key(key(KeyCode::Esc)).unwrap());
    assert_eq!(open_paths(&mut app), None);
    assert!(app.input_ui.shell_mode().is_some());
    assert_eq!(app.input_ui.text(), "cat src/beta.rs src/");
}

// Covers: the list is anchored to the word Tab opened it on. Moving the
// cursor into another word must close it rather than silently retarget.
// Owner: TUI shell palette policy.
#[test]
fn open_list_closes_when_cursor_leaves_the_word() {
    let (mut app, _workspace) = shell_app(&["src/a.rs", "src/b.rs"], "cat src/");
    assert!(app.handle_file_palette_key(key(KeyCode::Tab)).unwrap());
    assert!(open_paths(&mut app).is_some());

    app.input_ui.set_cursor(2); // inside "cat"
    app.clamp_file_selection();
    assert_eq!(open_paths(&mut app), None);
}

// Covers: Tab outside shell mode is not ours; the `@` palette and picker
// filters own it there.
// Owner: TUI shell palette policy.
#[test]
fn tab_outside_shell_mode_is_ignored() {
    let mut app = test_app();
    app.insert_input_text("cat src/");
    assert!(!app.handle_file_palette_key(key(KeyCode::Tab)).unwrap());
    assert_eq!(app.input_ui.text(), "cat src/");
}

#[test]
fn shell_quote_wraps_only_unsafe_paths() {
    let cases = [
        ("src/main.rs", "src/main.rs"),
        ("docs/my notes.md", "'docs/my notes.md'"),
        ("it's.txt", "'it'\\''s.txt'"),
        ("a$b", "'a$b'"),
        ("~root", "'~root'"),
        ("~", "'~'"),
        ("./~/", "./~/"),
        ("~/my notes.md", "~/'my notes.md'"),
    ];
    for (input, expected) in cases {
        assert_eq!(shell_quote(input), expected, "{input}");
    }
}

// Covers: never insert POSIX quoting into a shell with incompatible syntax.
// Owner: shell completion eligibility policy.
#[test]
fn completion_requires_a_supported_shell() {
    for (shell, supported) in [
        ("bash", true),
        ("ash", true),
        ("mksh", true),
        ("/bin/dash", true),
        ("ksh.exe", true),
        ("busybox", true),
        ("/bin/zsh", true),
        ("fish", false),
        ("sh.exe", true),
        ("powershell", false),
        ("pwsh.exe", false),
        ("cmd", false),
        ("nu", false),
        ("elvish", false),
        ("/usr/bin/elvish", false),
        ("custom-shell.exe", false),
    ] {
        assert_eq!(
            ShellFamily::for_executable(shell).supports_path_completion(),
            supported,
            "{shell}"
        );
    }
}
