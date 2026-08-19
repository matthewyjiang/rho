use pretty_assertions::assert_eq;

use super::{last_assistant_text, Entry};

// Covers: /copy must take the latest non-empty assistant text, not user, tool,
// reasoning, or an earlier reply.
// Owner: pure unit
#[test]
fn last_assistant_text_selects_latest_nonempty_assistant() {
    let cases: &[(&str, Vec<Entry>, Option<&str>)] = &[
        ("empty history", Vec::new(), None),
        ("user only", vec![Entry::User("hello".into())], None),
        (
            "whitespace assistant",
            vec![Entry::Assistant("  \n".into())],
            None,
        ),
        (
            "single assistant",
            vec![
                Entry::User("hello".into()),
                Entry::Assistant("reply one".into()),
            ],
            Some("reply one"),
        ),
        (
            "latest assistant after later user",
            vec![
                Entry::Assistant("reply one".into()),
                Entry::User("thanks".into()),
            ],
            Some("reply one"),
        ),
        (
            "skips reasoning notice and empty assistant",
            vec![
                Entry::Assistant("keep this".into()),
                Entry::Reasoning("thinking".into()),
                Entry::Notice("note".into()),
                Entry::Assistant(" \t".into()),
                Entry::Error("boom".into()),
            ],
            Some("keep this"),
        ),
        (
            "two assistants",
            vec![
                Entry::Assistant("first".into()),
                Entry::Assistant("second\n".into()),
            ],
            Some("second\n"),
        ),
    ];

    for (name, entries, expected) in cases {
        assert_eq!(last_assistant_text(entries), *expected, "{name}");
    }
}
