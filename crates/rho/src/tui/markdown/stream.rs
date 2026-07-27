use super::*;

/// How much of `text` can be shown as a live partial-line preview.
///
/// Includes the stable prefix before an open inline marker so completing bold or
/// italic does not blank the already-drawn prose and yank the viewport up.
pub(in crate::tui) fn markdown_preview_prefix_len(
    text: &str,
    width: usize,
    in_code_block: bool,
) -> Option<usize> {
    let current_line_start = text.rfind('\n').map_or(0, |index| index + '\n'.len_utf8());
    let current_line_in_code_block =
        line_starts_in_code_block(text, current_line_start, in_code_block);
    let current_line = &text[current_line_start..];
    if current_line.is_empty() || starts_with_code_fence_fragment(current_line) {
        return None;
    }

    let stable_line_len = if current_line_in_code_block {
        current_line.len()
    } else {
        inline_markdown_stable_prefix_len(current_line)
    };
    if stable_line_len == 0 {
        return None;
    }

    let prefix_len = current_line_start + stable_line_len;
    let mut probe_code_block = in_code_block;
    let rendered_width = markdown_lines(&text[..prefix_len], width, &mut probe_code_block)
        .last()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| display_width(span.content.as_ref()))
                .sum::<usize>()
        })
        .unwrap_or_default();
    (rendered_width > 0).then_some(prefix_len)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct MarkdownStreamPrefix {
    pub(in crate::tui) byte_index: usize,
    pub(in crate::tui) ends_with_wrap: bool,
}

pub(in crate::tui) fn markdown_stream_prefix(
    text: &str,
    width: usize,
    in_code_block: bool,
) -> MarkdownStreamPrefix {
    let current_line_start = text.rfind('\n').map_or(0, |index| index + '\n'.len_utf8());
    let current_line_in_code_block =
        line_starts_in_code_block(text, current_line_start, in_code_block);
    let mut prefix = MarkdownStreamPrefix {
        byte_index: current_line_start,
        ends_with_wrap: false,
    };

    let current_line = &text[current_line_start..];
    if current_line.is_empty() || starts_with_code_fence_fragment(current_line) {
        return prefix;
    }

    if current_line_in_code_block {
        let complete =
            complete_hard_wrap_prefix(current_line, code_block_stream_content_width(width));
        if complete.byte_index > 0 {
            prefix.byte_index = current_line_start + complete.byte_index;
            prefix.ends_with_wrap = complete.ends_with_wrap;
        }
        return prefix;
    }

    if !matches!(
        heading_stream_state(current_line),
        HeadingStreamState::NotHeading
    ) {
        return prefix;
    }

    // Only wrap inside the stable prefix. Open markers stay pending so already
    // drawn prose is not pulled back when the span finally closes shorter.
    let stable_line_len = inline_markdown_stable_prefix_len(current_line);
    if stable_line_len == 0 {
        return prefix;
    }
    let stable_line = &current_line[..stable_line_len];

    let rendered_line = markdown_inline_text(stable_line);
    let complete = complete_word_wrap_prefix(&rendered_line, width);
    if complete.byte_index == 0 {
        return prefix;
    }

    let rendered_prefix = &rendered_line[..complete.byte_index];
    for candidate in stable_line
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(stable_line.len()))
        .skip(1)
    {
        let absolute_candidate = current_line_start + candidate;
        if markdown_safe_prefix_len(text, absolute_candidate, in_code_block) != absolute_candidate {
            continue;
        }
        let candidate_source = &stable_line[..candidate];
        let candidate_rendered = markdown_inline_text(candidate_source);
        if candidate_rendered == rendered_prefix {
            prefix.byte_index = absolute_candidate;
            prefix.ends_with_wrap = complete.ends_with_wrap;
        } else if candidate_source.len() != candidate_rendered.len()
            && !candidate_source
                .chars()
                .last()
                .is_some_and(char::is_whitespace)
            && candidate_rendered.starts_with(rendered_prefix)
        {
            prefix.byte_index = absolute_candidate;
            prefix.ends_with_wrap = false;
        }
    }

    prefix
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CompleteStreamPrefix {
    byte_index: usize,
    ends_with_wrap: bool,
}

fn complete_word_wrap_prefix(text: &str, width: usize) -> CompleteStreamPrefix {
    wrap_line_at_whitespace_ranges(text, width)
        .into_iter()
        .rfind(|range| {
            range.end < text.len() || display_width(&text[range.clone()]) >= width.max(1)
        })
        .map(|range| CompleteStreamPrefix {
            byte_index: range.end,
            ends_with_wrap: true,
        })
        .unwrap_or_default()
}

fn complete_hard_wrap_prefix(text: &str, width: usize) -> CompleteStreamPrefix {
    let width = width.max(1);
    let mut line_width = 0;
    let mut last_complete = 0;
    for (index, ch) in text.char_indices() {
        let ch_width = char_display_width(ch);
        if line_width > 0 && line_width + ch_width > width {
            last_complete = index;
            line_width = 0;
        }
        line_width += ch_width;
        let next = index + ch.len_utf8();
        if line_width >= width {
            last_complete = next;
            line_width = 0;
        }
    }

    if last_complete == 0 {
        CompleteStreamPrefix::default()
    } else {
        CompleteStreamPrefix {
            byte_index: last_complete,
            ends_with_wrap: true,
        }
    }
}

fn markdown_safe_prefix_len(text: &str, candidate_byte_index: usize, in_code_block: bool) -> usize {
    let candidate_byte_index = candidate_byte_index.min(text.len());
    let current_line_start = text[..candidate_byte_index]
        .rfind('\n')
        .map_or(0, |index| index + '\n'.len_utf8());
    // Fence state depends on complete lines before the candidate, so scan the
    // full text rather than only the candidate prefix.
    let current_line_in_code_block =
        line_starts_in_code_block(text, current_line_start, in_code_block);

    let current_line = &text[current_line_start..candidate_byte_index];
    if current_line.is_empty() {
        return candidate_byte_index;
    }
    if starts_with_code_fence_fragment(current_line) {
        return current_line_start;
    }
    if current_line_in_code_block {
        return candidate_byte_index;
    }

    // The candidate is taken from the source line. Compare against the stable
    // prefix of the full current line so open markers after the candidate do
    // not invalidate already-closed prose before them.
    let full_current_line_end = text[current_line_start..]
        .find('\n')
        .map_or(text.len(), |index| current_line_start + index);
    let full_current_line = &text[current_line_start..full_current_line_end];
    let stable_end = current_line_start + inline_markdown_stable_prefix_len(full_current_line);
    if candidate_byte_index <= stable_end {
        candidate_byte_index
    } else {
        stable_end
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

fn code_block_stream_content_width(width: usize) -> usize {
    let width = width.max(1);
    match width {
        1 => 1,
        2 | 3 => width - 1,
        width => width - 4,
    }
}

fn starts_with_code_fence_fragment(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && (trimmed.starts_with("```") || (trimmed.len() < 3 && "```".starts_with(trimmed)))
}
