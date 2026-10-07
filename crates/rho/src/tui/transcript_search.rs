//! Incremental find over the rendered transcript, opened with
//! `search_transcript` (Ctrl+F by default).
//!
//! Search matches what the transcript paints: the session header, measured
//! entries, and live rows, case-insensitively within one rendered row.
//! Collapsed tool output and text hidden by display settings are not searched.
//! Key handlers only edit the query or request a step; every frame while
//! search is open recollects matches against the painted document, so
//! streaming text and live tool rows stay current, then applies the step.

use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::Backend, buffer::Buffer, layout::Rect, style::Style, text::Line, Terminal};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{
    composer_chrome::join_footer_parts,
    frame_context::FrameContext,
    line_editor::LineEditor,
    line_editor_view::EditorPresentation,
    text_selection::is_code_block_copy_span,
    view_composer::{editor_frame, ComposerFrame},
    App, ComposerMode, HistoryScroll, Theme,
};

/// One hit: an absolute history row and the display columns it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TranscriptMatch {
    pub(super) line: usize,
    pub(super) columns: Range<usize>,
}

/// Which match the next frame should focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SearchStep {
    /// The query changed: focus the nearest hit above the starting viewport.
    Query,
    /// Previous hit, toward the top of the transcript.
    Older,
    /// Next hit, toward the live bottom.
    Newer,
}

/// Composer state while transcript search is open.
#[derive(Clone, Debug)]
pub(super) struct TranscriptSearch {
    pub(super) editor: LineEditor,
    /// Scroll position when search opened; Esc returns here.
    origin: HistoryScroll,
    /// First row below the viewport when search opened. A new query focuses
    /// the last hit above it, so search runs upward from what was on screen.
    anchor_line: usize,
    /// Hits in document order.
    matches: Vec<TranscriptMatch>,
    focus: Option<usize>,
    /// Width and row count the matches were last collected against.
    collected_for: Option<(usize, usize)>,
    pending: Option<SearchStep>,
}

impl TranscriptSearch {
    fn new(origin: HistoryScroll, anchor_line: usize) -> Self {
        Self {
            editor: LineEditor::new(""),
            origin,
            anchor_line,
            matches: Vec::new(),
            focus: None,
            collected_for: None,
            pending: None,
        }
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        self.editor.insert_text(text);
        self.pending = Some(SearchStep::Query);
    }

    /// Footer label, such as `3/17` or `no matches`. `None` for an empty query.
    fn position_label(&self) -> Option<String> {
        if self.editor.value.is_empty() {
            return None;
        }
        Some(match self.focus {
            Some(index) => format!("{}/{}", index + 1, self.matches.len()),
            None => "no matches".into(),
        })
    }
}

pub(super) fn transcript_search_frame(search: &TranscriptSearch, width: usize) -> ComposerFrame {
    let position = search.position_label();
    let prompt = join_footer_parts(
        ["find", position.as_deref().unwrap_or_default()]
            .into_iter()
            .chain(["↑↓ match", "Enter keep", "Esc cancel"]),
    );
    editor_frame(
        &prompt,
        search.editor.viewport(EditorPresentation::Plain, width),
        width,
    )
}

/// A lowercased query, plus its text when ASCII for the fast row filter.
pub(super) struct Needle {
    chars: Vec<char>,
    ascii: Option<String>,
}

pub(super) fn needle(query: &str) -> Needle {
    let chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let ascii = chars
        .iter()
        .all(char::is_ascii)
        .then(|| chars.iter().collect());
    Needle { chars, ascii }
}

/// Scratch space reused across rows so a whole-transcript scan does not
/// allocate per row.
#[derive(Default)]
struct ScanBuffers {
    text: String,
    /// Lowercased characters with the display columns of their grapheme.
    haystack: Vec<(char, Range<usize>)>,
}

/// Report display-column ranges of non-overlapping, case-insensitive
/// `needle` hits in one rendered row to `hit`. Code-block `COPY` buttons are
/// chrome, not text.
fn scan_line(
    line: &Line<'_>,
    needle: &Needle,
    buffers: &mut ScanBuffers,
    mut hit: impl FnMut(Range<usize>),
) {
    let needle_chars = needle.chars.as_slice();
    if needle_chars.is_empty() {
        return;
    }
    // Most rows miss. For ASCII rows a lowercase substring check rejects them
    // without the grapheme walk; it never rejects a row the walk would match.
    if let Some(ascii) = &needle.ascii {
        let text = &mut buffers.text;
        text.clear();
        for span in &line.spans {
            text.push_str(&span.content);
        }
        if text.is_ascii() {
            text.retain(|ch| !ch.is_ascii_control());
            text.make_ascii_lowercase();
            if !text.contains(ascii.as_str()) {
                return;
            }
        }
    }
    let haystack = &mut buffers.haystack;
    haystack.clear();
    let mut column = 0usize;
    for span in &line.spans {
        if is_code_block_copy_span(span) {
            let width = UnicodeWidthStr::width(span.content.as_ref());
            // NUL never appears in a typed query, so hits cannot span the button.
            haystack.push(('\0', column..column + width));
            column += width;
            continue;
        }
        // Hits cover whole graphemes, matching how cells are painted.
        for grapheme in span.content.graphemes(/*is_extended*/ true) {
            let width = UnicodeWidthStr::width(grapheme);
            let columns = column..column + width;
            haystack.extend(
                grapheme
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .flat_map(char::to_lowercase)
                    .map(|lower| (lower, columns.clone())),
            );
            column += width;
        }
    }
    let mut start = 0;
    while start + needle_chars.len() <= haystack.len() {
        let window = &haystack[start..start + needle_chars.len()];
        if window.iter().map(|(ch, _)| ch).eq(needle_chars.iter()) {
            hit(window[0].1.start..window[needle_chars.len() - 1].1.end);
            start += needle_chars.len();
        } else {
            start += 1;
        }
    }
}

