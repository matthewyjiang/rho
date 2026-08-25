//! Incremental last-entry cache: keep completed fence/table rows and paint
//! only the still-open tail. Close, leftover prose, and table reflow fall
//! back to block promotion so this path never owns remainder text.

use std::sync::Arc;

use ratatui::text::Line;

use super::{
    append_entry_segment_into, incremental_entry_source, CachedCodeBlock, CachedEntry,
    EntryContentRender,
};
use crate::tui::{
    markdown::{
        code_block_copy_columns, incremental_markdown_tail_start, is_closing_fence,
        opening_fence_info_token, parse_opening_fence, render_markdown, streaming_table,
        streaming_table_bottom_border, update_code_block_state, CodeFence, CodeFenceState,
        StreamingTable,
    },
    render::{pad_display_line, padded_content_width},
    syntax::BlockHighlighter,
    theme::Theme,
    Entry,
};

/// Painted prefix of a last streamed assistant/reasoning entry that can grow.
#[derive(Clone)]
pub(in crate::tui) struct IncrementalEntryCache {
    pub(super) stable_source_len: usize,
    pub(super) stable_line_count: usize,
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
    header_line: usize,
    fence: CodeFence,
    language: Option<String>,
    highlighter: Option<BlockHighlighter>,
}

#[derive(Clone)]
struct OpenTableTail {
    source_start: usize,
    committed_end: usize,
    committed_line_count: usize,
    table: StreamingTable,
}

pub(super) fn incremental_cache_for(
    entry: &Entry,
    is_last: bool,
    width: usize,
    content_line_count: Option<usize>,
) -> Option<IncrementalEntryCache> {
    // Only the last entry can be appended to, so only its cache is ever read
    // (see `entry_appended`). Building one for every entry would re-render
    // each streamed message's stable prefix a second time.
    if !is_last {
        return None;
    }
    let (text, render) = incremental_entry_source(entry)?;
    Some(inspect_incremental(text, render, width, content_line_count))
}

/// Continue an open fence or table, or promote newly completed blocks.
///
/// Returns `false` when the caller should rebuild the entry.
pub(super) fn extend_last_entry(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    content_end: usize,
    reasoning: bool,
) -> bool {
    if extend_open_tail(entry, text, render, width, has_trailing_blank, reasoning) {
        return true;
    }
    promote_stable_tail(entry, text, render, width, has_trailing_blank, content_end)
}

fn inspect_incremental(
    text: &str,
    render: EntryContentRender,
    width: usize,
    content_line_count: Option<usize>,
) -> IncrementalEntryCache {
    let stable_source_len = incremental_markdown_tail_start(text);
    let stable_line_count = if stable_source_len == 0 {
        0
    } else {
        render(&text[..stable_source_len], width).lines.len()
    };
    let tail = inspect_open_tail(
        text,
        stable_source_len,
        stable_line_count,
        width,
        content_line_count,
        render,
    );
    IncrementalEntryCache {
        stable_source_len,
        stable_line_count,
        tail,
    }
}

fn inspect_open_tail(
    text: &str,
    tail_start: usize,
    stable_line_count: usize,
    width: usize,
    content_line_count: Option<usize>,
    render: EntryContentRender,
) -> IncrementalTail {
    let tail = &text[tail_start..];
    if tail.is_empty() {
        return IncrementalTail::None;
    }
    if let Some(fence) = open_fence_from_source(
        text,
        tail_start,
        stable_line_count,
        width,
        content_line_count,
        render,
    ) {
        return IncrementalTail::Fence(fence);
    }
    if let Some(table) = open_table_from_source(text, tail_start, width, content_line_count) {
        return IncrementalTail::Table(table);
    }
    IncrementalTail::None
}

fn open_fence_from_source(
    text: &str,
    tail_start: usize,
    stable_line_count: usize,
    width: usize,
    content_line_count: Option<usize>,
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
        highlighter = state.take_highlighter();
    }
    let committed_line_count = committed_visual_lines(
        content_line_count,
        text,
        committed_end,
        || {
            if committed_end > tail_start {
                stable_line_count
                    .saturating_add(render(&text[tail_start..committed_end], width).lines.len())
            } else {
                stable_line_count
            }
        },
        |incomplete| {
            fence_preview_line_count(
                incomplete,
                fence,
                language.clone(),
                highlighter.clone(),
                width,
            )
        },
    )?;
    Some(OpenFenceTail {
        source_start: tail_start,
        committed_end,
        committed_line_count,
        header_line: stable_line_count,
        fence,
        language,
        highlighter,
    })
}

