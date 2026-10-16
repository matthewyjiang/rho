use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{complete_mention, mention_under_cursor, MentionEdit, SkillMentionCursor};
use crate::tui::{tests::test_app, App};

/// `|` marks the cursor in a case's text.
fn at_cursor(marked: &str) -> (String, usize) {
    let cursor = marked.chars().position(|ch| ch == '|').unwrap();
    (marked.replacen('|', "", 1), cursor)
}

fn apply(text: &str, edit: MentionEdit) -> (String, usize) {
    let chars = text.chars().collect::<Vec<_>>();
    let before = chars[..edit.replace.start].iter().collect::<String>();
    let after = chars[edit.replace.end..].iter().collect::<String>();
    (format!("{before}{}{after}", edit.inserted), edit.cursor)
}

// Covers: the palette offers skills only while the cursor is inside a
// whitespace-delimited `$` token, and completion rewrites only that token
// Owner: TUI skill mention completion
#[test]
fn mention_completion_rewrites_only_the_token_under_the_cursor() {
    // (label, text with cursor, completed text with cursor; None when no
    // mention is under the cursor)
    let cases = [
        ("bare sigil", "use $|", Some("use $review |")),
        ("typed prefix", "use $re|", Some("use $review |")),
        ("cursor mid-token", "$re|vi then", Some("$review |then")),
        ("later prose kept", "a $r| b $c", Some("a $review |b $c")),
        ("before the sigil", "use |$re", None),
        ("not token start", "x$re|", None),
        ("other token", "$re next|", None),
        ("punctuation ends a name", "$re.|", None),
        ("command token", "/skill:re|", None),
    ];

    for (label, marked, expected) in cases {
        let (text, cursor) = at_cursor(marked);
        let completed =
            mention_under_cursor(&text, cursor).map(|SkillMentionCursor { token, .. }| {
                apply(&text, complete_mention(&text, &token, "review"))
            });
        assert_eq!(completed, expected.map(at_cursor), "{label}");
    }
}

/// An app whose workspace has the skills `review` and `2fa`.
fn app_with_skills() -> (App, TempDir) {
    let root = TempDir::new().unwrap();
    for name in ["review", "2fa"] {
        let dir = root.path().join(".agents/skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\nbody\n"),
        )
        .unwrap();
    }
    let mut app = test_app();
    app.info.runtime.cwd = root.path().to_path_buf();
    (app, root)
}

// Covers: completing a mention before a collapsed paste must keep the paste
// expanding at its marker, not inside the rewritten token
// Owner: TUI skill mention completion
#[test]
fn mention_completion_keeps_a_later_paste_aligned() {
    let (mut app, _root) = app_with_skills();
    let paste = "line\n".repeat(20);
    app.insert_input_text("use $re ");
    app.insert_pasted_input_text(&paste);
    assert_eq!(
        app.input_ui.paste_segments().len(),
        1,
        "paste must collapse"
    );
    app.input_ui.set_cursor("use $re".len());
    let choice = app.selected_command().unwrap();

    app.complete_command_choice(&choice);

    assert_eq!(app.expanded_input(), format!("use $review {paste}"));
}

// Covers: Enter must not turn a typed price into a skill mention
// Owner: TUI skill mention completion
#[test]
fn enter_completes_a_mention_only_once_the_name_could_be_one() {
    // (typed text, whether Enter completes the highlighted row)
    let cases = [
        ("it costs $", false),
        ("it costs $2", false),
        ("it costs $2f", true),
        ("use $re", true),
    ];

    for (text, completes) in cases {
        let (mut app, _root) = app_with_skills();
        app.insert_input_text(text);
        let choice = app.selected_command().unwrap();
        assert_eq!(app.enter_completes_choice(&choice), completes, "{text}");
    }
}
