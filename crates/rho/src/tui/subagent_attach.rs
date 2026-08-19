//! In-place attach view for activity-rail clicks and `/attach`.

use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};

use ratatui::Frame;

use super::{
    attachment::{AttachInput, AttachmentApp, AttachmentDisplaySettings},
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
pub(super) fn action_hint() -> &'static str {
    "attach"
}

impl App {
    pub(super) fn is_attach_view(&self) -> bool {
        self.embedded_attach.is_some()
    }

    pub(super) fn draw_embedded_attach(&mut self, frame: &mut Frame<'_>) -> bool {
        if !self.is_attach_view() {
            return false;
        }
        let notice = self.parent_attach_notice();
        if let Some(view) = self.embedded_attach.as_mut() {
            view.set_parent_notice(notice);
            view.draw(frame);
            view.note_drawn();
        }
        true
    }

    pub(super) fn subagent_action_hint(&self) -> &'static str {
        action_hint()
    }

    pub(super) fn activate_subagent_row(&mut self, target: &SubagentAttachTarget, _now: Instant) {
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
        let directory = crate::subagent::resolve_run_directory(&target.run_id)?;
        let display = AttachmentDisplaySettings {
            show_reasoning_output: self.info.runtime.show_reasoning_output,
            zen_mode: self.info.runtime.zen_mode,
            max_tool_output_lines: self.info.runtime.max_tool_output_lines.max(1),
            theme: self.info.services.theme.clone(),
        };
        let mut view = AttachmentApp::open_embedded(
            &target.run_id,
            directory,
            display,
            self.info.services.herdr.clone(),
        );
        view.set_parent_notice(self.parent_attach_notice());
        self.attach_parent_was_busy = self.is_ui_busy();
        self.embedded_attach = Some(view);
        self.notify_status(format!(
            "attached to {} · {}",
            target.agent_id,
            attach_command(&target.run_id)
        ));
        Ok(())
    }

    pub(super) fn leave_attach_view(&mut self) {
        if self.embedded_attach.take().is_some() {
            self.attach_parent_was_busy = false;
            self.set_status("ready");
        }
    }

    pub(super) fn parent_attach_notice(&self) -> Option<String> {
        match self.input_ui.composer() {
            ComposerMode::Approval(_) => Some("parent approval waiting".into()),
            ComposerMode::Questionnaire(_) => Some("parent questionnaire waiting".into()),
            _ if self.attach_parent_was_busy && !self.is_ui_busy() => {
                Some("parent turn complete".into())
            }
            _ => None,
        }
    }

    pub(super) async fn poll_embedded_attach(&mut self) -> anyhow::Result<bool> {
        if self.embedded_attach.is_none() {
            return Ok(false);
        }
        let notice = self.parent_attach_notice();
        let Some(view) = self.embedded_attach.as_mut() else {
            return Ok(false);
        };
        view.set_parent_notice(notice);
        let changed = view.refresh().await?;
        Ok(changed || view.should_redraw(Instant::now()))
    }

    pub(super) fn handle_attach_view_key(&mut self, key: KeyEvent) -> bool {
        if self.embedded_attach.is_none() {
            return false;
        }
        if is_cycle_key(key) {
            self.cycle_embedded_attach(cycle_delta(key));
            return true;
        }
        let Some(view) = self.embedded_attach.as_mut() else {
            return false;
        };
        match view.handle_event(Event::Key(key)) {
            AttachInput::Ignored => false,
            AttachInput::Handled => true,
            AttachInput::Leave => {
                self.leave_attach_view();
                true
            }
            AttachInput::Quit => {
                self.leave_attach_view();
                self.should_quit = true;
                true
            }
        }
    }

    pub(super) fn handle_attach_view_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(view) = self.embedded_attach.as_mut() else {
            return false;
        };
        view.handle_event(Event::Mouse(mouse)).redraws()
    }

    pub(super) fn handle_attach_view_resize(&mut self) -> bool {
        let Some(view) = self.embedded_attach.as_mut() else {
            return false;
        };
        view.handle_event(Event::Resize(0, 0)).redraws()
    }

    fn cycle_embedded_attach(&mut self, delta: isize) {
        let candidates = self.subagent_panel.candidates();
        if candidates.len() < 2 {
            return;
        }
        let current = self
            .embedded_attach
            .as_ref()
            .map(AttachmentApp::run_id)
            .map(str::to_owned);
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