fn open_table_from_source(
    text: &str,
    tail_start: usize,
    width: usize,
    content_line_count: Option<usize>,
) -> Option<OpenTableTail> {
    let inner = padded_content_width(width);
    let complete_end = last_complete_end(text, tail_start);
    if complete_end <= tail_start {
        return None;
    }
    let table = streaming_table(&text[tail_start..complete_end], inner)?;
    if table.consumed_source_len() < complete_end.saturating_sub(tail_start) {
        return None;
    }
    let incomplete = &text[complete_end..];
    let preview_lines = if incomplete.is_empty() {
        0
    } else {
        table.paint_data_row(incomplete)?.len()
    };
    let committed_line_count = match content_line_count {
        Some(content) => content.saturating_sub(preview_lines).saturating_sub(1),
        None => return None,
    };
    Some(OpenTableTail {
        source_start: tail_start,
        committed_end: complete_end,
        committed_line_count,
        table,
    })
}

fn committed_visual_lines(
    content_line_count: Option<usize>,
    text: &str,
    committed_end: usize,
    render_committed: impl FnOnce() -> usize,
    preview_len: impl FnOnce(&str) -> Option<usize>,
) -> Option<usize> {
    match content_line_count {
        Some(content) if committed_end == text.len() => Some(content),
        Some(content) => Some(content.saturating_sub(preview_len(&text[committed_end..])?)),
        None => Some(render_committed()),
    }
}

fn fence_preview_line_count(
    incomplete: &str,
    fence: CodeFence,
    language: Option<String>,
    highlighter: Option<BlockHighlighter>,
    width: usize,
) -> Option<usize> {
    if incomplete.is_empty() {
        return Some(0);
    }
    let mut state = CodeFenceState::continue_open(fence, language, highlighter);
    Some(
        render_markdown(incomplete, padded_content_width(width), &mut state)
            .lines
            .len(),
    )
}

fn extend_open_tail(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    reasoning: bool,
) -> bool {
    let Some(mut cache) = entry.incremental.take() else {
        return false;
    };
    let tail = std::mem::take(&mut cache.tail);
    let next_tail = match tail {
        IncrementalTail::Fence(tail) => extend_open_fence(
            entry,
            text,
            render,
            width,
            has_trailing_blank,
            reasoning,
            tail,
        )
        .map(IncrementalTail::Fence),
        IncrementalTail::Table(tail) => {
            extend_open_table(entry, text, has_trailing_blank, tail).map(IncrementalTail::Table)
        }
        IncrementalTail::None => None,
    };
    match next_tail {
        Some(tail) => {
            cache.tail = tail;
            entry.incremental = Some(cache);
            true
        }
        None => {
            entry.incremental = Some(cache);
            false
        }
    }
}

fn extend_open_fence(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    reasoning: bool,
    mut tail: OpenFenceTail,
) -> Option<OpenFenceTail> {
    if tail.source_start > text.len() {
        return None;
    }
    let first = text[tail.source_start..].lines().next()?;
    if parse_opening_fence(first) != Some(tail.fence) {
        return None;
    }
    if fence_closed(text, tail.source_start, tail.fence) {
        return None;
    }
    if tail.committed_end > text.len() {
        return None;
    }
    let new_complete = last_complete_end(text, tail.source_start);
    if new_complete < tail.committed_end {
        return None;
    }

    let trailing_blank = take_trailing_blank(&mut entry.lines, has_trailing_blank);
    entry.lines.truncate(tail.committed_line_count);
    entry
        .code_blocks
        .retain(|block| block.line < tail.committed_line_count);

    if new_complete > tail.committed_end {
        let continue_open = tail.committed_end > tail.source_start;
        append_fence_chunk(
            entry,
            &text[tail.committed_end..new_complete],
            render,
            width,
            &mut tail,
            continue_open,
            reasoning,
        );
        tail.committed_end = new_complete;
        tail.committed_line_count = entry.lines.len();
    }

    if text.len() > tail.committed_end {
        append_fence_preview(entry, &text[tail.committed_end..], width, &tail, reasoning);
    }

    refresh_fence_copy(entry, text, &tail, width);
    if let Some(blank) = trailing_blank {
        entry.lines.push(blank);
    }
    Some(tail)
}

fn append_fence_chunk(
    entry: &mut CachedEntry,
    chunk: &str,
    render: EntryContentRender,
    width: usize,
    tail: &mut OpenFenceTail,
    continue_open: bool,
    reasoning: bool,
) {
    if chunk.is_empty() {
        return;
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
        tail.highlighter = state.take_highlighter();
        return;
    }

    let mut state =
        CodeFenceState::continue_open(tail.fence, tail.language.clone(), tail.highlighter.take());
    let rendered = render_markdown(chunk, padded_content_width(width), &mut state);
    let mut lines = rendered.lines;
    if reasoning {
        Theme::reasoning_output(&mut lines);
    }
    entry.lines.extend(lines.into_iter().map(pad_display_line));
    tail.highlighter = state.take_highlighter();
}

