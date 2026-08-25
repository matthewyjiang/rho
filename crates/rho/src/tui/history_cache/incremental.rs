//! Incremental last-entry cache: keep completed fence/table rows and re-paint
//! only the still-open tail as streamed markdown grows.

use std::sync::Arc;

use ratatui::text::Line;

use super::{
    append_entry_segment_into, incremental_entry_source, CachedCodeBlock, CachedEntry,
    EntryContentRender,
};
use crate::tui::{
    markdown::{
        code_block_copy_columns, incremental_markdown_tail_start, is_closing_fence,
        opening_fence_info_token, parse_opening_fence, render_markdown, render_streaming_table,
        render_streaming_table_data_row, streaming_table, streaming_table_bottom_border,
        update_code_block_state, CodeFence, CodeFenceState, StreamingTable,
    },
    render::{pad_display_line, padded_content_width},
    syntax::BlockHighlighter,
    theme::Theme,
    Entry,
};

/// Painted prefix of a last streamed assistant/reasoning entry that can grow.
#[derive(Clone)]
pub(in crate::tui) struct IncrementalEntryCache {
    pub(in crate::tui) stable_source_len: usize,
    pub(in crate::tui) stable_line_count: usize,
    tail: IncrementalTail,
}

impl std::fmt::Debug for IncrementalEntryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncrementalEntryCache")
            .field("stable_source_len", &self.stable_source_len)
            .field("stable_line_count", &self.stable_line_count)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default)]
enum IncrementalTail {
    #[default]
    None,
    Fence(OpenFenceTail),
    Table(OpenTableTail),
}

#[derive(Clone)]
struct OpenFenceTail {
    source_start: usize,
    committed_end: usize,
    committed_line_count: usize,
    fence: CodeFence,
    language: Option<String>,
    highlighter: Option<BlockHighlighter>,
}

#[derive(Clone)]
struct OpenTableTail {
    source_start: usize,
    committed_end: usize,
    committed_line_count: usize,
    committed_data_rows: usize,
    column_widths: Vec<usize>,
}

pub(super) fn incremental_cache_for(
    entry: &Entry,
    is_last: bool,
    width: usize,
) -> Option<IncrementalEntryCache> {
    // Only the last entry can be appended to, so only its cache is ever read
    // (see `entry_appended`). Building one for every entry would re-render
    // each streamed message's stable prefix a second time.
    if !is_last {
        return None;
    }
    let (text, render) = incremental_entry_source(entry)?;
    let stable_source_len = incremental_markdown_tail_start(text);
    let stable_line_count = if stable_source_len == 0 {
        0
    } else {
        render(&text[..stable_source_len], width).lines.len()
    };
    let tail = inspect_open_tail(text, stable_source_len, stable_line_count, width, render);
    Some(IncrementalEntryCache {
        stable_source_len,
        stable_line_count,
        tail,
    })
}

/// Continue an open fence or table without re-rendering settled rows.
///
/// Returns `false` when the caller should fall back to the block-promotion path
/// (closed mermaid, reflow, or a tail that is still ordinary prose).
pub(super) fn extend_open_tail(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    is_reasoning: bool,
) -> bool {
    let Some(cache) = entry.incremental.clone() else {
        return false;
    };
    match cache.tail {
        IncrementalTail::Fence(tail) => extend_open_fence(
            entry,
            text,
            render,
            width,
            has_trailing_blank,
            is_reasoning,
            tail,
        ),
        IncrementalTail::Table(tail) => {
            extend_open_table(entry, text, render, width, has_trailing_blank, tail)
        }
        IncrementalTail::None => false,
    }
}

