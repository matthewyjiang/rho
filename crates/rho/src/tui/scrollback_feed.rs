//! Minimal mode: the transcript scrolls into the terminal's own scrollback.
//!
//! Frames still fill the main screen, with history pinned to the bottom of
//! its pane. Before a frame scrolls committed rows off the top of the pane,
//! [`App::paint`] writes them into scrollback, oldest first, and repaints the
//! screen. From then on the terminal owns those rows: native wheel
//! scrolling, selection, and search reach them, and Rho never redraws them.
//! Live rows (stream previews, running tools) are never written. A committed
//! entry that changes later, such as a reasoning entry gaining its "Thought
//! for" line, changes only the rows still on screen.
//!
//! The feed remembers how far it wrote as a header or entry line rather than
//! a row number, so re-rendering an entry or changing width keeps its place.
//! The pane never starts above that mark, so rows already in scrollback do
//! not show twice. Wherever the feed cannot tell what the terminal holds, it
//! repeats rows rather than risk losing any.

use std::io;

use ratatui::{
    buffer::Buffer,
    layout::{Rect, Size},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::{
    history_cache::{HistoryLineCache, HistoryRenderSettings},
    screen_layout::terminal_meets_minimum,
    synced_backend::repaint_every_row,
    App, DefaultTerminal, Entry,
};

/// How much of the history document is in scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeedMark {
    /// The first `n` header rows; past the header's end means all of it.
    Header(usize),
    /// The header, every entry before `index`, and the first `line` rows of
    /// entry `index`. Past the entry's end means all of it, so rows the entry
    /// grows by land after the mark.
    Entry { index: usize, line: usize },
}

/// Minimal mode's link between the history pane and native scrollback.
#[derive(Debug)]
pub(super) struct ScrollbackFeed {
    mark: FeedMark,
    /// `mark` as a document row at the last paint. The pane starts no higher.
    mark_row: usize,
    /// Unwritten rows of a transcript replaced since the last paint, for it
    /// to write. `Some` until then, so a second replacement before it does
    /// not write the new header early.
    retired: Option<Vec<Line<'static>>>,
    /// `None` before the first paint.
    last_paint: Option<LastPaint>,
    /// Whether a surface had the keyboard at the last paint.
    surface_had_keyboard: bool,
}

#[derive(Clone, Copy, Debug)]
struct LastPaint {
    size: Size,
    /// To render rows between paints.
    settings: HistoryRenderSettings,
}

/// What a resize does to the feed.
#[derive(Debug, PartialEq, Eq)]
struct ResizeRewind {
    /// The mark's row from now on.
    mark_row: usize,
    /// Rows at the top of the screen to push back into scrollback first.
    foreign_rows: u16,
}

impl ScrollbackFeed {
    pub(super) fn new() -> Self {
        Self {
            mark: FeedMark::Header(0),
            mark_row: 0,
            retired: None,
            last_paint: None,
            surface_had_keyboard: false,
        }
    }

    /// `start`, the pane's first row of `history_len`, moved past rows the
    /// feed already wrote, so none shows twice.
    pub(super) fn clamp_start(&self, start: usize, history_len: usize) -> usize {
        start.max(self.mark_row.min(history_len))
    }

    /// How the feed follows the screen going from `previous` to `size`.
    ///
    /// Terminals that reflow on resize pull scrollback rows back onto a
    /// taller screen, where the next frame would paint over them. The mark
    /// steps back over the document rows among them, so those show again and
    /// later scroll out as usual. Rows pulled back from before the document,
    /// the shell's or a replaced transcript's, go back into scrollback. A
    /// terminal that pulls nothing back repeats rows in scrollback instead: a
    /// repeated row is better than a lost one. A shorter screen, or a new
    /// width, keeps the mark, for the same reason.
    fn rewind_after_resize(mark_row: usize, previous: Size, size: Size) -> ResizeRewind {
        if previous.width != size.width || size.height <= previous.height {
            return ResizeRewind {
                mark_row,
                foreign_rows: 0,
            };
        }
        let grown = size.height - previous.height;
        let rewound = mark_row.min(usize::from(grown));
        ResizeRewind {
            mark_row: mark_row - rewound,
            // `rewound` is at most `grown`, a `u16`.
            foreign_rows: grown - rewound as u16,
        }
    }

    /// The mark as a document row. Valid while `lines` is current.
    fn mark_row(&self, lines: &HistoryLineCache, document: FeedDocument) -> usize {
        match self.mark {
            FeedMark::Header(line) => line.min(document.header_len),
            // An entry the cache does not hold is past the measured end.
            FeedMark::Entry { index, line } => lines
                .entry_line_range(index)
                .map_or(document.committed_len, |range| {
                    document.header_len + range.start + line.min(range.len())
                }),
        }
    }

