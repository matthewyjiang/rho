use super::*;
use pretty_assertions::assert_eq;

// Pure parser coverage: delimiter parity, escaped closers, and syntax-owned
// contents cannot be distinguished by the PTY's plain-text assertion.
#[test]
fn escape_aware_inline_spans() {
    // Expected styles come from the global theme; hold it steady.
    let _guard = crate::tui::theme::theme_test_lock();
    let plain = Theme::text();
    let italic = Theme::markdown_italic();
    let bold = Theme::markdown_bold();
    let code = Theme::markdown_inline_code();
    let link = Theme::markdown_link();
    for (source, expected) in [
        (r"\_literal\_", vec![("_literal_", plain)]),
        (r"\\*styled*", vec![(r"\", plain), ("styled", italic)]),
        (r"\\\*literal\*", vec![(r"\*literal*", plain)]),
        (r"*a\*b*", vec![("a*b", italic)]),
        (r"*bold\**", vec![("bold*", italic)]),
        (r"**bold\***", vec![("bold*", bold)]),
        (r"\***bold**", vec![("*", plain), ("bold", bold)]),
        (r"**a\*\*b**", vec![("a**b", bold)]),
        (r"\`literal\`", vec![("`literal`", plain)]),
        (r"`\*code\*`", vec![(r"\*code\*", code)]),
        (r"`code\` tail", vec![(r"code\", code), (" tail", plain)]),
        (r"\$x\$", vec![("$x$", plain)]),
        (r"$\frac{1}{2}$", vec![(r"$\frac{1}{2}$", plain)]),
        (r"\*\*literal\*\*", vec![("**literal**", plain)]),
        (r"\\**styled**", vec![(r"\", plain), ("styled", bold)]),
        (r"\[label](target)", vec![("[label](target)", plain)]),
        (
            r"[a\]b](c\)d)",
            vec![("a]b", plain), (": ", plain), ("c)d", link)],
        ),
        (r"![a\]b](c\)d)", vec![("a]b", plain)]),
        (
            r"\![label](target)",
            vec![
                ("!", plain),
                ("label", plain),
                (": ", plain),
                ("target", link),
            ],
        ),
        (r"\a \é \! \\", vec![("\\a \\é ! \\", plain)]),
    ] {
        let actual: Vec<_> = markdown_inline_segments(source)
            .into_iter()
            .map(|segment| (segment.text, segment.style))
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(text, style)| (text.to_owned(), style))
            .collect();
        assert_eq!(actual, expected, "source: {source}");
    }
}

#[test]
fn escaped_openers_do_not_hold_streamed_prose() {
    for (source, stable) in [
        (r"text \*open", r"text \*open"),
        (r"text \_open", r"text \_open"),
        (r"text \`open", r"text \`open"),
        (r"text \[open", r"text \[open"),
        (r"text \$open", r"text \$open"),
        (r"text \\*open", r"text \\"),
        (r"text *a\*b", "text "),
        (r"*bold\**", r"*bold\**"),
        (r"**bold\***", r"**bold\***"),
        (r"\***bold**", r"\***bold**"),
        (r"\***bold", r"\*"),
        (r"text `code\` tail", r"text `code\` tail"),
        (r"text [a](b\)c", "text "),
        (r"text \", "text "),
        (r"text \\", r"text \\"),
    ] {
        assert_eq!(
            &source[..inline_markdown_stable_prefix_len(source)],
            stable,
            "source: {source}"
        );
    }
}
