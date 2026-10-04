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
        enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen,
    },
};
use ratatui::{
    backend::{Backend, ClearType, CrosstermBackend, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
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

/// Ratatui's crossterm backend with each frame sent as one synchronized
/// update. `Terminal::draw` calls `draw` once, positions the cursor, then calls
/// `flush`, so begin-in-`draw` and end-in-`flush` cover cells and caret
/// together. Both markers are idempotent, so no open-update state is tracked.
pub(crate) struct SyncedBackend(CrosstermBackend<Stdout>);

impl Backend for SyncedBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        queue!(self.0, BeginSynchronizedUpdate)?;
        self.0.draw(content)
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
        self.0.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.0.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.0.window_size()
    }
}