    /// Moves the mark to document row `row`, at most the committed length,
    /// naming the entry of the last row written so that entry's growth is
    /// written too. `entries` is the transcript's length.
    fn set_mark(
        &mut self,
        row: usize,
        lines: &HistoryLineCache,
        entries: usize,
        document: FeedDocument,
    ) {
        self.mark = if row <= document.header_len {
            FeedMark::Header(row)
        } else {
            let last = row - document.header_len - 1;
            match lines.entry_at_cached_line(last) {
                Some((index, start)) => FeedMark::Entry {
                    index,
                    line: last + 1 - start,
                },
                None => {
                    debug_assert!(false, "committed row {row} is not in the line cache");
                    // All of it: repeating nothing beats a mark that rewinds.
                    FeedMark::Entry {
                        index: entries.saturating_sub(1),
                        line: usize::MAX,
                    }
                }
            }
        };
        self.mark_row = row;
    }
}

/// Lengths of the parts of the history document Rho can write to scrollback.
#[derive(Clone, Copy, Debug)]
struct FeedDocument {
    header_len: usize,
    /// Header and transcript rows; live rows follow.
    committed_len: usize,
}

/// The document and the mark's row in it, from [`App::settle_feed_mark`].
struct SettledFeed {
    document: FeedDocument,
    mark_row: usize,
    /// Whether entries were measured, which changes the frame's history.
    measured: bool,
}

impl App {
    /// Whether the terminal's own scrollback holds the transcript (minimal
    /// mode), which leaves scrolling and search to the terminal.
    pub(super) fn terminal_owns_scrollback(&self) -> bool {
        self.scrollback.is_some()
    }

    /// Writes to scrollback any committed rows the next frame would scroll
    /// off the top of the history pane. See [`App::paint`].
    pub(super) fn feed_scrollback(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        if !terminal_meets_minimum(area) {
            return Ok(());
        }
        // Rows above the mark are in scrollback; in-app scrolling is off.
        self.scroll_history_to_bottom();
        let mut ctx = self.frame_context(area);
        let settings = ctx.settings;
        let settled = self.settle_feed_mark(settings);
        if settled.measured {
            ctx = self.frame_context(area);
        }
        let document = settled.document;
        let feed = self.feed_mut();
        let resized = feed
            .last_paint
            .map(|last| last.size)
            .filter(|previous| *previous != size);
        feed.last_paint = Some(LastPaint { size, settings });
        let mut rows = feed.retired.take().unwrap_or_default();
        let rewind = match resized {
            Some(previous) => ScrollbackFeed::rewind_after_resize(settled.mark_row, previous, size),
            None => ResizeRewind {
                mark_row: settled.mark_row,
                foreign_rows: 0,
            },
        };
        let pane_height = usize::from(ctx.layout.history_content.height);
        let pane_start = ctx.history_len.saturating_sub(pane_height);
        let end = pane_start.min(document.committed_len).max(rewind.mark_row);
        rows.extend(self.feed_rows(settings, rewind.mark_row..end));
        self.set_feed_mark(end, document);
        if rows.is_empty() && rewind.foreign_rows == 0 {
            return Ok(());
        }
        terminal
            .backend_mut()
            .push_screen_top(rewind.foreign_rows)?;
        write_to_scrollback(terminal, &rows, size)?;
        repaint_every_row(terminal)
    }

    /// Writes the rows still on screen as plain lines and leaves the cursor
    /// below them, so the transcript stays where the session ended. Called
    /// once, as the session view closes.
    pub(super) fn release_main_screen(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let Some(feed) = self.scrollback.as_mut() else {
            return Ok(());
        };
        let mut rows = feed.retired.take().unwrap_or_default();
        let last_settings = feed.last_paint.map(|last| last.settings);
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        // Too small a screen lays out no frame; the last paint's settings
        // still render the rows.
        let settings = if terminal_meets_minimum(area) {
            Some(self.frame_context(area).settings)
        } else {
            last_settings
        };
        if let Some(settings) = settings {
            rows.extend(self.take_unwritten_rows(settings));
        }
        let kept = rows.len().min(usize::from(size.height.saturating_sub(1)));
        let (head, tail) = rows.split_at(rows.len() - kept);
        write_to_scrollback(terminal, head, size)?;
        terminal
            .backend_mut()
            .finish_main_screen(&rows_buffer(tail, size.width))
    }

