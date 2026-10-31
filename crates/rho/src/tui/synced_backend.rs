//! Crossterm backend that presents each frame atomically.
//!
//! Ratatui paints a frame as cell writes, then shows and moves the hardware
//! cursor, each step flushing separately. Terminals may render between those
//! writes, so the caret visibly jumps across painted cells before settling.
//! Wrapping the frame in synchronized output (DEC private mode 2026) makes
//! supporting terminals hold presentation until the frame is complete; other
//! terminals ignore the sequence.

use std::io::{self, Stdout};

use crossterm::{
    execute, queue,
    terminal::{
        self, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen,
    },
};
use ratatui::{
    backend::{Backend, ClearType, CrosstermBackend, WindowSize},
    buffer::{Cell, CellWidth},
    layout::{Position, Rect, Size},
    Terminal,
};

/// Rho's interactive terminal. Every TUI surface draws through this type so no
/// frame bypasses synchronized output; `scripts/architecture.json` forbids
/// ratatui's unsynchronized `init`, `try_init`, and `DefaultTerminal`.
pub(crate) type DefaultTerminal = Terminal<SyncedBackend>;

/// `ratatui::try_init` for [`SyncedBackend`]: install a panic hook that
/// restores the terminal, enter raw mode and the alternate screen.
pub(crate) fn try_init() -> io::Result<DefaultTerminal> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        hook(info);
    }));
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    Terminal::new(SyncedBackend(CrosstermBackend::new(io::stdout())))
}

/// Panicking [`try_init`], mirroring `ratatui::init`. The panic runs the hook,
/// so a half-initialized terminal is still restored.
pub(crate) fn init() -> DefaultTerminal {
    try_init().expect("failed to initialize terminal")
}

/// Forces the next draw to repaint every cell after a terminal resize event.
///
/// Ratatui only clears when the size differs from its last frame. A shrink
/// then restore can coalesce into events that all report the old size, yet
/// the terminal truncated cells on the shrink; a diff-only redraw then leaves
/// them blank. Call this from each surface's `Event::Resize` handler.
pub(crate) fn repaint_after_resize(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let size = terminal.size()?;
    terminal.resize(Rect::new(0, 0, size.width, size.height))
}

/// Ratatui's crossterm backend with each frame sent as one synchronized
/// update. `Terminal::draw` calls `draw` once, positions the cursor, then calls
/// `flush`, so begin-in-`draw` and end-in-`flush` cover cells and caret
/// together. A clear also begins the update and stays queued, so every clear
/// must be followed by a draw; ratatui's resize and autoresize always are.
/// Both markers are idempotent, so no open-update state is tracked. Cells are
/// reordered by [`write_covered_cells_first`] so wide emoji draw intact.
pub(crate) struct SyncedBackend(CrosstermBackend<Stdout>);

/// Moves each wide cell after the cells it covers in the same row.
///
/// ratatui-core writes the trailing cell of a VS16 emoji such as "❤️" right
/// after the emoji, to clear it on terminals that draw the emoji one column
/// wide. Where the emoji is two columns wide, that write lands one column right
/// (ratatui-crossterm 0.1.2 skips the cursor move as if the cursor advanced one
/// column) and shifts the rest of the row. With the cursor move (fixed upstream
/// in ratatui/ratatui#2721), it erases the emoji's right half instead. Writing
/// the covered cells first is correct for both widths, and the backend moves the
/// cursor before each out-of-sequence write. CJK and other wide cells cover
/// nothing in a diff, so they keep their order.
fn write_covered_cells_first(cells: &mut [(u16, u16, &Cell)]) {
    let mut index = 0;
    while index < cells.len() {
        let (x, y, cell) = cells[index];
        let end = x.saturating_add(cell.cell_width());
        let covered = cells[index + 1..]
            .iter()
            .take_while(|(next_x, next_y, _)| *next_y == y && *next_x < end)
            .count();
        cells[index..=index + covered].rotate_left(1);
        index += covered + 1;
    }
}

impl Backend for SyncedBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        queue!(self.0, BeginSynchronizedUpdate)?;
        let mut cells: Vec<_> = content.collect();
        write_covered_cells_first(&mut cells);
        self.0.draw(cells.into_iter())
    }

    fn flush(&mut self) -> io::Result<()> {
        queue!(self.0, EndSynchronizedUpdate)?;
        Backend::flush(&mut self.0)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.0.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.0.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.0.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    /// Queues the clear inside the next frame's synchronized update instead of
    /// flushing it. Ratatui clears before redrawing a resized frame, and a
    /// flushed clear shows a blank screen until that frame arrives.
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        let clear_type = match clear_type {
            ClearType::All => terminal::ClearType::All,
            ClearType::AfterCursor => terminal::ClearType::FromCursorDown,
            ClearType::BeforeCursor => terminal::ClearType::FromCursorUp,
            ClearType::CurrentLine => terminal::ClearType::CurrentLine,
            ClearType::UntilNewLine => terminal::ClearType::UntilNewLine,
        };
        queue!(self.0, BeginSynchronizedUpdate, terminal::Clear(clear_type))
    }

    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.0.window_size()
    }
}
