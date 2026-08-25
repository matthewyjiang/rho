use super::*;
use crate::tui::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum TrailingBlank {
    Include,
    Omit,
}

impl TrailingBlank {
    fn is_included(self) -> bool {
        match self {
            Self::Include => true,
            Self::Omit => false,
        }
    }
}

pub(in crate::tui) fn entry_lines(
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
    max_image_height: u16,
) -> Vec<Line<'static>> {
    render_entry(entry, width, max_tool_output_lines, max_image_height).lines
}

pub(in crate::tui) fn render_entry(
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
    max_image_height: u16,
) -> RenderedEntry {
    render_entry_with_options(
        entry,
        width,
        max_tool_output_lines,
        max_image_height,
        TrailingBlank::Include,
    )
}

/// Render an entry with an optional trailing spacer blank and no leading blank.
///
/// Spacing between transcript blocks comes from each entry's trailing blank.
/// Open stream tails omit that blank so a live continuation can abut the
/// committed content without a mid-message gap.
pub(in crate::tui) fn render_entry_with_options(
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
    max_image_height: u16,
    trailing_blank: TrailingBlank,
) -> RenderedEntry {
    let inner_width = padded_content_width(width);
    let RenderedEntry {
        mut lines,
        code_blocks,
        image_sources,
        image_rows,
        ..
    } = match entry {
        Entry::Assistant(assistant) => render_markdown_entry_with_summary(
            &assistant.text,
            width,
            inner_width,
            render_assistant_content,
            assistant.worked_for.map(crate::tui::goal::worked_summary),
        ),
        Entry::Reasoning(reasoning) => render_markdown_entry_with_summary(
            &reasoning.text,
            width,
            inner_width,
            render_reasoning_content,
            reasoning.thought_for.map(crate::tui::goal::thought_summary),
        ),
        _ => {
            let mut lines = Vec::new();
            render_non_assistant_entry(&mut lines, entry, inner_width, max_tool_output_lines);
            RenderedEntry {
                lines,
                ..RenderedEntry::default()
            }
        }
    };

    let image_placement = reserve_entry_image_rows(&mut lines, entry, width, max_image_height);
    // Trailing spacer separates transcript blocks. User messages keep their
    // background on content rows only so the spacer does not grow an empty
    // highlighted band below the prompt. Strip underline so a lead-in link
    // does not paint a full-width rule under the blank row.
    let spacer_style = match entry {
        crate::tui::Entry::Tool(_) => Theme::tool_card_padding(),
        crate::tui::Entry::User(_) => Style::default(),
        crate::tui::Entry::Assistant(_)
        | crate::tui::Entry::Reasoning(_)
        | crate::tui::Entry::Notice(_)
        | crate::tui::Entry::RuntimeInfo(_)
        | crate::tui::Entry::Changelog(_)
        | crate::tui::Entry::Error(_) => lines
            .first()
            .and_then(|line| line.spans.first())
            .map(|span| chrome_edge_style(span.style))
            .unwrap_or_default(),
    };
    let mut padded = Vec::with_capacity(lines.len() + usize::from(trailing_blank.is_included()));
    padded.extend(lines.into_iter().map(pad_display_line));
    if trailing_blank.is_included() {
        padded.push(styled_blank_line(width, spacer_style));
    }
    RenderedEntry {
        lines: padded,
        code_blocks,
        image_placement,
        image_sources,
        image_rows,
    }
}

fn render_markdown_entry_with_summary(
    text: &str,
    width: usize,
    inner_width: usize,
    render_content: fn(&str, usize) -> RenderedEntry,
    summary: Option<String>,
) -> RenderedEntry {
    let mut rendered = if text.is_empty() {
        RenderedEntry::default()
    } else {
        render_content(text, width)
    };
    if let Some(summary) = summary {
        // Keep the dim receipt off the last content row so short replies
        // do not sit flush against `Worked for` / `Thought for`.
        let summary_style = Theme::dim().add_modifier(Modifier::DIM);
        if rendered.lines.last().is_some_and(line_has_visible_text) {
            push_wrapped_text(
                &mut rendered.lines,
                "",
                inner_width,
                summary_style,
                LineFill::Natural,
            );
        }
        push_wrapped_text(
            &mut rendered.lines,
            &summary,
            inner_width,
            summary_style,
            LineFill::Natural,
        );
    }
    rendered
}

fn line_has_visible_text(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .any(|span| !span.content.chars().all(char::is_whitespace))
}

pub(in crate::tui) fn apply_markdown_images(
    rendered: &mut RenderedEntry,
    images: &[(usize, FeedImage)],
    width: usize,
    max_image_height: u16,
) {
    if images.is_empty() {
        return;
    }

    for block in &mut rendered.code_blocks {
        let original_top_line = block.top_line;
        let preceding_image_rows = images
            .iter()
            .filter_map(|(source_index, image)| {
                rendered
                    .image_rows
                    .get(*source_index)
                    .filter(|&&row| row < original_top_line)
                    .map(|_| {
                        image
                            .height_for_width(width, max_image_height)
                            .saturating_sub(1)
                    })
            })
            .sum::<usize>();
        block.top_line = block.top_line.saturating_add(preceding_image_rows);
    }

    // Content starts at row 0; the trailing spacer sits after content.
    if let Some(placements) = reserve_markdown_image_rows(
        &mut rendered.lines,
        &rendered.image_rows,
        images,
        width,
        max_image_height,
    ) {
        rendered.image_placement = Some(placements);
    }
}
