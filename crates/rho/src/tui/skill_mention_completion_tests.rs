use pretty_assertions::assert_eq;

use super::{complete_mention, mention_under_cursor, SkillMentionCursor};

/// `|` marks the cursor in a case's text.
fn at_cursor(marked: &str) -> (String, usize) {
    let cursor = marked.chars().position(|ch| ch == '|').unwrap();
    (marked.replacen('|', "", 1), cursor)
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
        let completed = mention_under_cursor(&text, cursor)
            .map(|SkillMentionCursor { token, .. }| complete_mention(&text, &token, "review"));
        assert_eq!(completed, expected.map(at_cursor), "{label}");
    }
}
