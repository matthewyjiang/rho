//! In-place attach view for activity-rail clicks and `/attach`.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use ratatui::Frame;

use super::{
    attachment::{
        AttachChrome, AttachInput, AttachmentApp, AttachmentDisplaySettings, ParentNotice,
    },
    exclusive_screen::ExclusiveOccupant,
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

fn open_view(
    target: &SubagentAttachTarget,
    display: AttachmentDisplaySettings,
) -> anyhow::Result<AttachmentApp> {
    let directory = crate::subagent::resolve_run_directory(&target.run_id)?;
    Ok(AttachmentApp::new(&target.run_id, directory, display))
}

/// Next live-rail target, or `None` when cycling is not defined.
///
/// Live rail only. A finished run opened from `/attach` is not in this set, so
/// Tab is a no-op instead of jumping to the first live row.
pub(super) fn next_target<'a>(
    current: &str,
    targets: &'a [SubagentAttachTarget],
    delta: isize,
) -> Option<&'a SubagentAttachTarget> {
    if targets.len() < 2 {
        return None;
    }
    let index = targets.iter().position(|target| target.run_id == current)?;
    let next = (index as isize + delta).rem_euclid(targets.len() as isize) as usize;
    let target = &targets[next];
    (target.run_id != current).then_some(target)
}

/// Composer-owned parent wait. Distinct from [`ParentNotice::TurnComplete`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParentWait {
    Approval,
    Questionnaire,
}

/// Parent chrome while attach is showing. Composer waits win over turn-complete.
pub(super) fn parent_notice(
    waiting: Option<ParentWait>,
    parent_turn_armed: bool,
    parent_busy: bool,
) -> Option<ParentNotice> {
    match waiting {
        Some(ParentWait::Approval) => Some(ParentNotice::Approval),
        Some(ParentWait::Questionnaire) => Some(ParentNotice::Questionnaire),
        None if parent_turn_armed && !parent_busy => Some(ParentNotice::TurnComplete),
        None => None,
    }
}

fn cycle_delta(key: KeyEvent) -> Option<isize> {
    if key.kind != KeyEventKind::Press
        || key.modifiers.contains(KeyModifiers::CONTROL)
        || key.modifiers.contains(KeyModifiers::ALT)
    {
        return None;
    }
    match key.code {
        KeyCode::Tab | KeyCode::Right => Some(1),
        KeyCode::BackTab | KeyCode::Left => Some(-1),
        _ => None,
    }
}

fn composer_wait(composer: &ComposerMode) -> Option<ParentWait> {
    match composer {
        ComposerMode::Approval(_) => Some(ParentWait::Approval),
        ComposerMode::Questionnaire(_) => Some(ParentWait::Questionnaire),
        _ => None,
    }
}

impl App {
    pub(super) fn draw_attach_screen(&mut self, frame: &mut Frame<'_>) -> bool {
        let Some(parent_turn_armed) = self.exclusive.parent_turn_armed() else {
            return false;
        };
        let notice = parent_notice(
            composer_wait(self.input_ui.composer()),
            parent_turn_armed,
            self.is_ui_busy(),
        );
        let Some(view) = self.exclusive.attach_view_mut() else {
            return false;
        };
        view.draw(frame, AttachChrome::Embedded { notice });
        view.note_drawn();
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
        let display = AttachmentDisplaySettings::from_runtime(
            self.info.runtime.show_reasoning_output,
            self.info.runtime.zen_mode,
            self.info.runtime.max_tool_output_lines,
        );
        let view = open_view(target, display)?;
        let parent_turn_armed = match &self.exclusive {
            ExclusiveOccupant::Attach {
                parent_turn_armed, ..
            } => *parent_turn_armed,
            ExclusiveOccupant::Session
            | ExclusiveOccupant::Setup(_)
            | ExclusiveOccupant::Peek { .. } => self.is_ui_busy(),
        };
        self.exclusive = ExclusiveOccupant::Attach {
            view: Box::new(view),
            parent_turn_armed,
        };
        self.notify_status(format!(
            "attached to {} · {}",
            target.agent_id,
            attach_command(&target.run_id)
        ));
        Ok(())
    }

    pub(super) fn leave_attach_view(&mut self) {
        if matches!(self.exclusive, ExclusiveOccupant::Attach { .. }) {
            self.exclusive = ExclusiveOccupant::Session;
        }
    }

    pub(super) fn route_attach_event(&mut self, event: Event) -> bool {
        let resize = matches!(event, Event::Resize(_, _));
        if let Event::Key(key) = event {
            if let Some(delta) = cycle_delta(key) {
                self.cycle_attachment_view(delta);
                return resize;
            }
        }
        let Some(view) = self.exclusive.attach_view_mut() else {
            return resize;
        };
        match view.handle_event(event) {
            AttachInput::Leave => self.leave_attach_view(),
            AttachInput::Quit => {
                self.leave_attach_view();
                self.notify_status("left attach view; press ctrl-c again to quit");
                self.ctrl_c_streak = 1;
            }
            AttachInput::Ignored | AttachInput::Handled => {}
        }
        resize
    }

    fn cycle_attachment_view(&mut self, delta: isize) {
        let targets = self
            .subagent_panel
            .candidates()
            .into_iter()
            .map(|candidate| SubagentAttachTarget {
                run_id: candidate.run_id,
                agent_id: candidate.agent_id,
            })
            .collect::<Vec<_>>();
        let Some(current) = self
            .exclusive
            .attach_view()
            .map(|view| view.run_id().to_owned())
        else {
            return;
        };
        let Some(target) = next_target(&current, &targets, delta).cloned() else {
            return;
        };
        if let Err(error) = self.enter_attach_view(&target) {
            self.notify_status(format!("could not switch attach view: {error}"));
        }
    }
}

#[cfg(test)]
#[path = "subagent_attach_tests.rs"]
mod tests;
