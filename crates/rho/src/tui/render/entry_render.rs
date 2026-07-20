use super::*;
use crate::tui::Entry;

pub(in crate::tui) fn entry_lines(
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
) -> Vec<Line<'static>> {
    render_entry(entry, width, max_tool_output_lines).lines
}

pub(in crate::tui) fn render_entry(
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
) -> RenderedEntry {
    let inner_width = padded_inner_width(width);
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
        Entry::Reasoning(text) => {
            let rendered = render_reasoning_content(text, width);
            (
                rendered.lines,
                rendered.code_blocks,
                rendered.image_sources,
                rendered.image_rows,
            )
        }
        _ => {
            let mut lines = Vec::new();
            render_non_assistant_entry(&mut lines, entry, inner_width, max_tool_output_lines);
            (lines, Vec::new(), Vec::new(), Vec::new())
        }
    };

    let image_placement = reserve_entry_image_rows(&mut lines, entry, width);
    let block_style = lines
        .first()
        .and_then(|line| line.spans.first())
        .map(|span| span.style)
        .unwrap_or_default();
    let mut padded = Vec::with_capacity(lines.len() + 2);
    padded.push(styled_blank_line(width, block_style));
    padded.extend(lines.into_iter().map(pad_line));
    padded.push(styled_blank_line(width, block_style));
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
                    .map(|_| image.height_for_width(width).saturating_sub(1))
            })
            .sum::<usize>();
        block.top_line = block.top_line.saturating_add(preceding_image_rows);
    }

    let padded_image_rows = rendered
        .image_rows
        .iter()
        .map(|row| row.saturating_add(1))
        .collect::<Vec<_>>();
    if let Some(placements) =
        reserve_markdown_image_rows(&mut rendered.lines, &padded_image_rows, images, width)
    {
        rendered.image_placement = Some(placements);
    }
}

#[cfg(test)]
pub(in crate::tui) fn render_entry_with_images(
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
    markdown_images: Option<&[(usize, FeedImage)]>,
) -> RenderedEntry {
    let mut rendered = render_entry(entry, width, max_tool_output_lines);
    if let Some(images) = markdown_images {
        apply_markdown_images(&mut rendered, images, width);
    }
    rendered
}
