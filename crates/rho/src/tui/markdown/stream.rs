use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct MarkdownStreamPrefix {
    pub(in crate::tui) byte_index: usize,
    pub(in crate::tui) ends_with_wrap: bool,
}

/// Drainable and previewable ends of a pending markdown buffer.
///
/// `drain` is safe to commit permanently. `preview_end` may extend through the
/// stable open-line prefix so already-drawn prose stays visible while a later
/// inline marker is still incomplete. Preview never commits; only drain does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct MarkdownStreamBounds {
    pub(in crate::tui) drain: MarkdownStreamPrefix,
    pub(in crate::tui) preview_end: Option<usize>,
}

/// `line_start` says whether `text` begins mid-way through a committed list
/// item; its first line then wraps as hung continuation rows.
pub(in crate::tui) fn markdown_stream_bounds(
    text: &str,
    width: usize,
    in_code_block: bool,
    line_start: StreamLineStart,
) -> MarkdownStreamBounds {
    let current_line_start = text.rfind('\n').map_or(0, |index| index + '\n'.len_utf8());
    // Only the first pending line can continue a committed list item.
    let continued_indent = (current_line_start == 0)
        .then(|| line_start.continued_indent(width.max(1)))
        .flatten();
    let current_line_in_code_block =
        line_starts_in_code_block(text, current_line_start, in_code_block);
    let current_line = &text[current_line_start..];
    let mut drain = MarkdownStreamPrefix {
        byte_index: current_line_start,
        ends_with_wrap: false,
    };

    if current_line.is_empty() || starts_with_code_fence_fragment(current_line) {
        return MarkdownStreamBounds {
            drain,
            preview_end: None,
        };
    }

    if current_line_in_code_block {
        // Code-block rows render at the full pane width; keep the streaming
        // wrap boundary in lockstep with `hard_wrap_ranges` / `hard_wrap_styled_spans`.
        let complete = complete_hard_wrap_prefix(current_line, width.max(1));
        if complete.byte_index > 0 {
            drain.byte_index = current_line_start + complete.byte_index;
            drain.ends_with_wrap = complete.ends_with_wrap;
        }
        return MarkdownStreamBounds {
            drain,
            preview_end: previewable_prefix_end(
                current_line_start,
                current_line,
                display_width(current_line),
            ),
        };
    }

    // Only wrap/preview inside the stable prefix. Open markers stay pending so
    // already-drawn prose is not pulled back when the span finally closes shorter.
    let stable_line_len = inline_markdown_stable_prefix_len(current_line);
    let stable_line = &current_line[..stable_line_len];
    // Rendered once and shared: inline rendering now includes txm math spans,
    // so a second pass per call is no longer cheap.
    let rendered_line = markdown_inline_text(stable_line);
    let preview_end = previewable_prefix_end(
        current_line_start,
        stable_line,
        display_width(&rendered_line),
    );

    if continued_indent.is_none()
        && !matches!(
            heading_stream_state(current_line),
            HeadingStreamState::NotHeading
        )
    {
        // Headings drain only once the line completes.
        return MarkdownStreamBounds { drain, preview_end };
    }

    if stable_line_len == 0 {
        return MarkdownStreamBounds {
            drain,
            preview_end: None,
        };
    }

    let indent = continued_indent
        .unwrap_or_else(|| ContinuationIndent::list_item(current_line, width.max(1)));
    let complete = complete_word_wrap_prefix(&rendered_line, width, indent);
    if complete.byte_index == 0 {
        return MarkdownStreamBounds { drain, preview_end };
    }

    if let Some(wrapped) = stable_wrap_drain_prefix(
        stable_line,
        &rendered_line,
        &rendered_line[..complete.byte_index],
        complete,
    ) {
        drain.byte_index = current_line_start + wrapped.byte_index;
        drain.ends_with_wrap = wrapped.ends_with_wrap;
    }

    MarkdownStreamBounds { drain, preview_end }
}

/// Map a rendered soft-wrap cut back onto the stable source prefix.
fn stable_wrap_drain_prefix(
    stable_line: &str,
    rendered_line: &str,
    rendered_prefix: &str,
    complete: CompleteStreamPrefix,
) -> Option<CompleteStreamPrefix> {
    if rendered_prefix.is_empty() {
        return None;
    }
    // Literal line: source and render are the same string, so the wrap cut
    // is already a source index.
    if stable_line == rendered_line {
        let byte_index = rendered_prefix.len();
        if stable_line.is_char_boundary(byte_index)
            && !starts_with_code_fence_fragment(&stable_line[..byte_index])
        {
            return Some(CompleteStreamPrefix {
                byte_index,
                ends_with_wrap: complete.ends_with_wrap,
            });
        }
    }

    // A source cut must render exactly the completed rows. Committing a
    // whole inline span that also contains a partial row would put the next
    // preview on a new row instead of continuing that unfinished row.
    // Find the latest exact match backward so earlier prefixes need not render.
    for candidate in stable_line
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(stable_line.len()))
        .rev()
        .take_while(|&index| index > 0)
    {
        if starts_with_code_fence_fragment(&stable_line[..candidate]) {
            continue;
        }
        let candidate_rendered;
        let candidate_rendered = if candidate == stable_line.len() {
            rendered_line
        } else {
            candidate_rendered = markdown_inline_text(&stable_line[..candidate]);
            &candidate_rendered
        };
        if candidate_rendered == rendered_prefix {
            return Some(CompleteStreamPrefix {
                byte_index: candidate,
                ends_with_wrap: complete.ends_with_wrap,
            });
        }
    }
    None
}

