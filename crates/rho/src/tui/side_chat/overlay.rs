use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

use super::super::{
    copy_interaction::CopyHit,
    line_editor::LineEditor,
    overlay_panel::{
        clamp_panel_scroll, overlay_panel_inner_width, overlay_panel_layout, render_overlay_panel,
        OverlayPanelFrame,
    },
    render::{display_width, render_entry_with_options, TrailingBlank},
    theme::Theme,
    Entry,
};

pub(super) const TITLE: &str = "Side chat";
const FOOTER_IDLE: &str = "Enter send   Esc close";
const FOOTER_BUSY: &str = "Enter send   Esc cancel";
const INPUT_PREFIX: &str = "> ";

pub(super) struct SideScrollMetrics {
    pub(super) body_len: usize,
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
    inner_width: usize,
    metrics: SideScrollMetrics,
}

#[derive(Debug)]
pub(super) struct SideOverlay {
    pub(super) entries: Vec<Entry>,
    pub(super) composer: LineEditor,
    pub(super) scroll: usize,
    pub(super) busy: bool,
    pub(super) snapshot: String,
    streaming_assistant: Option<String>,
}

impl SideOverlay {
    pub(super) fn new(snapshot: String) -> Self {
        Self {
            entries: Vec::new(),
            composer: LineEditor::new(""),
            scroll: 0,
            busy: false,
            snapshot,
            streaming_assistant: None,
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

fn side_overlay_panel_body(overlay: &SideOverlay, inner_width: usize) -> SidePanelBody {
    let mut body = overlay.body_lines(inner_width);
    body.lines.push(Line::from(Span::styled(
        "─".repeat(inner_width),
        Theme::dim(),
    )));
    let input = format!("{INPUT_PREFIX}{}", overlay.composer.value);
    body.lines.push(Line::from(Span::styled(
        truncate_input(&input, inner_width),
        Theme::input_prompt(),
    )));
    body
}

pub(super) fn side_scroll_metrics(overlay: &SideOverlay, area: Rect) -> Option<SideScrollMetrics> {
    Some(prepare_side_panel(overlay, area)?.metrics)
}

fn prepare_side_panel(overlay: &SideOverlay, area: Rect) -> Option<PreparedSidePanel> {
    if area.width < 8 || area.height < 8 {
        return None;
    }
    let mut inner_width = overlay_panel_inner_width(area);
    let mut body = side_overlay_panel_body(overlay, inner_width);
    if body.lines.len() > overlay_panel_layout(area, body.lines.len()).body_rows {
        // Only reflow when the scrollbar takes a content column. Metrics,
        // hit targets and the frame all consume this final render.
        inner_width = inner_width.saturating_sub(1).max(1);
        body = side_overlay_panel_body(overlay, inner_width);
    }
    let body_len = body.lines.len();
    let body_rows = overlay_panel_layout(area, body_len).body_rows;
    Some(PreparedSidePanel {
        body,
        inner_width,
        metrics: SideScrollMetrics {
            body_len,
            body_rows,
            max_scroll: side_max_scroll(body_len, body_rows),
        },
    })
}

fn side_max_scroll(body_len: usize, body_rows: usize) -> usize {
    let input_row = body_len.saturating_sub(1);
    let transcript_rows = body_rows.saturating_sub(2);
    input_row.saturating_sub(transcript_rows.saturating_add(1))
}

fn resolve_side_scroll(scroll: usize, metrics: &SideScrollMetrics) -> usize {
    if scroll == usize::MAX {
        metrics.max_scroll
    } else {
        let input_row = metrics.body_len.saturating_sub(1);
        let transcript_rows = metrics.body_rows.saturating_sub(2);
        clamp_panel_scroll(scroll, input_row.saturating_sub(1), transcript_rows)
    }
}

pub(super) fn side_overlay_frame(overlay: &SideOverlay, area: Rect) -> Option<OverlayPanelFrame> {
    let PreparedSidePanel {
        body,
        inner_width,
        metrics,
    } = prepare_side_panel(overlay, area)?;
    let scroll = resolve_side_scroll(overlay.scroll, &metrics);
    let input_row = body.lines.len().saturating_sub(1);

    let footer = if overlay.busy {
        FOOTER_BUSY
    } else {
        FOOTER_IDLE
    };
    let mut frame = render_overlay_panel(TITLE, footer, &body.lines, scroll, area);
    frame.copy_hits = body.copy_hits;
    let cursor_x = INPUT_PREFIX
        .chars()
        .count()
        .saturating_add(overlay.composer.cursor)
        .min(inner_width.saturating_sub(1));
    let input_screen_row = metrics
        .body_rows
        .saturating_sub(1)
        .min(input_row.saturating_sub(scroll));
    frame.cursor = Some(Position {
        x: frame
            .outer
            .x
            .saturating_add(1)
            .saturating_add(cursor_x as u16),
        y: frame
            .outer
            .y
            .saturating_add(1)
            .saturating_add(input_screen_row as u16),
    });
    Some(frame)
}

fn truncate_input(input: &str, width: usize) -> String {
    if display_width(input) <= width {
        return input.to_string();
    }
    crate::tui::render::truncate_one_line(input, width)
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;
