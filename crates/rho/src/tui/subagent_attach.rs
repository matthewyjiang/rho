//! In-place attach view for activity-rail clicks and `/attach`.

use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use ratatui::Frame;

use super::{
    attachment::{embedded_footer_hint, AttachInput, AttachmentApp, AttachmentDisplaySettings},
    subagent_panel::SubagentAttachTarget,
    App, ComposerMode,
};

/// Command a user runs to watch delegated run `run_id` from another terminal.
///
/// `run_id` is a validated 6-char hex id, so it needs no shell quoting.
pub(super) fn attach_command(run_id: &str) -> String {
    format!("rho attach {run_id}")
}

/// Short hover hint shown on the right edge of a subagent row.
pub(super) const ACTION_HINT: &str = "attach";

/// Modal session for the in-place attach viewer.
pub(super) struct EmbeddedAttach {
    view: AttachmentApp,
    /// Snapshot of parent busy-ness when the modal first opened. Cycling runs
    /// must not resample this or the "parent turn complete" badge disappears.
    opened_while_busy: bool,
}

impl EmbeddedAttach {
    fn open(
        target: &SubagentAttachTarget,
        display: AttachmentDisplaySettings,
        opened_while_busy: bool,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            view: open_view(target, display)?,
            opened_while_busy,
        })
    }

    fn switch_run(
        &mut self,
        target: &SubagentAttachTarget,
        display: AttachmentDisplaySettings,
    ) -> anyhow::Result<()> {
        self.view = open_view(target, display)?;
        Ok(())
    }

    fn should_redraw(&self, now: Instant) -> bool {
        self.view.should_redraw(now)
    }
}

fn open_view(
    target: &SubagentAttachTarget,
    display: AttachmentDisplaySettings,
) -> anyhow::Result<AttachmentApp> {
    let directory = crate::subagent::resolve_run_directory(&target.run_id)?;
    Ok(AttachmentApp::new(&target.run_id, directory, display))
}

impl App {
    pub(super) fn is_attach_view(&self) -> bool {
        self.embedded_attach.is_some()
    }

    pub(super) fn draw_embedded_attach(&mut self, frame: &mut Frame<'_>) -> bool {
        let notice = self.parent_attach_notice();
        let Some(session) = self.embedded_attach.as_mut() else {
            return false;
        };
        session
            .view
            .draw(frame, embedded_footer_hint(), notice.as_deref());
        session.view.note_drawn();
        true
    }

    pub(super) fn activate_subagent_row(&mut self, target: &SubagentAttachTarget) {
        if let Err(error) = self.enter_attach_view(target) {
            self.notify_status(format!(
                "could not attach to {} {}: {error}",
                target.agent_id, target.run_id
            ));
        }
    }

    pub(super) fn enter_attach_view(
        &mut self,
        target: &SubagentAttachTarget,
    ) -> anyhow::Result<()> {
        let display = AttachmentDisplaySettings {
            show_reasoning_output: self.info.runtime.show_reasoning_output,
            zen_mode: self.info.runtime.zen_mode,
            max_tool_output_lines: self.info.runtime.max_tool_output_lines.max(1),
            theme: self.info.services.theme.clone(),
        };
        if let Some(session) = self.embedded_attach.as_mut() {
            session.switch_run(target, display)?;
        } else {
            self.embedded_attach = Some(EmbeddedAttach::open(target, display, self.is_ui_busy())?);
        }
        self.notify_status(format!(
            "attached to {} · {}",
            target.agent_id,
            attach_command(&target.run_id)
        ));
        Ok(())
    }

    pub(super) fn leave_attach_view(&mut self) {
        self.embedded_attach = None;
    }

    pub(super) fn parent_attach_notice(&self) -> Option<String> {
        match self.input_ui.composer() {
            ComposerMode::Approval(_) => Some("parent approval waiting".into()),
            ComposerMode::Questionnaire(_) => Some("parent questionnaire waiting".into()),
            _ if self
                .embedded_attach
                .as_ref()
                .is_some_and(|session| session.opened_while_busy)
                && !self.is_ui_busy() =>
            {
                Some("parent turn complete".into())
            }
            _ => None,
        }
    }

    pub(super) async fn poll_embedded_attach(&mut self) -> anyhow::Result<bool> {
        let Some(session) = self.embedded_attach.as_mut() else {
            return Ok(false);
        };
        let changed = session.view.refresh().await?;
        Ok(changed || session.view.should_redraw(Instant::now()))
    }

    /// Consume a terminal event when the attach modal is open.
    ///
    /// Returns `false` when attach is not showing so the parent TUI can handle
    /// the event. When attach is showing, every event is consumed.
    pub(super) fn dispatch_attach(&mut self, event: Event) -> bool {
        if self.embedded_attach.is_none() {
            return false;
        }
        if let Event::Key(key) = event {
            if is_cycle_key(key) {
                self.cycle_embedded_attach(cycle_delta(key));
                return true;
            }
            match self
                .embedded_attach
                .as_mut()
                .expect("attach view checked above")
                .view
                .handle_event(Event::Key(key))
            {
                AttachInput::Leave => self.leave_attach_view(),
                AttachInput::Quit => {
                    self.leave_attach_view();
                    self.should_quit = true;
                }
                AttachInput::Ignored | AttachInput::Handled => {}
            }
            return true;
        }
        let _ = self
            .embedded_attach
            .as_mut()
            .expect("attach view checked above")
            .view
            .handle_event(event);
        true
    }

    pub(super) fn attach_should_redraw(&self, now: Instant) -> bool {
        self.embedded_attach
            .as_ref()
            .is_some_and(|session| session.should_redraw(now))
    }

    fn cycle_embedded_attach(&mut self, delta: isize) {
        let candidates = self.subagent_panel.candidates();
        if candidates.len() < 2 {
            return;
        }
        let current = self
            .embedded_attach
            .as_ref()
            .map(|session| session.view.run_id().to_owned());
        let index = current
            .as_deref()
            .and_then(|run_id| {
                candidates
                    .iter()
                    .position(|candidate| candidate.run_id == run_id)
            })
            .unwrap_or(0);
        let next = (index as isize + delta).rem_euclid(candidates.len() as isize) as usize;
        if Some(candidates[next].run_id.as_str()) == current.as_deref() {
            return;
        }
        let target = SubagentAttachTarget {
            run_id: candidates[next].run_id.clone(),
            agent_id: candidates[next].agent_id.clone(),
        };
        if let Err(error) = self.enter_attach_view(&target) {
            self.notify_status(format!("could not switch attach view: {error}"));
        }
    }
}

fn is_cycle_key(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && match key.code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
            }
            _ => false,
        }
}

fn cycle_delta(key: KeyEvent) -> isize {
    match key.code {
        KeyCode::Tab | KeyCode::Right => 1,
        KeyCode::BackTab | KeyCode::Left => -1,
        _ => 0,
    }
}

#[cfg(test)]
#[path = "subagent_attach_tests.rs"]
mod tests;
