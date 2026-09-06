use pretty_assertions::assert_eq;

use super::{shell_word_at_cursor, FileMention, PathTokenSource};

// Covers: quoting and escaping must preserve token boundaries and replacement
// coordinates, including a cursor inside a token with a quoted tail.
// Owner: shell word parser.
#[test]
fn quoted_word_boundaries_and_decoding() {
    for (input, cursor, start, end, query) in [
        ("cat 'my dir/'", 13, 4, 13, "my dir/"),
        ("cat 'my dir/'child", 18, 4, 18, "my dir/child"),
        ("cat 'it'\\''s dir/'", 18, 4, 18, "it's dir/"),
        ("cat my\\ dir/", 12, 4, 12, "my dir/"),
        ("cat \"my dir/\" tail", 9, 4, 13, "my d"),
        ("cat \"my\\dir/\"", 13, 4, 13, "my\\dir/"),
        ("cat 'é dir/'", 12, 4, 12, "é dir/"),
        ("cat ", 4, 4, 4, ""),
    ] {
        assert_eq!(
            shell_word_at_cursor(input, cursor),
            FileMention {
                start,
                end,
                query: query.into(),
                source: PathTokenSource::ShellWord
            },
            "{input:?} at {cursor}"
        );
    }
}