fn append_fence_preview(
    entry: &mut CachedEntry,
    chunk: &str,
    width: usize,
    tail: &OpenFenceTail,
    reasoning: bool,
) {
    if chunk.is_empty() {
        return;
    }
    let mut state =
        CodeFenceState::continue_open(tail.fence, tail.language.clone(), tail.highlighter.clone());
    let rendered = render_markdown(chunk, padded_content_width(width), &mut state);
    let mut lines = rendered.lines;
    if reasoning {
        Theme::reasoning_output(&mut lines);
    }
    entry.lines.extend(lines.into_iter().map(pad_display_line));
}

fn refresh_fence_copy(entry: &mut CachedEntry, text: &str, tail: &OpenFenceTail, width: usize) {
    let Some(body) = fence_body_source(text, tail.source_start, tail.fence) else {
        return;
    };
    let inner = padded_content_width(width);
    if let Some(block) = entry
        .code_blocks
        .iter_mut()
        .find(|block| block.line == tail.header_line)
    {
        block.text = Arc::from(body);
        return;
    }
    let Some(copy_columns) = code_block_copy_columns(inner) else {
        return;
    };
    entry.code_blocks.push(CachedCodeBlock {
        line: tail.header_line,
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
    has_trailing_blank: bool,
    mut tail: OpenTableTail,
) -> Option<OpenTableTail> {
    if tail.source_start > text.len() {
        return None;
    }
    let complete_end = last_complete_end(text, tail.source_start);
    if complete_end < tail.committed_end {
        return None;
    }

    let mut new_rows = Vec::new();
    if complete_end > tail.committed_end {
        for line in text[tail.committed_end..complete_end].split_inclusive('\n') {
            let line = line
                .strip_suffix('\n')
                .unwrap_or(line)
                .strip_suffix('\r')
                .unwrap_or(line);
            new_rows.push(tail.table.paint_data_row(line)?);
        }
    }

    let incomplete = &text[complete_end..];
    let preview = if incomplete.is_empty() {
        None
    } else {
        Some(tail.table.paint_data_row(incomplete)?)
    };

    let trailing_blank = take_trailing_blank(&mut entry.lines, has_trailing_blank);
    entry.lines.truncate(tail.committed_line_count);
    entry
        .code_blocks
        .retain(|block| block.line < tail.committed_line_count);
    for row in new_rows {
        entry.lines.extend(row.into_iter().map(pad_display_line));
    }
    tail.committed_end = complete_end;
    tail.committed_line_count = entry.lines.len();
    if let Some(preview) = preview {
        entry
            .lines
            .extend(preview.into_iter().map(pad_display_line));
    }
    entry
        .lines
        .push(pad_display_line(streaming_table_bottom_border(
            tail.table.column_widths(),
        )));
    if let Some(blank) = trailing_blank {
        entry.lines.push(blank);
    }
    Some(tail)
}

fn promote_stable_tail(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    content_end: usize,
) -> bool {
    let Some(cache) = entry.incremental.as_ref() else {
        return false;
    };
    if cache.stable_source_len > text.len() {
        return false;
    }
    let mutable_source = &text[cache.stable_source_len..];
    let new_tail_start = cache.stable_source_len + incremental_markdown_tail_start(mutable_source);
    if new_tail_start > text.len() {
        return false;
    }
    let preserve_end = cache.stable_line_count;
    if preserve_end >= content_end || preserve_end > entry.lines.len() {
        return false;
    }

    let previous_stable_source_len = cache.stable_source_len;
    let trailing_blank = take_trailing_blank(&mut entry.lines, has_trailing_blank);
    entry.lines.truncate(preserve_end);
    entry.code_blocks.retain(|block| block.line < preserve_end);
    append_entry_segment_into(
        &mut entry.lines,
        &mut entry.code_blocks,
        &text[previous_stable_source_len..new_tail_start],
        width,
        render,
    );
    append_entry_segment_into(
        &mut entry.lines,
        &mut entry.code_blocks,
        &text[new_tail_start..],
        width,
        render,
    );
    if let Some(trailing_blank) = trailing_blank {
        entry.lines.push(trailing_blank);
    }
    let content_line_count = entry
        .lines
        .len()
        .saturating_sub(usize::from(has_trailing_blank));
    entry.incremental = Some(inspect_incremental(
        text,
        render,
        width,
        Some(content_line_count),
    ));
    true
}

fn take_trailing_blank(
    lines: &mut Vec<Line<'static>>,
    has_trailing_blank: bool,
) -> Option<Line<'static>> {
    if has_trailing_blank {
        lines.pop()
    } else {
        None
    }
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
