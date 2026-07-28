use super::*;
use crate::tui::{markdown::markdown_lines, theme::Theme};
use pretty_assertions::assert_eq;
use ratatui::text::Line;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn parses_all_atx_heading_levels() {
    let cases = [
        ("# one", HeadingLevel::H1, "one"),
        ("## two", HeadingLevel::H2, "two"),
        ("### three", HeadingLevel::H3, "three"),
        ("#### four", HeadingLevel::H4, "four"),
        ("##### five", HeadingLevel::H5, "five"),
        ("###### six", HeadingLevel::H6, "six"),
    ];

    for (source, level, content) in cases {
        assert_eq!(
            parse_atx_heading(source),
            Some(AtxHeading { level, content })
        );
    }
}

#[test]
fn accepts_common_atx_spacing_and_closing_hashes() {
    let cases = [
        ("   ## heading", HeadingLevel::H2, "heading"),
        ("#\theading", HeadingLevel::H1, "heading"),
        ("### heading ###", HeadingLevel::H3, "heading"),
        ("### heading ###   ", HeadingLevel::H3, "heading"),
        ("### ###", HeadingLevel::H3, ""),
        ("######", HeadingLevel::H6, ""),
        ("## heading###", HeadingLevel::H2, "heading###"),
    ];

    for (source, level, content) in cases {
        assert_eq!(
            parse_atx_heading(source),
            Some(AtxHeading { level, content })
        );
    }
}

#[test]
fn rejects_lines_that_are_not_atx_headings() {
    for source in [
        "#hashtag",
        "####### heading",
        "    # indented",
        "text # heading",
        "\t# heading",
    ] {
        assert_eq!(parse_atx_heading(source), None, "source: {source:?}");
    }
}

#[test]
fn classifies_streaming_heading_prefixes_without_committing_early() {
    for source in ["", " ", "   ", "#", "   ###"] {
        assert_eq!(
            heading_stream_state(source),
            HeadingStreamState::Potential,
            "source: {source:?}"
        );
    }
    for source in ["# ", "## heading", "   ####\theading"] {
        assert_eq!(
            heading_stream_state(source),
            HeadingStreamState::Heading,
            "source: {source:?}"
        );
    }
    for source in ["#hashtag", "####### ", "    # heading", "ordinary"] {
        assert_eq!(
            heading_stream_state(source),
            HeadingStreamState::NotHeading,
            "source: {source:?}"
        );
    }
}

#[test]
fn preserves_heading_style_across_unicode_wrapping() {
    let content = "你🙂".repeat(20);
    let mut in_code_block = false;
    let lines = markdown_lines(&format!("### {content}"), 7, &mut in_code_block);

    assert_eq!(lines.iter().map(line_text).collect::<String>(), content);
    assert!(lines
        .iter()
        .flat_map(|line| &line.spans)
        .all(|span| { span.style == Theme::markdown_heading(HeadingLevel::H3) }));
}

#[test]
fn leaves_heading_like_text_literal_when_invalid_or_inside_code() {
    let mut in_code_block = false;
    let lines = markdown_lines(
        "#hashtag\n####### nope\n    # indented\n```md\n# literal\n```",
        80,
        &mut in_code_block,
    );
    let text = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(&text[..3], ["#hashtag", "####### nope", "    # indented"]);
    assert!(text.iter().any(|line| line.contains("# literal")));
}
