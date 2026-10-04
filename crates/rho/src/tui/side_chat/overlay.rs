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
    render::{display_width, render_entry_with_options, InputFrame, TrailingBlank},
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
    pub(super) body_rows: usize,
    pub(super) max_scroll: usize,
}

#[derive(Default)]
struct SidePanelBody {
    lines: Vec<Line<'static>>,
    copy_hits: Vec<CopyHit>,
}

struct PreparedSidePanel {
    body: SidePanelBody,
    metrics: SideScrollMetrics,
    composer: SideComposerLayout,
}

/// Where the composer sits in the panel body, for caret paint, pointer hits,
/// and the row window the paint path retains.
#[derive(Clone, Copy, Debug)]
pub(super) struct SideComposerLayout {
    /// Body line of the first painted composer row (after the divider).
    first_line: usize,
    /// Painted composer rows.
    rows: usize,
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
    /// Raw composer char index under screen cell `column`/`row`. `clamp`
    /// pins cells outside the painted composer rows to their nearest edge,
    /// for drags that leave the composer; otherwise those cells miss.
    pub(super) fn composer_index_at(
        &self,
        composer: &SideComposer,
        column: u16,
        row: u16,
        clamp: bool,
    ) -> Option<usize> {
        let layout = self.composer;
        let body = self.frame.body();
        let scroll = self.frame.scroll();
        // Screen rows of the painted composer, clipped to the visible body.
        let first = layout.first_line.max(scroll);
        let last = (layout.first_line + layout.rows).min(scroll + body.height as usize);
        if first >= last || body.width == 0 {
            return None;
        }
        let line = scroll + usize::from(row.saturating_sub(body.y));
        let inside_rows = row >= body.y && (first..last).contains(&line);
        let inside_columns = column >= body.x && column < body.x.saturating_add(body.width);
        let line = match (inside_rows && inside_columns, clamp) {
            (true, _) => line,
            (false, false) => return None,
            (false, true) => line.clamp(first, last - 1),
        };
        let column = column.clamp(body.x, body.x.saturating_add(body.width - 1));
        let text_column = usize::from(column - body.x).saturating_sub(display_width(INPUT_PREFIX));
        let text_row = layout.view_start + (line - layout.first_line);
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

    /// The composer is the end of the scrolled body; editing it scrolls back
    /// to the end so the draft and caret stay visible.
    pub(super) fn reveal_composer(&mut self) {
        self.follow_end();
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
        let target = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        // Reaching the end follows it again, so a growing draft stays in view.
        self.scroll_to(target, metrics);
    }
}

pub(super) fn side_scroll_metrics(overlay: &SideOverlay, area: Rect) -> Option<SideScrollMetrics> {
    Some(prepare_side_panel(overlay, area)?.metrics)
}

/// Transcript rows plus the wrapped composer at `inner_width`.
fn side_panel_parts(overlay: &SideOverlay, inner_width: usize) -> (SidePanelBody, InputFrame) {
    let text_width = inner_width
        .saturating_sub(display_width(INPUT_PREFIX))
        .max(1);
    (
        overlay.body_lines(inner_width),
        overlay.composer.buffer.frame(text_width),
    )
}

fn prepare_side_panel(overlay: &SideOverlay, area: Rect) -> Option<PreparedSidePanel> {
    if area.width < 8 || area.height < 8 {
        return None;
    }
    // Like the main composer, a tall draft grows up to the panel but always
    // leaves the divider and one transcript row above it.
    let max_composer_rows = overlay_panel_layout(area, usize::MAX)
        .body_rows
        .saturating_sub(2)
        .max(1);
    let mut inner_width = overlay_panel_inner_width(area);
    let (mut body, mut input) = side_panel_parts(overlay, inner_width);
    let body_len = body.lines.len() + 1 + input.lines.len().min(max_composer_rows);
    if body_len > overlay_panel_layout(area, body_len).body_rows {
        // Resolve the scrollbar width before laying out the composer, so its
        // wrap matches the painted content width.
        inner_width = inner_width.saturating_sub(1).max(1);
        (body, input) = side_panel_parts(overlay, inner_width);
    }
    body.lines.push(Line::from(Span::styled(
        "─".repeat(inner_width),
        Theme::dim(),
    )));
    let prefix_width = display_width(INPUT_PREFIX);
    let rows = input.lines.len().min(max_composer_rows);
    let caret_row = (input.cursor.y as usize).min(input.lines.len().saturating_sub(1));
    let view_start = visible_composer_start(
        caret_row,
        input.lines.len(),
        rows,
        overlay.composer.buffer.view_start(),
    );
    let first_line = body.lines.len();
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
        body.lines.push(line.style(Theme::input_prompt()));
    }
    let body_len = body.lines.len();
    let body_rows = overlay_panel_layout(area, body_len).body_rows;
    Some(PreparedSidePanel {
        body,
        metrics: SideScrollMetrics {
            body_rows,
            max_scroll: body_len.saturating_sub(body_rows),
        },
        composer: SideComposerLayout {
            first_line,
            rows,
            view_start,
            text_width: inner_width.saturating_sub(prefix_width).max(1),
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
        metrics,
        composer,
    } = prepare_side_panel(overlay, area)?;
    let scroll = resolve_side_scroll(overlay.scroll, &metrics);

    let footer = if overlay.busy {
        FOOTER_BUSY
    } else {
        FOOTER_IDLE
    };
    let mut frame = render_overlay_panel(TITLE, footer, body.lines, scroll, area);
    frame.copy_hits = body.copy_hits;
    let caret_line =
        composer.first_line + (composer.caret.y as usize).saturating_sub(composer.view_start);
    let caret_screen_row = metrics
        .body_rows
        .saturating_sub(1)
        .min(caret_line.saturating_sub(scroll));
    let caret_column = (display_width(INPUT_PREFIX) + composer.caret.x as usize)
        .min(frame.body().width.saturating_sub(1) as usize);
    frame.cursor = Some(Position {
        x: frame.body().x.saturating_add(caret_column as u16),
        y: frame.body().y.saturating_add(caret_screen_row as u16),
    });
    Some(SideOverlayFrame {
        frame,
        metrics,
        composer,
    })
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;