/// Append hits for `lines`, numbered from `first_line`. Returns the row after
/// the last one scanned.
fn collect_matches<'a>(
    matches: &mut Vec<TranscriptMatch>,
    buffers: &mut ScanBuffers,
    lines: impl IntoIterator<Item = &'a Line<'static>>,
    first_line: usize,
    needle: &Needle,
) -> usize {
    let mut line = first_line;
    for row in lines {
        scan_line(row, needle, buffers, |columns| {
            matches.push(TranscriptMatch { line, columns });
        });
        line += 1;
    }
    line
}

/// Focused hit after `step`, wrapping at either end.
pub(super) fn step_focus(
    matches: &[TranscriptMatch],
    focus: Option<usize>,
    step: SearchStep,
    anchor_line: usize,
) -> Option<usize> {
    let last = matches.len().checked_sub(1)?;
    let focus = focus.map(|index| index.min(last));
    Some(match (step, focus) {
        (SearchStep::Query, _) | (SearchStep::Older | SearchStep::Newer, None) => matches
            .partition_point(|hit| hit.line < anchor_line)
            .checked_sub(1)
            .unwrap_or(last),
        (SearchStep::Older, Some(index)) => index.checked_sub(1).unwrap_or(last),
        (SearchStep::Newer, Some(index)) if index >= last => 0,
        (SearchStep::Newer, Some(index)) => index + 1,
    })
}

/// Keep the focus on the same hit, or the nearest one before it, after the
/// document changed underneath the search.
fn relocate_focus(matches: &[TranscriptMatch], previous: &TranscriptMatch) -> Option<usize> {
    let last = matches.len().checked_sub(1)?;
    let key = (previous.line, previous.columns.start);
    let after = matches.partition_point(|hit| (hit.line, hit.columns.start) <= key);
    Some(after.saturating_sub(1).min(last))
}

