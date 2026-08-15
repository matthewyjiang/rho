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
    let (mut lines, code_blocks, image_sources, image_rows) = match entry {
        Entry::Assistant(text) => {
            let rendered = render_assistant_content(text, width);
            (
                rendered.lines,
                rendered.code_blocks,
                rendered.image_sources,
                rendered.image_rows,
            )
        }
        Entry::Reasoning(reasoning) => {
            let mut lines = Vec::new();
            let mut code_blocks = Vec::new();
            let mut image_sources = Vec::new();
            let mut image_rows = Vec::new();
            if !reasoning.text.is_empty() {
                let rendered = render_reasoning_content(&reasoning.text, width);
                lines = rendered.lines;
                code_blocks = rendered.code_blocks;
                image_sources = rendered.image_sources;
                image_rows = rendered.image_rows;
            }
            if let Some(elapsed) = reasoning.thought_for {
                push_wrapped_text(
                    &mut lines,
                    &crate::tui::reasoning_phase::thought_summary(elapsed),
                    inner_width,
                    Theme::dim().add_modifier(Modifier::DIM),
                    LineFill::Natural,
                );
            }
            (lines, code_blocks, image_sources, image_rows)
        }
        _ => {
            let mut lines = Vec::new();
            render_non_assistant_entry(&mut lines, entry, inner_width, max_tool_output_lines);
            (lines, Vec::new(), Vec::new(), Vec::new())
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
