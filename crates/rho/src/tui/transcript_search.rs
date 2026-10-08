//! Incremental find over the rendered transcript, opened with
//! `search_transcript` (Ctrl+F by default).
//!
//! Search matches what the transcript paints: the session header, measured
//! entries, and live rows, case-insensitively within one rendered row.
//! Collapsed tool output and text hidden by display settings are not searched.
//! Key handlers only edit the query or request a step; every frame while
//! search is open brings matches up to date with the painted document, so
//! streaming text and live tool rows stay current, then applies the step.
//! Live rows are rescanned every frame; measured rows only when they change.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

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

/// Measuring time per frame while search indexes a lazily measured resume:
/// one 60 Hz frame, so keys typed meanwhile still land within a frame or so.
/// One entry can overrun it; the slowest measured on a real 957-entry resume
/// took 14 ms (release).
pub(super) const INDEX_SLICE: Duration = Duration::from_millis(16);

/// One hit: an absolute history row and the display columns it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TranscriptMatch {
    pub(super) line: usize,
    pub(super) columns: Range<usize>,
}

/// The measured document that stored matches were collected against. While
/// it is unchanged only live rows can differ, so spinner frames during a turn
/// skip the whole-transcript scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SearchedRows {
    width: usize,
    /// [`HistoryLineCache`](super::history_cache::HistoryLineCache) revision.
    revision: u64,
    /// Session header and measured rows; live rows start here.
    static_len: usize,
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
    searched: Option<SearchedRows>,
    pending: Option<SearchStep>,
    /// First measured transcript entry that the row numbers above count
    /// from. Measuring older entries inserts rows above it, so every stored
    /// row moves down by their height.
    measured_from: usize,
    /// Older entries are still being measured, so more hits may appear.
    indexing: bool,
}