impl App {
    pub(super) fn open_transcript_search<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        let ctx = self.frame_context(area);
        self.measure_full_history(&ctx.layout, ctx.settings);
        let ctx = self.frame_context(area);
        let height = ctx.layout.history_content.height as usize;
        let anchor_line = self
            .visible_history_start(ctx.history_len, height)
            .saturating_add(height);
        let search = TranscriptSearch::new(self.history.scroll(), anchor_line);
        self.input_ui
            .set_composer(ComposerMode::TranscriptSearch(search));
        self.set_status_quiet("search transcript");
        Ok(())
    }

    /// Keys while transcript search owns the composer. Returns false in any
    /// other composer mode. History paging and jump keys are routed earlier.
    pub(super) fn handle_transcript_search_key(&mut self, key: KeyEvent) -> bool {
        let ComposerMode::TranscriptSearch(search) = self.input_ui.composer_mut() else {
            return false;
        };
        let step = match (key.modifiers, key.code) {
            (_, KeyCode::Esc) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                let origin = search.origin;
                self.close_transcript_search();
                self.history.scroll_chrome_mut().restore(origin);
                None
            }
            (_, KeyCode::Enter) => {
                self.close_transcript_search();
                None
            }
            (_, KeyCode::Up) => Some(SearchStep::Older),
            (_, KeyCode::Down) => Some(SearchStep::Newer),
            (_, KeyCode::Backspace) => {
                search.editor.backspace();
                Some(SearchStep::Query)
            }
            (_, KeyCode::Delete) => {
                search.editor.delete();
                Some(SearchStep::Query)
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                search.editor.insert_char(ch);
                Some(SearchStep::Query)
            }
            (_, KeyCode::Left) => {
                search.editor.move_cursor_left();
                None
            }
            (_, KeyCode::Right) => {
                search.editor.move_cursor_right();
                None
            }
            (_, KeyCode::Home) => {
                search.editor.move_cursor_home();
                None
            }
            (_, KeyCode::End) => {
                search.editor.move_cursor_end();
                None
            }
            _ => None,
        };
        if let (Some(step), ComposerMode::TranscriptSearch(search)) =
            (step, self.input_ui.composer_mut())
        {
            search.pending = Some(step);
        }
        self.input_ui.clear_paste_burst();
        self.ctrl_c_streak = 0;
        true
    }

    fn close_transcript_search(&mut self) {
        self.input_ui.set_composer(ComposerMode::Input);
        self.set_status_quiet("");
    }

    /// Recollect matches against this frame's document, keep the focus on the
    /// same hit, and apply a pending step. Returns true when the matches or
    /// scroll position changed, so the caller must rebuild the frame context
    /// before painting.
    ///
    /// A document at rest (no turn, no live rows, same width and row count)
    /// cannot have changed text, so its matches are reused. Otherwise rows can
    /// change in place, such as a streaming line, and every frame rescans.
    pub(super) fn sync_transcript_search(&mut self, ctx: &FrameContext) -> bool {
        let shape = (ctx.width, ctx.history_len);
        let at_rest = !self.loading_active() && ctx.live_history.lines.is_empty();
        let needle = match self.input_ui.composer() {
            ComposerMode::TranscriptSearch(search)
                if search.pending.is_some() || search.collected_for != Some(shape) || !at_rest =>
            {
                needle(&search.editor.value)
            }
            _ => return false,
        };
        let matches = self.collect_transcript_matches(ctx, &needle);
        let ComposerMode::TranscriptSearch(search) = self.input_ui.composer_mut() else {
            return false;
        };
        search.collected_for = Some(shape);
        let ComposerMode::TranscriptSearch(search) = self.input_ui.composer_mut() else {
            return false;
        };
        let previous = search
            .focus
            .and_then(|index| search.matches.get(index).cloned());
        let changed = matches != search.matches;
        search.matches = matches;
        search.focus = previous.and_then(|hit| relocate_focus(&search.matches, &hit));
        let Some(step) = search.pending.take() else {
            return changed;
        };
        search.focus = step_focus(&search.matches, search.focus, step, search.anchor_line);
        let target = search.focus.map(|index| search.matches[index].line);
        let origin = search.origin;
        match target {
            Some(line) => self.reveal_history_line(ctx, line),
            // Clearing the query, or typing past the last hit, returns to
            // where the search started, like incremental search in an editor.
            None if step == SearchStep::Query => self.history.scroll_chrome_mut().restore(origin),
            None => {}
        }
        true
    }

    /// Scroll so `line` is on screen with some context above it. Leaves the
    /// viewport alone when the row is already visible.
    fn reveal_history_line(&mut self, ctx: &FrameContext, line: usize) {
        let height = ctx.layout.history_content.height as usize;
        let start = self.visible_history_start(ctx.history_len, height);
        if (start..start.saturating_add(height)).contains(&line) {
            return;
        }
        self.history.scroll_chrome_mut().set_top_line(
            ctx.history_len,
            height,
            line.saturating_sub(height / 3),
        );
    }

    /// Hits across the session header, measured transcript rows, and live
    /// rows, numbered the same way as the painted history document.
    fn collect_transcript_matches(
        &mut self,
        ctx: &FrameContext,
        needle: &Needle,
    ) -> Vec<TranscriptMatch> {
        let mut matches = Vec::new();
        if needle.chars.is_empty() {
            return matches;
        }
        let mut buffers = ScanBuffers::default();
        let header_len = self.visible_session_header_len(ctx.width);
        let header = self.session_header_lines(ctx.width)[..header_len].to_vec();
        let mut line = collect_matches(&mut matches, &mut buffers, &header, 0, needle);
        self.sync_open_stream_tail();
        let cwd = self.info.runtime.cwd.clone();
        line = self
            .history
            .with_lines_and_images_mut(|cache, entries, images| {
                let rows = cache.measured_lines(entries, ctx.settings, &|index, sources| {
                    images.ready_images(index, sources, &cwd)
                });
                collect_matches(&mut matches, &mut buffers, rows, line, needle)
            });
        collect_matches(
            &mut matches,
            &mut buffers,
            &ctx.live_history.lines,
            line,
            needle,
        );
        matches
    }

    /// Paint hits on the visible history rows; the focused one stands out.
    pub(super) fn paint_transcript_search_hits(
        &self,
        buffer: &mut Buffer,
        area: Rect,
        first_line: usize,
    ) {
        let ComposerMode::TranscriptSearch(search) = self.input_ui.composer() else {
            return;
        };
        let end = first_line.saturating_add(area.height as usize);
        let first = search.matches.partition_point(|hit| hit.line < first_line);
        for (index, hit) in search.matches.iter().enumerate().skip(first) {
            if hit.line >= end {
                break;
            }
            let style = if search.focus == Some(index) {
                Theme::transcript_search_focus()
            } else {
                Theme::search_match(Style::default())
            };
            let y = area.y.saturating_add((hit.line - first_line) as u16);
            for column in hit
                .columns
                .clone()
                .take_while(|column| *column < area.width as usize)
            {
                buffer[(area.x.saturating_add(column as u16), y)].set_style(style);
            }
        }
    }
}

#[cfg(test)]
#[path = "transcript_search_tests.rs"]
mod tests;
