use std::{future::Future, io};

use crate::tui::DefaultTerminal;
use anyhow::{anyhow, Context};
use crossterm::{
    cursor::{MoveTo, Show},
    event::{DisableFocusChange, EnableFocusChange, Event},
    execute, queue,
    style::Print,
    terminal::{disable_raw_mode, Clear, ClearType, LeaveAlternateScreen},
};

use super::{
    keyboard_modes, mouse_capture, synced_backend::TerminalScreen, terminal_events::TerminalEvents,
};

pub(super) struct SuspendedRun<T> {
    pub(super) operation_result: anyhow::Result<T>,
    pub(super) resume_result: anyhow::Result<()>,
}

pub(super) struct TerminalSession {
    screen: TerminalScreen,
    events: Option<TerminalEvents>,
    keyboard: Option<keyboard_modes::Enabled>,
    mouse_capture_enabled: bool,
    focus_change_enabled: bool,
}

impl TerminalSession {
    /// Takes input on `screen`. The main screen leaves the mouse to the
    /// terminal, so wheel scrolling and drag selection reach its scrollback.
    pub(super) fn acquire(screen: TerminalScreen) -> Self {
        Self {
            screen,
            events: Some(TerminalEvents::new()),
            // Released crossterm replaces the Windows console mode when mouse
            // capture starts. Acquire VT input afterward so it is not cleared.
            mouse_capture_enabled: acquire_mouse_capture(screen),
            keyboard: Some(keyboard_modes::Enabled::acquire()),
            focus_change_enabled: enable_focus_change().is_ok(),
        }
    }

    /// Re-enables mouse capture on the alternate screen, for hosts that drop
    /// it on focus changes. The main screen leaves the mouse to the terminal.
    pub(super) fn reassert_mouse_capture(&self) {
        match self.screen {
            TerminalScreen::Alternate => mouse_capture::reassert(),
            TerminalScreen::Main => {}
        }
    }

    pub(super) async fn next_event(&mut self) -> std::io::Result<Event> {
        self.events
            .as_mut()
            .expect("terminal events active")
            .next()
            .await
    }

    /// Run work with exclusive ownership of the user's terminal.
    ///
    /// `handoff_status` is printed on the main screen after leaving the
    /// alternate buffer so the caller can show what the terminal is waiting on.
    ///
    /// The outer result reports terminal lifecycle failures. The inner result
    /// belongs to the suspended operation and is safe to present in the TUI
    /// because the terminal has resumed before this method returns it.
    pub(super) async fn run_suspended<T, F, Fut>(
        &mut self,
        terminal: &mut DefaultTerminal,
        handoff_status: &str,
        operation: F,
    ) -> SuspendedRun<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        self.stop_events();
        if let Err(suspend_error) = self.suspend(handoff_status) {
            return SuspendedRun {
                operation_result: Err(suspend_error.context("suspended operation was not started")),
                resume_result: self
                    .resume(terminal)
                    .context("failed to recover Rho after terminal suspension failed"),
            };
        }

        let operation_result = operation().await;
        let resume_result = self
            .resume(terminal)
            .context("failed to resume Rho after suspended operation");
        SuspendedRun {
            operation_result,
            resume_result,
        }
    }

    fn stop_events(&mut self) {
        self.events = None;
    }

    fn suspend(&mut self, handoff_status: &str) -> anyhow::Result<()> {
        let mut failures = Vec::new();
        if self.focus_change_enabled {
            if let Err(error) = disable_focus_change() {
                failures.push(format!("disable focus change: {error}"));
            }
            self.focus_change_enabled = false;
        }
        // Release in reverse acquisition order: mouse capture restores the
        // full pre-mouse Windows console mode, including its original VT bit.
        if let Some(keyboard) = self.keyboard.take() {
            if let Err(error) = keyboard.try_release() {
                failures.push(format!("disable keyboard modes: {error}"));
            }
        }
        if self.mouse_capture_enabled {
            if let Err(error) = mouse_capture::disable() {
                failures.push(format!("disable mouse capture: {error}"));
            }
            self.mouse_capture_enabled = false;
        }
        // Clear the screen the session drew on and print the caller status in
        // one flush, so handoff does not flash scrollback.
        if let Err(error) = hand_off_terminal(self.screen, handoff_status) {
            failures.push(format!("restore terminal: {error}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(failures.join("; ")))
        }
    }

    fn resume(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let mut resumed =
            crate::tui::synced_backend::try_init(self.screen).context("initialize terminal")?;
        match self.screen {
            TerminalScreen::Alternate => {}
            // Keep what the handed-off program printed, as leaving the
            // alternate screen would; the event reader is not back yet.
            TerminalScreen::Main => crate::tui::synced_backend::claim_main_screen(&mut resumed)
                .context("claim the main screen")?,
        }
        *terminal = resumed;
        self.mouse_capture_enabled = acquire_mouse_capture(self.screen);
        self.keyboard = Some(keyboard_modes::Enabled::acquire());
        self.focus_change_enabled = enable_focus_change().is_ok();
        self.events = Some(TerminalEvents::new());
        Ok(())
    }
}

fn enable_focus_change() -> io::Result<()> {
    // ConPTY delivers CSI I/O when VT input is enabled; native FOCUS_EVENT
    // records are only the legacy path. Both require the appropriate setup.
    execute!(io::stdout(), EnableFocusChange)
}

fn disable_focus_change() -> io::Result<()> {
    execute!(io::stdout(), DisableFocusChange)
}

fn acquire_mouse_capture(screen: TerminalScreen) -> bool {
    match screen {
        TerminalScreen::Alternate => mouse_capture::enable().is_ok(),
        TerminalScreen::Main => false,
    }
}

fn hand_off_terminal(screen: TerminalScreen, handoff_status: &str) -> io::Result<()> {
    // Disable raw mode before clearing the screen. Raw mode has more side
    // effects, matching ratatui::try_restore's ordering.
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    match screen {
        TerminalScreen::Alternate => queue!(stdout, LeaveAlternateScreen, Clear(ClearType::All))?,
        // Erase row by row: erasing the display can push it into scrollback.
        // Resume repaints the session frame.
        TerminalScreen::Main => {
            let (_, rows) = crossterm::terminal::size()?;
            super::synced_backend::queue_erase_rows(&mut stdout, 0..rows)?;
        }
    }
    execute!(
        stdout,
        MoveTo(0, 0),
        Show,
        Print(handoff_status.replace("\r\n", "\n").replace('\n', "\r\n")),
        Print("\r\n"),
    )?;
    Ok(())
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.stop_events();
        if let Some(keyboard) = self.keyboard.take() {
            keyboard.release();
        }
        if self.mouse_capture_enabled {
            let _ = mouse_capture::disable();
            self.mouse_capture_enabled = false;
        }
        if self.focus_change_enabled {
            let _ = disable_focus_change();
            self.focus_change_enabled = false;
        }
    }
}
