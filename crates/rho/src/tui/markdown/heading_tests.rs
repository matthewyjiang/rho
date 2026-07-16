use super::*;
use crate::tui::{markdown::markdown_lines, theme::Theme};
use pretty_assertions::assert_eq;
use ratatui::{
    style::{Color, Modifier},
    text::Line,
};

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
fn renders_levels_with_distinct_hierarchical_styles_and_without_markers() {
    let source = "# one\n## two\n### three\n#### four\n##### five\n###### six";
    let levels = [
        HeadingLevel::H1,
        HeadingLevel::H2,
        HeadingLevel::H3,
        HeadingLevel::H4,
        HeadingLevel::H5,
        HeadingLevel::H6,
    ];
    let mut in_code_block = false;

    let lines = markdown_lines(source, 80, &mut in_code_block);

    assert_eq!(
        lines.iter().map(line_text).collect::<Vec<_>>(),
        ["one", "two", "three", "four", "five", "six"]
    );
    assert_eq!(
        lines
            .iter()
            .map(|line| line.spans[0].style.fg)
            .collect::<Vec<_>>(),
        [
            Some(Color::Magenta),
            Some(Color::Blue),
            Some(Color::Cyan),
            Some(Color::Green),
            Some(Color::Yellow),
            Some(Color::Gray),
        ]
    );
    assert_eq!(
        lines
            .iter()
            .zip(levels)
            .map(|(line, level)| (line.spans.len(), line.spans[0].style, level))
            .collect::<Vec<_>>(),
        levels
            .into_iter()
            .map(|level| (1, Theme::markdown_heading(level), level))
            .collect::<Vec<_>>()
    );
    for level in [HeadingLevel::H1, HeadingLevel::H2, HeadingLevel::H3] {
        assert!(Theme::markdown_heading(level).has_modifier(Modifier::BOLD));
    }
    for level in [HeadingLevel::H4, HeadingLevel::H5, HeadingLevel::H6] {
        assert!(!Theme::markdown_heading(level).has_modifier(Modifier::BOLD));
    }
}

#[test]
fn composes_heading_color_with_inline_markdown_styles() {
    let mut in_code_block = false;
    let lines = markdown_lines(
        "#### **bold** and *italic* with `code` and [docs](https://example.com)",
        120,
        &mut in_code_block,
    );
    let heading = Theme::markdown_heading(HeadingLevel::H4);
    let styles = lines[0]
        .spans
        .iter()
        .map(|span| (span.content.as_ref(), span.style))
        .collect::<Vec<_>>();

    assert!(styles.contains(&("bold", heading.patch(Theme::markdown_bold()))));
    assert!(styles.contains(&("italic", heading.patch(Theme::markdown_italic()))));
    assert!(styles.contains(&("code", heading.patch(Theme::markdown_inline_code()))));
    assert!(styles.contains(&("https://example.com", heading.patch(Theme::markdown_link()))));
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
