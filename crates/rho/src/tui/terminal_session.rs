use std::future::Future;

use anyhow::{anyhow, Context};
use crossterm::event::Event;
use ratatui::DefaultTerminal;

use super::{keyboard_modes, mouse_capture, terminal_events::TerminalEvents};

pub(super) struct SuspendedRun<T> {
    pub(super) operation_result: anyhow::Result<T>,
    pub(super) resume_result: anyhow::Result<()>,
}

pub(super) struct TerminalSession {
    events: Option<TerminalEvents>,
    keyboard: Option<keyboard_modes::Enabled>,
    mouse_capture_enabled: bool,
}

impl TerminalSession {
    pub(super) fn acquire() -> Self {
        Self {
            events: Some(TerminalEvents::new()),
            keyboard: Some(keyboard_modes::Enabled::acquire()),
            mouse_capture_enabled: mouse_capture::enable().is_ok(),
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
    /// The outer result reports terminal lifecycle failures. The inner result
    /// belongs to the suspended operation and is safe to present in the TUI
    /// because the terminal has resumed before this method returns it.
    pub(super) async fn run_suspended<T, F, Fut>(
        &mut self,
        terminal: &mut DefaultTerminal,
        operation: F,
    ) -> SuspendedRun<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        self.stop_events();
        if let Err(suspend_error) = self.suspend() {
            return SuspendedRun {
                operation_result: Err(suspend_error.context("external editor was not started")),
                resume_result: self
                    .resume(terminal)
                    .context("failed to recover Rho after terminal suspension failed"),
            };
        }

        let operation_result = operation().await;
        let resume_result = self
            .resume(terminal)
            .context("failed to resume Rho after external editor");
        SuspendedRun {
            operation_result,
            resume_result,
        }
    }

    fn stop_events(&mut self) {
        self.events = None;
    }

    fn suspend(&mut self) -> anyhow::Result<()> {
        let mut failures = Vec::new();
        if self.mouse_capture_enabled {
            if let Err(error) = mouse_capture::disable() {
                failures.push(format!("disable mouse capture: {error}"));
            }
            self.mouse_capture_enabled = false;
        }
        if let Some(keyboard) = self.keyboard.take() {
            if let Err(error) = keyboard.try_release() {
                failures.push(format!("disable keyboard modes: {error}"));
            }
        }
        if let Err(error) = ratatui::try_restore() {
            failures.push(format!("restore terminal: {error}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(failures.join("; ")))
        }
    }

    fn resume(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        let resumed = ratatui::try_init().context("initialize terminal")?;
        *terminal = resumed;
        self.keyboard = Some(keyboard_modes::Enabled::acquire());
        self.mouse_capture_enabled = mouse_capture::enable().is_ok();
        self.events = Some(TerminalEvents::new());
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Some(keyboard) = self.keyboard.take() {
            keyboard.release();
        }
        if self.mouse_capture_enabled {
            let _ = mouse_capture::disable();
            self.mouse_capture_enabled = false;
        }
    }
}
