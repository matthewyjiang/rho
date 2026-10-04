use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

use super::super::{
    copy_interaction::CopyHit,
    overlay_panel::{
        overlay_panel_inner_width, overlay_panel_layout, render_overlay_panel, OverlayPanelFrame,
    },
    panel_pointer::PanelPointer,
    render::{display_width, render_entry_with_options, TrailingBlank},
    screen_layout::visible_composer_start,
    theme::Theme,
    Entry,
};
use super::composer::SideComposer;

pub(super) const TITLE: &str = "Side chat";
const FOOTER_IDLE: &str = "Enter send   Esc close";
const FOOTER_BUSY: &str = "Esc close   Ctrl+C cancel";
const INPUT_PREFIX: &str = "> ";

pub(super) struct SideScrollMetrics {
    pub(super) max_scroll: usize,
}

#[derive(Default)]
struct SidePanelBody {
    lines: Vec<Line<'static>>,
    copy_hits: Vec<CopyHit>,
}

struct PreparedSidePanel {
    /// Transcript rows only; the composer is pinned below them.
    body: SidePanelBody,
    /// The divider, then the visible composer rows.
    pinned: Vec<Line<'static>>,
    metrics: SideScrollMetrics,
    composer: SideComposerLayout,
}

/// The composer's painted row window, for caret paint, pointer hits, and the
/// window the paint path retains.
#[derive(Clone, Copy, Debug)]
pub(super) struct SideComposerLayout {
    /// First painted wrapped row of the composer text.
    pub(super) view_start: usize,
    /// Wrapped text width, after the prompt prefix.
    pub(super) text_width: usize,
    /// Caret wrapped row and display column in the composer text.
    caret: Position,
}

pub(super) struct SideOverlayFrame {
    pub(super) frame: OverlayPanelFrame,
    pub(super) metrics: SideScrollMetrics,
    pub(super) composer: SideComposerLayout,
}

impl SideOverlayFrame {
    /// Screen cells of the painted composer rows: the pinned rows below the
    /// divider.
    fn composer_rect(&self) -> Rect {
        let pinned = self.frame.pinned();
        Rect {
            y: pinned.y.saturating_add(1),
            height: pinned.height.saturating_sub(1),
            ..pinned
        }
    }

    /// Raw composer char index under screen cell `column`/`row`. `clamp`
    /// pins cells outside the composer rows to their nearest edge, for drags
    /// that leave the composer; otherwise those cells miss.
    pub(super) fn composer_index_at(
        &self,
        composer: &SideComposer,
        column: u16,
        row: u16,
        clamp: bool,
    ) -> Option<usize> {
        let rect = self.composer_rect();
        if rect.is_empty() || (!clamp && !rect.contains(Position::new(column, row))) {
            return None;
        }
        let row = row.clamp(rect.top(), rect.bottom() - 1);
        let column = column.clamp(rect.left(), rect.right() - 1);
        let layout = self.composer;
        let text_row = layout.view_start + usize::from(row - rect.y);
        let text_column = usize::from(column - rect.x).saturating_sub(display_width(INPUT_PREFIX));
        Some(
            composer
                .buffer
                .char_index_at(layout.text_width, text_row, text_column),
        )
    }
}

#[derive(Debug)]
pub(super) struct SideOverlay {
    pub(super) entries: Vec<Entry>,
    pub(super) composer: SideComposer,
    pub(super) scroll: usize,
    pub(super) busy: bool,
    pub(super) snapshot: String,
    streaming_assistant: Option<String>,
    /// Transcript selection, scrollbar drag, and hover. Body lines only grow
    /// at the end, so a selection anchored by line survives streaming.
    pub(super) pointer: PanelPointer,
}

impl SideOverlay {
    pub(super) fn new(snapshot: String) -> Self {
        Self {
            entries: Vec::new(),
            composer: SideComposer::default(),
            scroll: 0,
            busy: false,
            snapshot,
            streaming_assistant: None,
            pointer: PanelPointer::default(),
        }
    }

    pub(super) fn push_user(&mut self, text: String) {
        self.entries.push(Entry::User(text));
        self.follow_end();
    }

    pub(super) fn push_notice(&mut self, text: String) {
        self.entries.push(Entry::Error(text));
        self.follow_end();
    }

    pub(super) fn fail_run(&mut self, text: String) {
        self.commit_stream();
        self.busy = false;
        self.push_notice(text);
    }

    pub(super) fn append_assistant_delta(&mut self, delta: &str) {
        match &mut self.streaming_assistant {
            Some(text) => text.push_str(delta),
            None => self.streaming_assistant = Some(delta.to_owned()),
        }
        self.follow_end();
    }

    pub(super) fn reset_assistant_stream(&mut self) {
        self.streaming_assistant = None;
    }

    pub(super) fn push_tool(&mut self, name: String) {
        self.commit_stream();
        // Side-chat events expose only the tool name, not a tool card or result.
        self.entries.push(Entry::Notice(format!("tool {name}")));
        self.follow_end();
    }

    pub(super) fn finish_assistant(&mut self) {
        self.commit_stream();
        self.busy = false;
        self.follow_end();
    }

    pub(super) fn mark_cancelled(&mut self) {
        self.commit_stream();
        self.busy = false;
        self.follow_end();
    }

    fn commit_stream(&mut self) {
        if let Some(text) = self.streaming_assistant.take() {
            if !text.is_empty() {
                self.entries.push(Entry::Assistant(text.into()));
            }
        }
    }

    fn follow_end(&mut self) {
        self.scroll = usize::MAX;
    }

