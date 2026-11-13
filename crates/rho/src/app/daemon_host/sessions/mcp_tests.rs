use pretty_assertions::assert_eq;

use super::{cut_text, UTF8_CHAR_BYTES};

// Covers: a text body cut for a client must leave that client's own cut
// unchanged: the bytes it keeps, and whether it finds the text was longer.
// A cut to exactly the client's budget would read as text that fit.
// Owner: daemon host, MCP resource reads.
#[test]
fn a_cut_text_body_truncates_as_the_whole_text_does() {
    for (case, text, max) in [
        ("fits", "abc".to_owned(), 8),
        ("exactly the budget", "abcdefgh".to_owned(), 8),
        ("one byte over", "abcdefghi".to_owned(), 8),
        ("far over", "a".repeat(64), 8),
        ("wide characters over the cut", "\u{1F600}".repeat(16), 9),
        ("no budget at all", "abcdefgh".to_owned(), 0),
    ] {
        let whole = rho_tools::tool::truncate(text.clone(), max);
        let cut = cut_text(text, max);
        assert!(
            cut.len() <= max + UTF8_CHAR_BYTES,
            "{case}: kept {} bytes",
            cut.len()
        );
        assert_eq!(rho_tools::tool::truncate(cut, max), whole, "{case}");
    }
}
