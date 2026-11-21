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
    cursor::Show,
    execute, queue,
    terminal::{
        self, disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
        EnterAlternateScreen,
    },
};
use ratatui::{
    backend::{Backend, ClearType, CrosstermBackend, WindowSize},
    buffer::{Cell, CellWidth},
    layout::{Position, Rect, Size},
    Terminal,
};

mod main_screen;

pub(crate) use main_screen::{claim_main_screen, queue_erase_rows};

/// Rho's interactive terminal. Every TUI surface draws through this type so no
/// frame bypasses synchronized output; `scripts/architecture.json` forbids
/// ratatui's unsynchronized `init`, `try_init`, and `DefaultTerminal`.
pub(crate) type DefaultTerminal = Terminal<SyncedBackend>;

/// The terminal buffer frames draw on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalScreen {
    /// The alternate screen: frames own the whole terminal and leave the
    /// user's scrollback untouched.
    Alternate,
    /// The main screen: frames still fill the visible rows, and rows written
    /// with [`SyncedBackend::push_to_scrollback`] stay in the terminal's own
    /// scrollback, where native scrolling, selection, and search reach them.
    Main,
}

/// `ratatui::try_init` for [`SyncedBackend`]: install a panic hook that
/// restores the terminal, enter raw mode, and enter `screen`.
pub(crate) fn try_init(screen: TerminalScreen) -> io::Result<DefaultTerminal> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(screen);
        hook(info);
    }));
    enable_raw_mode()?;
    match screen {
        TerminalScreen::Alternate => execute!(io::stdout(), EnterAlternateScreen)?,
        TerminalScreen::Main => {}
    }
    Terminal::new(SyncedBackend {
        inner: CrosstermBackend::new(io::stdout()),
        screen,
    })
}

/// Panicking [`try_init`], mirroring `ratatui::init`. The panic runs the hook,
/// so a half-initialized terminal is still restored.
pub(crate) fn init(screen: TerminalScreen) -> DefaultTerminal {
    try_init(screen).expect("failed to initialize terminal")
}

/// `ratatui::restore` for `screen`: leave raw mode and give the terminal back.
///
/// The main screen is never left: leaving an alternate screen that was never
/// entered restores a cursor xterm saved at some earlier entry.
pub(crate) fn restore(screen: TerminalScreen) {
    match screen {
        TerminalScreen::Alternate => ratatui::restore(),
        TerminalScreen::Main => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), EndSynchronizedUpdate, Show);
        }
    }
}

/// Forces the next draw to repaint every cell, as after a terminal resize event.
///
/// Ratatui only clears when the size differs from its last frame. A shrink
/// then restore can coalesce into events that all report the old size, yet
/// the terminal truncated cells on the shrink; a diff-only redraw then leaves
/// them blank. Call this from each surface's `Event::Resize` handler.
///
/// Also clears for any other full repaint: unlike `Terminal::clear`, this
/// never queries the cursor, whose reply the session's event reader would
/// swallow.
pub(crate) fn repaint_every_row(terminal: &mut DefaultTerminal) -> io::Result<()> {
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
pub(crate) struct SyncedBackend {
    inner: CrosstermBackend<Stdout>,
    screen: TerminalScreen,
}

/// Size of the terminal this backend draws to, read from stdout with one ioctl.
///
/// crossterm's `terminal::size` opens and closes `/dev/tty` on every call, and
/// the TUI asks for the size on every frame (ratatui's autoresize) and every
/// streamed delta. `None` (stdout is not a terminal, or reports no size) falls
/// back to crossterm, which also tries `/dev/tty` and `tput`.
#[cfg(unix)]
fn stdout_window_size() -> Option<Size> {
    use std::os::fd::AsRawFd as _;
    let mut winsize = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes into a valid, properly sized `winsize`.
    let status = unsafe {
        libc::ioctl(
            io::stdout().as_raw_fd(),
            libc::TIOCGWINSZ,
            &mut winsize as *mut libc::winsize,
        )
    };
    (status == 0 && winsize.ws_col > 0 && winsize.ws_row > 0).then_some(Size {
        width: winsize.ws_col,
        height: winsize.ws_row,
    })
}

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
        queue!(self.inner, BeginSynchronizedUpdate)?;
        let mut cells: Vec<_> = content.collect();
        write_covered_cells_first(&mut cells);
        self.inner.draw(cells.into_iter())
    }

    fn flush(&mut self) -> io::Result<()> {
        queue!(self.inner, EndSynchronizedUpdate)?;
        Backend::flush(&mut self.inner)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    /// Queues the clear inside the next frame's synchronized update instead of
    /// flushing it. Ratatui clears before redrawing a resized frame, and a
    /// flushed clear shows a blank screen until that frame arrives.
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        queue!(self.inner, BeginSynchronizedUpdate)?;
        let clear_type = match (clear_type, self.screen) {
            (ClearType::All, TerminalScreen::Main) => return self.clear_rows(),
            (ClearType::All, TerminalScreen::Alternate) => terminal::ClearType::All,
            (ClearType::AfterCursor, _) => terminal::ClearType::FromCursorDown,
            (ClearType::BeforeCursor, _) => terminal::ClearType::FromCursorUp,
            (ClearType::CurrentLine, _) => terminal::ClearType::CurrentLine,
            (ClearType::UntilNewLine, _) => terminal::ClearType::UntilNewLine,
        };
        queue!(self.inner, terminal::Clear(clear_type))
    }

    fn size(&self) -> io::Result<Size> {
        #[cfg(unix)]
        if let Some(size) = stdout_window_size() {
            return Ok(size);
        }
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }
}