    fn body_lines(&self, width: usize) -> SidePanelBody {
        let width = width.max(1);
        let mut body = SidePanelBody::default();
        // Side chat currently produces text entries only, with no tool output
        // bodies or image placements to reserve.
        let mut render = |entry, trailing_blank| {
            let rendered = render_entry_with_options(
                entry,
                width,
                /*max_tool_output_lines*/ 0,
                /*max_image_height*/ 0,
                trailing_blank,
            );
            body.copy_hits
                .extend(rendered.code_blocks.into_iter().map(|block| CopyHit {
                    row: body.lines.len().saturating_add(block.top_line),
                    // Entry rendering adds the transcript's left padding column.
                    columns: block.copy_columns.start.saturating_add(1)
                        ..block.copy_columns.end.saturating_add(1),
                    text: block.text,
                }));
            body.lines.extend(rendered.lines);
        };
        for entry in &self.entries {
            render(entry, TrailingBlank::Include);
        }
        if let Some(text) = &self.streaming_assistant {
            render(&Entry::Assistant(text.clone().into()), TrailingBlank::Omit);
        } else if self.busy {
            body.lines.push(Line::from(Span::styled("…", Theme::dim())));
        }
        body
    }

    /// Pins `top_line` as the first visible body row (scrollbar drag). The
    /// last position follows the end again, like the history bottom.
    pub(super) fn scroll_to(&mut self, top_line: usize, metrics: &SideScrollMetrics) {
        if top_line >= metrics.max_scroll {
            self.follow_end();
        } else {
            self.scroll = top_line;
        }
    }

    pub(super) fn scroll_by(&mut self, delta: isize, metrics: &SideScrollMetrics) {
        let current = resolve_side_scroll(self.scroll, metrics);
        self.scroll = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .saturating_add(delta as usize)
                .min(metrics.max_scroll)
        };
    }
}

pub(super) fn side_scroll_metrics(overlay: &SideOverlay, area: Rect) -> Option<SideScrollMetrics> {
    Some(prepare_side_panel(overlay, area)?.metrics)
}

fn prepare_side_panel(overlay: &SideOverlay, area: Rect) -> Option<PreparedSidePanel> {
    if area.width < 8 || area.height < 8 {
        return None;
    }
    let inner_width = overlay_panel_inner_width(area);
    let prefix_width = display_width(INPUT_PREFIX);
    let text_width = inner_width.saturating_sub(prefix_width).max(1);
    let input = overlay.composer.buffer.frame(text_width);
    // Like the main composer, a tall draft grows up to the panel but always
    // leaves the divider and one transcript row above it.
    let max_composer_rows = overlay_panel_layout(area, usize::MAX)
        .body_rows
        .saturating_sub(2)
        .max(1);
    let rows = input.lines.len().min(max_composer_rows);
    let caret_row = (input.cursor.y as usize).min(input.lines.len().saturating_sub(1));
    let view_start = visible_composer_start(
        caret_row,
        input.lines.len(),
        rows,
        overlay.composer.buffer.view_start(),
    );
    let mut pinned = Vec::with_capacity(rows + 1);
    pinned.push(Line::from(Span::styled(
        "─".repeat(inner_width),
        Theme::dim(),
    )));
    for (index, mut line) in input
        .lines
        .into_iter()
        .enumerate()
        .skip(view_start)
        .take(rows)
    {
        let prefix = if index == 0 {
            INPUT_PREFIX.to_owned()
        } else {
            " ".repeat(prefix_width)
        };
        line.spans.insert(0, Span::raw(prefix));
        pinned.push(line.style(Theme::input_prompt()));
    }

    // Only the transcript scrolls, so only it gives up a scrollbar column.
    let transcript_rows = |len: usize| {
        overlay_panel_layout(area, len + pinned.len())
            .body_rows
            .saturating_sub(pinned.len())
    };
    let mut body = overlay.body_lines(inner_width);
    if body.lines.len() > transcript_rows(body.lines.len()) {
        body = overlay.body_lines(inner_width.saturating_sub(1));
    }
    let body_rows = transcript_rows(body.lines.len());
    Some(PreparedSidePanel {
        metrics: SideScrollMetrics {
            max_scroll: body.lines.len().saturating_sub(body_rows),
        },
        body,
        pinned,
        composer: SideComposerLayout {
            view_start,
            text_width,
            caret: input.cursor,
        },
    })
}

fn resolve_side_scroll(scroll: usize, metrics: &SideScrollMetrics) -> usize {
    scroll.min(metrics.max_scroll)
}

/// The side overlay as painted at `area`, with the scroll metrics of that
/// same render so pointer scrolling needs no second body render.
pub(super) fn side_overlay_frame(overlay: &SideOverlay, area: Rect) -> Option<SideOverlayFrame> {
    let PreparedSidePanel {
        body,
        pinned,
        metrics,
        composer,
    } = prepare_side_panel(overlay, area)?;
    let scroll = resolve_side_scroll(overlay.scroll, &metrics);

    let footer = if overlay.busy {
        FOOTER_BUSY
    } else {
        FOOTER_IDLE
    };
    let mut frame = render_overlay_panel(TITLE, footer, body.lines, pinned, scroll, area);
    frame.copy_hits = body.copy_hits;
    let mut prepared = SideOverlayFrame {
        frame,
        metrics,
        composer,
    };
    let rect = prepared.composer_rect();
    if !rect.is_empty() {
        let caret_row = (composer.caret.y as usize).saturating_sub(composer.view_start);
        let caret_column = display_width(INPUT_PREFIX) + composer.caret.x as usize;
        prepared.frame.cursor = Some(Position {
            x: rect.x
                + u16::try_from(caret_column)
                    .unwrap_or(u16::MAX)
                    .min(rect.width - 1),
            y: rect.y
                + u16::try_from(caret_row)
                    .unwrap_or(u16::MAX)
                    .min(rect.height - 1),
        });
    }
    Some(prepared)
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;
