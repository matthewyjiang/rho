//! Frames on the main screen, for minimal mode: the rows that keep the
//! terminal's own scrollback in step with the session.
//!
//! Every clear here erases row by row. On the main screen an erase-display
//! can push the rows into scrollback first (tmux `scroll-on-clear`),
//! duplicating rows Rho already placed there; erase-line never does.

use std::{io, ops::Range};

use crossterm::{
    cursor::{MoveTo, Show},
    queue,
    style::Print,
    terminal::{self, BeginSynchronizedUpdate},
};
use ratatui::{
    backend::Backend,
    buffer::{Buffer, Cell, CellDiffOption, CellWidth},
    layout::Position,
    style::{Color, Modifier},
};

use super::{
    repaint_every_row, write_covered_cells_first, DefaultTerminal, SyncedBackend, TerminalScreen,
};

/// Takes the main screen for session frames: scrolls the rows the shell, or a
/// program the session handed the terminal to, printed into scrollback, and
/// makes the next draw paint every row. Call only while no event reader
/// takes stdin, since the cursor query's reply is read from it directly.
pub(crate) fn claim_main_screen(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let cursor = terminal.get_cursor_position().ok();
    terminal.backend_mut().scroll_used_rows(cursor)?;
    repaint_every_row(terminal)
}

/// Queues an erase of each screen row in `rows`.
pub(crate) fn queue_erase_rows(writer: &mut impl io::Write, rows: Range<u16>) -> io::Result<()> {
    for row in rows {
        queue!(
            writer,
            MoveTo(0, row),
            terminal::Clear(terminal::ClearType::CurrentLine)
        )?;
    }
    Ok(())
}

/// Whether `cell` paints nothing a reader could see past the end of a row:
/// a space on the terminal's own background.
fn is_blank_tail(cell: &Cell) -> bool {
    cell.symbol() == " "
        && matches!(cell.bg, Color::Reset)
        && !cell
            .modifier
            .intersects(Modifier::REVERSED | Modifier::UNDERLINED | Modifier::CROSSED_OUT)
}

/// Cells of `buffer` row `row`, painted on screen row `screen_row`, without
/// trailing blanks. Scrollback keeps what is written; trailing blanks would
/// wrap into empty rows when a reflowing terminal narrows.
fn row_cells(buffer: &Buffer, row: u16, screen_row: u16) -> Vec<(u16, u16, &Cell)> {
    let area = buffer.area;
    let mut cells = Vec::new();
    // Like ratatui's diff, skip the cells a wide glyph covers.
    let mut covered = 0;
    for x in area.left()..area.right() {
        let cell = &buffer[(x, row)];
        if covered > 0 {
            covered -= 1;
            continue;
        }
        if cell.diff_option != CellDiffOption::Skip {
            cells.push((x - area.left(), screen_row, cell));
        }
        covered = cell.cell_width().saturating_sub(1);
    }
    while cells.last().is_some_and(|(_, _, cell)| is_blank_tail(cell)) {
        cells.pop();
    }
    write_covered_cells_first(&mut cells);
    cells
}

impl SyncedBackend {
    /// Erases every visible row; see the module docs.
    pub(super) fn clear_rows(&mut self) -> io::Result<()> {
        let height = self.size()?.height;
        queue_erase_rows(&mut self.inner, 0..height)
    }

    /// Paints every row of `buffer` on the screen rows from the top.
    fn paint_rows(&mut self, buffer: &Buffer) -> io::Result<()> {
        for row in 0..buffer.area.height {
            queue_erase_rows(&mut self.inner, row..row + 1)?;
            let cells = row_cells(buffer, buffer.area.top() + row, row);
            self.inner.draw(cells.into_iter())?;
        }
        Ok(())
    }

    /// Scrolls the top `rows` screen rows into scrollback with line feeds on
    /// the bottom row, which every terminal keeps as history.
    fn scroll_up(&mut self, rows: u16) -> io::Result<()> {
        if rows == 0 {
            return Ok(());
        }
        let height = self.size()?.height;
        queue!(self.inner, MoveTo(0, height.saturating_sub(1)))?;
        for _ in 0..rows {
            queue!(self.inner, Print("\n"))?;
        }
        Ok(())
    }

    /// Writes every row of `rows`, which fit on the screen, into the
    /// terminal's scrollback below whatever scrolled there before.
    ///
    /// Leaves the visible rows undefined: the caller repaints every row
    /// ([`repaint_every_row`]) and draws a full frame in the same update.
    pub(crate) fn push_to_scrollback(&mut self, rows: &Buffer) -> io::Result<()> {
        debug_assert_eq!(self.screen, TerminalScreen::Main);
        debug_assert!(rows.area.height <= self.size()?.height);
        queue!(self.inner, BeginSynchronizedUpdate)?;
        self.paint_rows(rows)?;
        self.scroll_up(rows.area.height)
    }

    /// Scrolls the rows above `cursor` into scrollback, so frames can take
    /// the whole main screen without painting over them. `None`, when the
    /// terminal did not say where the cursor is, treats every row as used.
    fn scroll_used_rows(&mut self, cursor: Option<Position>) -> io::Result<()> {
        let height = self.size()?.height;
        let used = cursor.map_or(height, |cursor| {
            cursor.y.saturating_add(u16::from(cursor.x > 0))
        });
        self.push_screen_top(used.min(height))
    }

    /// Scrolls the top `rows` visible rows into scrollback as they are. The
    /// caller repaints every row in the same update.
    pub(crate) fn push_screen_top(&mut self, rows: u16) -> io::Result<()> {
        debug_assert_eq!(self.screen, TerminalScreen::Main);
        queue!(self.inner, BeginSynchronizedUpdate)?;
        self.scroll_up(rows)
    }

    /// Leaves `tail` as the last lines on the main screen, with the cursor
    /// on the row below them for whatever the shell prints next. Replaces
    /// every visible row; `tail` must leave that row free.
    pub(crate) fn finish_main_screen(&mut self, tail: &Buffer) -> io::Result<()> {
        debug_assert_eq!(self.screen, TerminalScreen::Main);
        let height = self.size()?.height;
        queue!(self.inner, BeginSynchronizedUpdate)?;
        self.paint_rows(tail)?;
        queue_erase_rows(&mut self.inner, tail.area.height..height)?;
        queue!(self.inner, MoveTo(0, tail.area.height), Show)?;
        Backend::flush(self)
    }
}