impl TranscriptSearch {
    fn new(origin: HistoryScroll, anchor_line: usize, measured_from: usize) -> Self {
        Self {
            editor: LineEditor::new(""),
            origin,
            anchor_line,
            matches: Vec::new(),
            focus: None,
            searched: None,
            pending: None,
            measured_from,
            indexing: measured_from > 0,
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

    /// Move every stored row down by `rows` inserted above them, then add the
    /// hits found in those rows.
    fn shift_down(&mut self, rows: usize, hits_above: Vec<TranscriptMatch>) {
        self.anchor_line = self.anchor_line.saturating_add(rows);
        if let HistoryScroll::Manual { top_line } = &mut self.origin {
            *top_line = top_line.saturating_add(rows);
        }
        if let Some(searched) = &mut self.searched {
            searched.static_len = searched.static_len.saturating_add(rows);
        }
        for hit in &mut self.matches {
            hit.line = hit.line.saturating_add(rows);
        }
        let added = hits_above.len();
        self.matches.splice(0..0, hits_above);
        match self.focus {
            Some(index) => self.focus = Some(index + added),
            // Older rows held the first hits for this query: focus one.
            None if added > 0 => {
                self.pending.get_or_insert(SearchStep::Query);
            }
            None => {}
        }
    }
}

pub(super) fn transcript_search_frame(search: &TranscriptSearch, width: usize) -> ComposerFrame {
    let position = search.position_label();
    let indexing = if search.indexing { "indexing" } else { "" };
    let prompt = join_footer_parts(
        ["find", position.as_deref().unwrap_or_default(), indexing]
            .into_iter()
            .chain(["↑↓ match", "Enter keep", "Esc cancel"]),
    );
    editor_frame(
        &prompt,
        search.editor.viewport(EditorPresentation::Plain, width),
        width,
    )
}

/// A lowercased query, as characters for the column walk and as text for
/// the substring check.
pub(super) struct Needle {
    chars: Vec<char>,
    text: String,
}

pub(super) fn needle(query: &str) -> Needle {
    let chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let text = chars.iter().collect();
    Needle { chars, text }
}

/// Scratch space reused across rows so a whole-transcript scan does not
/// allocate per row.
#[derive(Default)]
struct ScanBuffers {
    /// The row lowercased per character, as the walk sees it.
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
    // Lowercase the row the same way the column walk below does, so a plain
    // substring check rejects most rows without walking graphemes.
    let text = &mut buffers.text;
    text.clear();
    let mut printable_ascii = true;
    for span in &line.spans {
        if is_code_block_copy_span(span) {
            text.push('\0');
            printable_ascii = false;
            continue;
        }
        for ch in span.content.chars() {
            if ch.is_control() {
                printable_ascii = false;
            } else if ch.is_ascii() {
                text.push(ch.to_ascii_lowercase());
            } else {
                printable_ascii = false;
                text.extend(ch.to_lowercase());
            }
        }
    }
    if !text.contains(needle.text.as_str()) {
        return;
    }
    if printable_ascii {
        // Printable ASCII paints one column per byte, so byte offsets are the
        // columns the walk would report.
        for (start, matched) in text.match_indices(needle.text.as_str()) {
            hit(start..start + matched.len());
        }
        return;
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
    /// Open the find prompt without measuring anything: on a lazily measured
    /// resume, older rows are measured over the next frames instead, see
    /// [`Self::search_frame_context`].
    pub(super) fn open_transcript_search<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<(), B::Error> {
        let size = terminal.size()?;
        let ctx = self.frame_context(Rect::new(0, 0, size.width, size.height));
        let height = ctx.layout.history_content.height as usize;
        let anchor_line = self
            .visible_history_start(ctx.history_len, height)
            .saturating_add(height);
        let search = TranscriptSearch::new(
            self.history.scroll(),
            anchor_line,
            self.history.measured_from(),
        );
        self.input_ui
            .set_composer(ComposerMode::TranscriptSearch(search));
        self.set_status_quiet("search transcript");
        Ok(())
    }

    /// Search is open and older transcript rows are still unmeasured. The
    /// event loops keep painting frames until this clears.
    pub(super) fn transcript_search_indexing(&self) -> bool {
        matches!(self.input_ui.composer(), ComposerMode::TranscriptSearch(_))
            && self.history.has_unmeasured_prefix()
    }

    /// Frame context for one paint. While search is open this first measures
    /// up to [`INDEX_SLICE`] of older rows, then brings matches up to date.
    pub(super) fn search_frame_context(&mut self, area: Rect) -> FrameContext {
        let mut ctx = self.frame_context(area);
        if !matches!(self.input_ui.composer(), ComposerMode::TranscriptSearch(_)) {
            return ctx;
        }
        if self.history.has_unmeasured_prefix() {
            let deadline = Instant::now() + INDEX_SLICE;
            self.measure_history_prefix_until(&ctx.layout, ctx.settings, deadline);
            ctx = self.frame_context(area);
        }
        let shifted = self.follow_measured_prefix(&ctx);
        if self.sync_transcript_search(&ctx) || shifted {
            ctx = self.frame_context(area);
        }
        ctx
    }

    /// Keep stored rows on the same text after older entries were measured,
    /// whether by indexing or by scrolling up, and scan only the new rows.
    /// Returns true when the search state changed.
    fn follow_measured_prefix(&mut self, ctx: &FrameContext) -> bool {
        let measured_from = self.history.measured_from();
        let ComposerMode::TranscriptSearch(search) = self.input_ui.composer_mut() else {
            return false;
        };
        search.indexing = measured_from > 0;
        let previous = std::mem::replace(&mut search.measured_from, measured_from);
        if measured_from >= previous {
            return false;
        }
        let needle = needle(&search.editor.value);
        let entry_rows = self
            .history
            .lines_mut()
            .entry_line_range(previous)
            .map_or(0, |rows| rows.start);
        let rows = entry_rows.saturating_add(self.visible_session_header_len(ctx.width));
        let hits = self.collect_transcript_matches(ctx, &needle, rows);
        if let ComposerMode::TranscriptSearch(search) = self.input_ui.composer_mut() {
            search.shift_down(rows, hits);
        }
        true
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

    /// Bring matches up to date with this frame's document, keep the focus on
    /// the same hit, and apply a pending step. Returns true when the matches
    /// or scroll position changed, so the caller must rebuild the frame
    /// context before painting.
    ///
    /// Measured rows are rescanned only when they changed (see
    /// [`SearchedRows`]), such as a streaming line growing in place. Live rows
    /// are few and rescanned every frame.
    fn sync_transcript_search(&mut self, ctx: &FrameContext) -> bool {
        let live = &ctx.live_history.lines;
        let rows = SearchedRows {
            width: ctx.width,
            revision: self.history.rows_revision(),
            static_len: ctx.history_len.saturating_sub(live.len()),
        };
        let ComposerMode::TranscriptSearch(search) = self.input_ui.composer() else {
            return false;
        };
        let needle = needle(&search.editor.value);
        let matches = if search.pending.is_none() && search.searched == Some(rows) {
            let kept = search
                .matches
                .partition_point(|hit| hit.line < rows.static_len);
            let mut matches = search.matches[..kept].to_vec();
            let mut buffers = ScanBuffers::default();
            collect_matches(&mut matches, &mut buffers, live, rows.static_len, &needle);
            matches
        } else {
            self.collect_transcript_matches(ctx, &needle, usize::MAX)
        };
        let ComposerMode::TranscriptSearch(search) = self.input_ui.composer_mut() else {
            return false;
        };
        search.searched = Some(rows);
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

    /// Scroll so `line` is on screen with some context above it, or keep the
    /// viewport when the row is already visible. Either way hold that
    /// position, even on the last screenful, so appended rows do not scroll
    /// the focused hit away. Paging and the jump binding still move it.
    fn reveal_history_line(&mut self, ctx: &FrameContext, line: usize) {
        let height = ctx.layout.history_content.height as usize;
        let start = self.visible_history_start(ctx.history_len, height);
        let top_line = if (start..start.saturating_add(height)).contains(&line) {
            start
        } else {
            line.saturating_sub(height / 3)
        };
        self.history
            .scroll_chrome_mut()
            .hold_top_line(ctx.history_len, height, top_line);
    }

    /// Hits in the first `limit` rows of the session header, measured
    /// transcript rows, and live rows, numbered like the painted document.
    fn collect_transcript_matches(
        &mut self,
        ctx: &FrameContext,
        needle: &Needle,
        limit: usize,
    ) -> Vec<TranscriptMatch> {
        let mut matches = Vec::new();
        if needle.chars.is_empty() {
            return matches;
        }
        let mut buffers = ScanBuffers::default();
        let header_len = self.visible_session_header_len(ctx.width).min(limit);
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
                let rows = rows.take(limit.saturating_sub(line));
                collect_matches(&mut matches, &mut buffers, rows, line, needle)
            });
        let live = ctx.live_history.lines.iter();
        let live = live.take(limit.saturating_sub(line));
        collect_matches(&mut matches, &mut buffers, live, line, needle);
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
