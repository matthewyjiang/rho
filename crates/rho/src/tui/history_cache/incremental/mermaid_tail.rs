//! Live mermaid fence tail.
//!
//! Supported grammars are line-oriented: a complete-line prefix of a valid
//! diagram is usually a smaller valid diagram. Each completed line re-renders
//! that prefix. Transient parse failures keep the last successful paint;
//! terminal failures latch to source until the fence closes.

use std::sync::Arc;

use ratatui::text::Line;

use super::super::{append_entry_segment_into, CachedCodeBlock, CachedEntry, EntryContentRender};
use super::{fence_body_source, fence_closed, last_complete_end, take_trailing_blank};
use crate::tui::{
    markdown::{
        mermaid_opening_fence, mermaid_streaming_panel, streaming_mermaid_prefix, CodeFence,
        StreamingMermaidPrefix,
    },
    render::{pad_display_line, padded_content_width},
    theme::Theme,
};

/// Monotone stream mode. Upgrades to diagram at most once, latches at most once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MermaidStreamMode {
    Probing,
    Diagram,
    Latched,
}

#[derive(Clone)]
struct LastGoodMermaid {
    title: &'static str,
    art: Vec<Line<'static>>,
}

#[derive(Clone)]
pub(super) struct OpenMermaidTail {
    source_start: usize,
    committed_end: usize,
    header_line: usize,
    fence: CodeFence,
    mode: MermaidStreamMode,
    last_good: Option<LastGoodMermaid>,
}

pub(super) fn open_from_source(
    text: &str,
    tail_start: usize,
    stable_line_count: usize,
    width: usize,
) -> Option<OpenMermaidTail> {
    let first = text[tail_start..].lines().next()?;
    let opening = mermaid_opening_fence(first)?;
    if fence_closed(text, tail_start, opening.fence) {
        return None;
    }
    let committed_end = last_complete_end(text, tail_start);
    let mut tail = OpenMermaidTail {
        source_start: tail_start,
        committed_end,
        header_line: stable_line_count,
        fence: opening.fence,
        mode: MermaidStreamMode::Probing,
        last_good: None,
    };
    probe_if_needed(&mut tail, text, width);
    Some(tail)
}

pub(super) fn overlay_diagram(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    reasoning: bool,
    tail: &mut OpenMermaidTail,
) {
    if tail.mode != MermaidStreamMode::Diagram || tail.last_good.is_none() {
        return;
    }
    repaint_tail(
        entry,
        text,
        render,
        width,
        has_trailing_blank,
        reasoning,
        tail,
    );
}

pub(super) fn extend(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    reasoning: bool,
    mut tail: OpenMermaidTail,
) -> Option<OpenMermaidTail> {
    if tail.source_start > text.len() {
        return None;
    }
    let first = text[tail.source_start..].lines().next()?;
    if mermaid_opening_fence(first).map(|opening| opening.fence) != Some(tail.fence) {
        return None;
    }
    if fence_closed(text, tail.source_start, tail.fence) {
        return None;
    }
    let new_complete = last_complete_end(text, tail.source_start);
    if new_complete < tail.committed_end {
        return None;
    }

    let grew = new_complete > tail.committed_end;
    tail.committed_end = new_complete;
    if grew {
        probe_if_needed(&mut tail, text, width);
    }
    // Diagram mode ignores incomplete last-line tokens; source modes still
    // preview them. Skip the truncate/repaint when nothing visible changed.
    if grew || tail.mode != MermaidStreamMode::Diagram {
        repaint_tail(
            entry,
            text,
            render,
            width,
            has_trailing_blank,
            reasoning,
            &tail,
        );
    }
    Some(tail)
}

fn probe_if_needed(tail: &mut OpenMermaidTail, text: &str, width: usize) {
    if tail.mode == MermaidStreamMode::Latched {
        return;
    }
    let body = complete_body(text, tail);
    if body.trim().is_empty() {
        return;
    }
    tail.apply(streaming_mermaid_prefix(body, padded_content_width(width)));
}

impl OpenMermaidTail {
    fn apply(&mut self, prefix: StreamingMermaidPrefix) {
        match prefix {
            StreamingMermaidPrefix::Diagram { title, lines } => {
                if self.mode != MermaidStreamMode::Latched {
                    self.mode = MermaidStreamMode::Diagram;
                    self.last_good = Some(LastGoodMermaid { title, art: lines });
                }
            }
            StreamingMermaidPrefix::Transient => {}
            StreamingMermaidPrefix::Terminal | StreamingMermaidPrefix::Unsafe => {
                self.mode = MermaidStreamMode::Latched;
                self.last_good = None;
            }
        }
    }
}

fn complete_body<'a>(text: &'a str, tail: &OpenMermaidTail) -> &'a str {
    let region = &text[tail.source_start..tail.committed_end];
    let Some(first_nl) = region.find('\n') else {
        return "";
    };
    let body = &region[first_nl + 1..];
    body.strip_suffix('\n')
        .unwrap_or(body)
        .strip_suffix('\r')
        .unwrap_or(body)
}

fn repaint_tail(
    entry: &mut CachedEntry,
    text: &str,
    render: EntryContentRender,
    width: usize,
    has_trailing_blank: bool,
    reasoning: bool,
    tail: &OpenMermaidTail,
) {
    let trailing_blank = take_trailing_blank(&mut entry.lines, has_trailing_blank);
    entry.lines.truncate(tail.header_line);
    entry
        .code_blocks
        .retain(|block| block.line < tail.header_line);
    if tail.mode == MermaidStreamMode::Diagram {
        paint_diagram(entry, text, width, reasoning, tail);
    } else {
        append_entry_segment_into(
            &mut entry.lines,
            &mut entry.code_blocks,
            &text[tail.source_start..],
            width,
            render,
        );
    }
    if let Some(blank) = trailing_blank {
        entry.lines.push(blank);
    }
}

fn paint_diagram(
    entry: &mut CachedEntry,
    text: &str,
    width: usize,
    reasoning: bool,
    tail: &OpenMermaidTail,
) {
    let Some(last_good) = tail.last_good.as_ref() else {
        return;
    };
    let source = fence_body_source(text, tail.source_start, tail.fence).unwrap_or_default();
    let rendered = mermaid_streaming_panel(
        last_good.title,
        last_good.art.clone(),
        source,
        padded_content_width(width),
    );
    let mut lines = rendered.lines;
    if reasoning {
        Theme::reasoning_output(&mut lines);
    }
    let local_start = entry.lines.len();
    entry.code_blocks.extend(
        rendered
            .code_blocks
            .into_iter()
            .map(|block| CachedCodeBlock {
                line: local_start + block.top_line,
                copy_columns: block.copy_columns.start.saturating_add(1)
                    ..block.copy_columns.end.saturating_add(1),
                text: Arc::from(block.text),
            }),
    );
    entry.lines.extend(lines.into_iter().map(pad_display_line));
}

#[cfg(test)]
#[path = "mermaid_tail_tests.rs"]
mod tests;