fn inspect_open_tail(
    text: &str,
    tail_start: usize,
    stable_line_count: usize,
    width: usize,
    render: EntryContentRender,
) -> IncrementalTail {
    let tail = &text[tail_start..];
    if tail.is_empty() {
        return IncrementalTail::None;
    }
    if let Some(fence) = open_fence_from_source(text, tail_start, stable_line_count, width, render)
    {
        return IncrementalTail::Fence(fence);
    }
    if let Some(table) = open_table_from_source(text, tail_start, stable_line_count, width) {
        return IncrementalTail::Table(table);
    }
    IncrementalTail::None
}

fn open_fence_from_source(
    text: &str,
    tail_start: usize,
    stable_line_count: usize,
    width: usize,
    render: EntryContentRender,
) -> Option<OpenFenceTail> {
    let first = text[tail_start..].lines().next()?;
    let fence = parse_opening_fence(first)?;
    if fence_closed(text, tail_start, fence) {
        return None;
    }
    let language = opening_fence_info_token(first);
    let committed_end = last_complete_end(text, tail_start);
    let mut highlighter = None;
    if committed_end > tail_start {
        let mut state = CodeFenceState::default();
        update_code_block_state(&text[tail_start..committed_end], &mut state);
        highlighter = state.highlighter;
    }
    let committed_line_count = if committed_end > tail_start {
        stable_line_count
            .saturating_add(render(&text[tail_start..committed_end], width).lines.len())
    } else {
        stable_line_count
    };
    Some(OpenFenceTail {
        source_start: tail_start,
        committed_end,
        committed_line_count,
        fence,
        language,
        highlighter,
    })
}

fn open_table_from_source(
    text: &str,
    tail_start: usize,
    stable_line_count: usize,
    width: usize,
) -> Option<OpenTableTail> {
    let inner = padded_content_width(width);
    let snapshot = streaming_table(&text[tail_start..], inner)?;
    let complete_end =
        last_complete_end(text, tail_start).min(tail_start + snapshot.consumed_source_len);
    let complete_snapshot = if complete_end > tail_start {
        streaming_table(&text[tail_start..complete_end], inner).unwrap_or(snapshot.clone())
    } else {
        snapshot.clone()
    };
    Some(OpenTableTail {
        source_start: tail_start,
        committed_end: complete_end,
        committed_line_count: stable_line_count
            .saturating_add(complete_snapshot.header_visual_len)
            .saturating_add(complete_data_visual_len(
                &text[tail_start..complete_end],
                inner,
                &complete_snapshot,
            )),
        committed_data_rows: complete_snapshot.data_row_count,
        column_widths: complete_snapshot.column_widths,
    })
}

fn complete_data_visual_len(source: &str, width: usize, snapshot: &StreamingTable) -> usize {
    if snapshot.data_row_count == 0 {
        return 0;
    }
    render_streaming_table(source, width, &snapshot.column_widths)
        .map(|lines| {
            lines
                .len()
                .saturating_sub(snapshot.header_visual_len)
                .saturating_sub(1)
        })
        .unwrap_or(0)
}