    /// Keeps the rows of the transcript about to be replaced that are not in
    /// scrollback yet, for the next paint to write, and starts the feed over
    /// at the new transcript's header. Call before replacing the transcript.
    pub(super) fn retire_scrollback_document(&mut self) {
        let Some(settings) = self
            .scrollback
            .as_ref()
            .filter(|feed| feed.retired.is_none())
            .and_then(|feed| feed.last_paint)
            .map(|last| last.settings)
        else {
            return;
        };
        let rows = self.take_unwritten_rows(settings);
        let feed = self.feed_mut();
        feed.retired = Some(rows);
        feed.mark = FeedMark::Header(0);
        feed.mark_row = 0;
    }

    /// Every committed row after the mark, which moves to the document's end.
    fn take_unwritten_rows(&mut self, settings: HistoryRenderSettings) -> Vec<Line<'static>> {
        let settled = self.settle_feed_mark(settings);
        let document = settled.document;
        let rows = self.feed_rows(settings, settled.mark_row..document.committed_len);
        self.set_feed_mark(document.committed_len, document);
        rows
    }

    /// Brings the mark up to date with the transcript: steps it back before
    /// removed entries, whose indices no longer mean what it wrote, and to
    /// the start of a surface the keyboard just left, which may have changed
    /// in rows already written. Then measures every entry from the mark on,
    /// so its rows have numbers.
    fn settle_feed_mark(&mut self, settings: HistoryRenderSettings) -> SettledFeed {
        let has_keyboard = self.surfaces.has_keyboard();
        let feed = self.feed_mut();
        let left_surface =
            std::mem::replace(&mut feed.surface_had_keyboard, has_keyboard) && !has_keyboard;
        if let FeedMark::Entry { index, .. } = feed.mark {
            if left_surface && matches!(self.history.entries().get(index), Some(Entry::Surface(_)))
            {
                self.feed_mut().mark = FeedMark::Entry { index, line: 0 };
            }
        }
        if let Some(removed) = self.history.take_removed_from() {
            let feed = self.feed_mut();
            if let FeedMark::Entry { index, .. } = feed.mark {
                if index >= removed {
                    // What was written of the removed entries stays in
                    // scrollback; their replacements are written after it.
                    feed.mark = match removed.checked_sub(1) {
                        Some(kept) => FeedMark::Entry {
                            index: kept,
                            line: usize::MAX,
                        },
                        None => FeedMark::Header(usize::MAX),
                    };
                }
            }
        }
        let first = match self.feed_mut().mark {
            FeedMark::Entry { index, .. } => index,
            FeedMark::Header(_) => 0,
        };
        let measured = self.history.measured_from() > first;
        if measured {
            // In one pass: each prepend rebuilds the cache's ranges.
            self.grow_measured_history_prefix(settings, /*extra_lines*/ usize::MAX);
        }
        let header_len = self.visible_session_header_len(settings.width);
        let document = FeedDocument {
            header_len,
            committed_len: header_len.saturating_add(self.cached_transcript_line_count(settings)),
        };
        let mark_row = self
            .scrollback
            .as_ref()
            .expect("minimal mode has a feed")
            .mark_row(self.history.lines(), document);
        SettledFeed {
            document,
            mark_row,
            measured,
        }
    }

    fn set_feed_mark(&mut self, row: usize, document: FeedDocument) {
        let feed = self.scrollback.as_mut().expect("minimal mode has a feed");
        feed.set_mark(row, self.history.lines(), self.history.len(), document);
    }

    fn feed_rows(
        &mut self,
        settings: HistoryRenderSettings,
        rows: std::ops::Range<usize>,
    ) -> Vec<Line<'static>> {
        self.visible_history_lines_with_live(settings.width, settings, rows.start, rows.len(), &[])
    }

    fn feed_mut(&mut self) -> &mut ScrollbackFeed {
        self.scrollback.as_mut().expect("minimal mode has a feed")
    }
}

/// Writes `rows` to scrollback a screen at a time.
fn write_to_scrollback(
    terminal: &mut DefaultTerminal,
    rows: &[Line<'static>],
    size: Size,
) -> io::Result<()> {
    for slice in rows.chunks(usize::from(size.height.max(1))) {
        terminal
            .backend_mut()
            .push_to_scrollback(&rows_buffer(slice, size.width))?;
    }
    Ok(())
}

/// `rows`, at most a screen of them, painted as the history pane paints
/// them, one buffer row each.
fn rows_buffer(rows: &[Line<'static>], width: u16) -> Buffer {
    let height = u16::try_from(rows.len()).expect("at most a screen of rows");
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    Paragraph::new(rows.to_vec()).render(area, &mut buffer);
    buffer
}

#[cfg(test)]
#[path = "scrollback_feed_tests.rs"]
mod tests;