/// Byte end of the live preview when the stable open-line prefix renders non-empty.
///
/// Uses a local width check on the current line only. Prior complete lines are
/// already covered by the drain bound; re-running `markdown_lines` over the full
/// prefix is unnecessary for a non-zero visibility test.
fn previewable_prefix_end(
    current_line_start: usize,
    stable_line: &str,
    rendered_width: usize,
) -> Option<usize> {
    if stable_line.is_empty() {
        return None;
    }
    (rendered_width > 0).then_some(current_line_start + stable_line.len())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CompleteStreamPrefix {
    byte_index: usize,
    ends_with_wrap: bool,
}

/// End of the last soft-wrapped row that later text cannot reflow.
///
/// Rows followed by more text are complete. A trailing row that fills the
/// width is complete only once it ends in whitespace: otherwise it may end
/// mid-word, and the next character would pull that word onto the next row.
fn complete_word_wrap_prefix(
    text: &str,
    width: usize,
    indent: ContinuationIndent,
) -> CompleteStreamPrefix {
    let width = width.max(1);
    wrap_markdown_line_ranges(text, width, indent)
        .into_iter()
        .rfind(|range| {
            let row_width = width - indent.row_indent(range.start);
            range.end < text.len()
                || (text.ends_with(char::is_whitespace)
                    && display_width(&text[range.clone()]) >= row_width)
        })
        .map(|range| CompleteStreamPrefix {
            byte_index: range.end,
            ends_with_wrap: true,
        })
        .unwrap_or_default()
}

fn complete_hard_wrap_prefix(text: &str, width: usize) -> CompleteStreamPrefix {
    let width = width.max(1);
    // Appended text can still extend the last grapheme (and change its width),
    // so even a full final row must remain pending until another cluster arrives.
    let last_complete = hard_wrap_ranges(text, width)
        .into_iter()
        .rfind(|range| range.end < text.len())
        .map_or(0, |range| range.end);

    if last_complete == 0 {
        CompleteStreamPrefix::default()
    } else {
        CompleteStreamPrefix {
            byte_index: last_complete,
            ends_with_wrap: true,
        }
    }
}

fn line_starts_in_code_block(text: &str, line_start: usize, in_code_block: bool) -> bool {
    let mut active_fence = in_code_block.then_some(CodeFence {
        marker: '`',
        length: 3,
    });
    for complete_line in text[..line_start].split_inclusive('\n') {
        let line = complete_line.trim_end_matches('\n');
        if active_fence.is_some_and(|fence| is_closing_fence(line, fence)) {
            active_fence = None;
        } else if active_fence.is_none() {
            active_fence = parse_opening_fence(line);
        }
    }
    active_fence.is_some()
}

fn starts_with_code_fence_fragment(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && (trimmed.starts_with("```") || (trimmed.len() < 3 && "```".starts_with(trimmed)))
}

/// Returns the start of the trailing block that can still change as markdown is appended.
///
/// Markdown is line-oriented except for fenced code blocks, display math, and tables. Keeping
/// the final block mutable lets the history cache promote completed blocks and
/// re-render only this suffix as streaming text arrives.
pub(in crate::tui) fn incremental_markdown_tail_start(text: &str) -> usize {
    let mut line_offsets = Vec::new();
    let mut raw_lines = Vec::new();
    let mut offset = 0;
    for source_line in text.split_inclusive('\n') {
        let raw_line = source_line.strip_suffix('\n').unwrap_or(source_line);
        let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        line_offsets.push(offset);
        raw_lines.push(raw_line);
        offset += source_line.len();
    }
    if raw_lines.is_empty() {
        return 0;
    }

    let mut line_index = 0;
    let mut trailing_block_start = 0;
    while line_index < raw_lines.len() {
        trailing_block_start = line_offsets[line_index];
        if let Some(opening) = parse_opening_fence(raw_lines[line_index]) {
            line_index += 1;
            while line_index < raw_lines.len() {
                let closes_block = is_closing_fence(raw_lines[line_index], opening);
                line_index += 1;
                if closes_block {
                    break;
                }
            }
            continue;
        }
        match math::display_math_span(&raw_lines[line_index..]) {
            Some(math::DisplayMathSpan::Complete { line_count }) => {
                line_index += line_count;
                continue;
            }
            Some(math::DisplayMathSpan::Incomplete) => {
                line_index = raw_lines.len();
                continue;
            }
            None => {}
        }
        if let Some(consumed_lines) = table::markdown_table_line_count(&raw_lines[line_index..]) {
            line_index += consumed_lines;
            continue;
        }
        line_index += 1;
    }
    trailing_block_start
}