fn extend_open_fence(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    is_reasoning: bool,
    mut tail: OpenFenceTail,
) -> bool {
    if tail.source_start > text.len() {
        return false;
    }
    let Some(first) = text[tail.source_start..].lines().next() else {
        return false;
    };
    if parse_opening_fence(first) != Some(tail.fence) {
        return false;
    }
    if is_mermaid_language(tail.language.as_deref())
        && fence_closed(text, tail.source_start, tail.fence)
    {
        return false;
    }
    if tail.committed_end > text.len() {
        return false;
    }

    let trailing_blank = take_trailing_blank(&mut entry.lines, has_trailing_blank);
    entry.lines.truncate(tail.committed_line_count);
    entry
        .code_blocks
        .retain(|block| block.line < tail.committed_line_count);

    let new_complete = last_complete_end(text, tail.source_start);
    if new_complete < tail.committed_end {
        return false;
    }
    if new_complete > tail.committed_end {
        let continue_open = tail.committed_end > tail.source_start;
        if !append_fence_chunk(
            entry,
            &text[tail.committed_end..new_complete],
            render,
            width,
            &mut tail,
            continue_open,
            is_reasoning,
        ) {
            return false;
        }
        tail.committed_end = new_complete;
        tail.committed_line_count = entry.lines.len();
    }

    let closed = fence_closed(text, tail.source_start, tail.fence);
    if !closed && text.len() > tail.committed_end {
        let continue_open = tail.committed_end > tail.source_start;
        let mut remainder_tail = tail.clone();
        if !append_fence_chunk(
            entry,
            &text[tail.committed_end..],
            render,
            width,
            &mut remainder_tail,
            continue_open,
            is_reasoning,
        ) {
            return false;
        }
    }

    refresh_fence_copy(entry, text, &tail, width);
    if let Some(blank) = trailing_blank {
        entry.lines.push(blank);
    }

    let stable_source_len = tail.source_start;
    let stable_line_count = {
        // Stable prefix is everything before the fence.
        entry
            .incremental
            .as_ref()
            .map(|cache| cache.stable_line_count)
            .unwrap_or(0)
    };
    entry.incremental = Some(IncrementalEntryCache {
        stable_source_len,
        stable_line_count,
        tail: if closed {
            IncrementalTail::None
        } else {
            IncrementalTail::Fence(tail)
        },
    });
    true
}

fn append_fence_chunk(
    entry: &mut CachedEntry,
    chunk: &str,
    render: EntryContentRender,
    width: usize,
    tail: &mut OpenFenceTail,
    continue_open: bool,
    is_reasoning: bool,
) -> bool {
    if chunk.is_empty() {
        return true;
    }
    if !continue_open {
        append_entry_segment_into(
            &mut entry.lines,
            &mut entry.code_blocks,
            chunk,
            width,
            render,
        );
        let mut state = CodeFenceState::default();
        update_code_block_state(chunk, &mut state);
        tail.highlighter = state.highlighter;
        return true;
    }

    let inner = padded_content_width(width);
    let mut state = CodeFenceState {
        active: Some(tail.fence),
        language: tail.language.clone(),
        highlighter: tail.highlighter.clone(),
    };
    let rendered = render_markdown(chunk, inner, &mut state);
    let mut lines = rendered.lines;
    if is_reasoning {
        Theme::reasoning_output(&mut lines);
    }
    entry.lines.extend(lines.into_iter().map(pad_display_line));
    tail.highlighter = state.highlighter;
    true
}

fn refresh_fence_copy(entry: &mut CachedEntry, text: &str, tail: &OpenFenceTail, width: usize) {
    let Some(body) = fence_body_source(text, tail.source_start, tail.fence) else {
        return;
    };
    let inner = padded_content_width(width);
    let copy_columns = code_block_copy_columns(inner);
    let header_line = tail
        .committed_line_count
        .saturating_sub(entry.lines.len().saturating_sub(tail.committed_line_count))
        .min(tail.committed_line_count);
    // Opening fence header sits at the first fence line.
    let header_line = entry
        .incremental
        .as_ref()
        .map(|cache| cache.stable_line_count)
        .unwrap_or(header_line);
    if let Some(block) = entry
        .code_blocks
        .iter_mut()
        .find(|block| block.line == header_line)
    {
        block.text = Arc::from(body);
        return;
    }
    let Some(copy_columns) = copy_columns else {
        return;
    };
    entry.code_blocks.push(CachedCodeBlock {
        line: header_line,
        copy_columns: copy_columns.start.saturating_add(1)..copy_columns.end.saturating_add(1),
        text: Arc::from(body),
    });
}

fn fence_body_source(text: &str, source_start: usize, fence: CodeFence) -> Option<String> {
    let rest = &text[source_start..];
    let first_nl = rest.find('\n')?;
    let mut body = &rest[first_nl + 1..];
    if let Some(stripped) = body.strip_suffix('\n') {
        if is_closing_fence(stripped, fence) {
            body = "";
        } else if let Some(last_nl) = stripped.rfind('\n') {
            if is_closing_fence(&stripped[last_nl + 1..], fence) {
                body = &stripped[..last_nl];
            } else {
                body = stripped;
            }
        } else {
            body = stripped;
        }
    } else if is_closing_fence(body, fence) {
        body = "";
    }
    Some(body.to_string())
}

fn extend_open_table(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    mut tail: OpenTableTail,
) -> bool {
    if tail.source_start > text.len() {
        return false;
    }
    let inner = padded_content_width(width);
    let Some(snapshot) = streaming_table(&text[tail.source_start..], inner) else {
        return false;
    };
    if snapshot.column_widths != tail.column_widths {
        return false;
    }

    let trailing_blank = take_trailing_blank(&mut entry.lines, has_trailing_blank);
    entry.lines.truncate(tail.committed_line_count);
    entry
        .code_blocks
        .retain(|block| block.line < tail.committed_line_count);

    let complete_end = last_complete_end(text, tail.source_start)
        .min(tail.source_start + snapshot.consumed_source_len);
    let complete_snapshot =
        streaming_table(&text[tail.source_start..complete_end], inner).unwrap_or(snapshot.clone());
    if complete_snapshot.column_widths != tail.column_widths {
        return false;
    }

    for row_index in tail.committed_data_rows..complete_snapshot.data_row_count {
        let Some(row_lines) = render_streaming_table_data_row(
            &text[tail.source_start..complete_end],
            inner,
            &tail.column_widths,
            row_index,
        ) else {
            return false;
        };
        entry
            .lines
            .extend(row_lines.into_iter().map(pad_display_line));
    }
    tail.committed_data_rows = complete_snapshot.data_row_count;
    tail.committed_end = complete_end;
    tail.committed_line_count = entry.lines.len();

    let table_ended = snapshot.consumed_source_len < text.len().saturating_sub(tail.source_start)
        && last_complete_end(text, tail.source_start)
            > tail.source_start + snapshot.consumed_source_len;
    if !table_ended
        && text.len() > tail.committed_end
        && complete_snapshot.data_row_count < snapshot.data_row_count
    {
        if let Some(row_lines) = render_streaming_table_data_row(
            &text[tail.source_start..],
            inner,
            &tail.column_widths,
            complete_snapshot.data_row_count,
        ) {
            entry
                .lines
                .extend(row_lines.into_iter().map(pad_display_line));
        }
    }
    entry
        .lines
        .push(pad_display_line(streaming_table_bottom_border(
            &tail.column_widths,
        )));

    if table_ended {
        let after = tail.source_start + snapshot.consumed_source_len;
        append_entry_segment_into(
            &mut entry.lines,
            &mut entry.code_blocks,
            &text[after..],
            width,
            render,
        );
    }

    if let Some(blank) = trailing_blank {
        entry.lines.push(blank);
    }

    let stable_line_count = entry
        .incremental
        .as_ref()
        .map(|cache| cache.stable_line_count)
        .unwrap_or(0);
    entry.incremental = Some(IncrementalEntryCache {
        stable_source_len: tail.source_start,
        stable_line_count,
        tail: if table_ended {
            IncrementalTail::None
        } else {
            IncrementalTail::Table(tail)
        },
    });
    true
}

fn take_trailing_blank(
    lines: &mut [Line<'static>],
    has_trailing_blank: bool,
) -> Option<Line<'static>> {
    if !has_trailing_blank {
        return None;
    }
    lines.last().cloned()
}

fn last_complete_end(text: &str, from: usize) -> usize {
    text[from..]
        .rfind('\n')
        .map_or(from, |index| from + index + 1)
}

fn fence_closed(text: &str, source_start: usize, fence: CodeFence) -> bool {
    text[source_start..]
        .lines()
        .skip(1)
        .any(|line| is_closing_fence(line, fence))
}

fn is_mermaid_language(language: Option<&str>) -> bool {
    language == Some("mermaid")
}
