//! `/attach` overlay: pick a running subagent by role, title, and activity.

use crate::subagent::{self, RunState, RunningRun};
use crate::title::activity_label;

use super::{
    picker_overlay::OverlayChrome, App, ComposerMode, PickerAction, PickerBadge, PickerBadgeTone,
    PickerItem, PickerLayout, UiPicker,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AttachCandidate {
    pub run_id: String,
    pub agent_id: String,
    pub title: Option<String>,
    pub last_activity: Option<String>,
    pub state: RunState,
    pub elapsed_seconds: u64,
}

impl From<RunningRun> for AttachCandidate {
    fn from(run: RunningRun) -> Self {
        Self {
            run_id: run.id,
            agent_id: run.agent_id,
            title: run.title,
            last_activity: run.last_activity,
            state: run.state,
            elapsed_seconds: run.elapsed_seconds,
        }
    }
}

pub(super) fn picker(candidates: &[AttachCandidate]) -> UiPicker {
    let items = candidates.iter().map(candidate_item).collect();
    UiPicker::new("attach subagent", items, PickerAction::AttachSubagent)
        .with_layout(PickerLayout::Overlay)
        .with_badge_placement(super::PickerBadgePlacement::Navigation)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " SUBAGENTS".into(),
            detail_label: Some(" ACTIVITY".into()),
            nav_keys_hint: "↑↓ runs".into(),
        })
        .with_confirm_verb("attach")
}

fn candidate_item(candidate: &AttachCandidate) -> PickerItem {
    let title = candidate
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .unwrap_or("untitled");
    let activity = match candidate.state {
        RunState::Starting => "starting",
        RunState::Running => activity_label(candidate.last_activity.as_deref()),
        RunState::Ok | RunState::Error | RunState::Stopped => candidate.state.as_str(),
    };
    let elapsed = subagent::format_elapsed_secs(candidate.elapsed_seconds);
    PickerItem {
        section: Some(candidate.agent_id.clone()),
        label: title.to_owned(),
        detail: Some(format!(
            "{activity}\nelapsed {elapsed}\nrole {}",
            candidate.agent_id
        )),
        preview: None,
        badge: Some(PickerBadge {
            text: activity.to_owned(),
            tone: PickerBadgeTone::Internal,
        }),
        value: candidate.run_id.clone(),
        selection_verb: Some("attach"),
    }
}

impl App {
    pub(super) fn execute_attach_command(&mut self) -> anyhow::Result<()> {
        let candidates = self.subagent_panel.candidates();
        if candidates.is_empty() {
            self.set_status("no running subagents");
            return Ok(());
        }
        self.input_ui
            .set_composer(ComposerMode::Picker(picker(&candidates)));
        self.set_status("attach subagent");
        Ok(())
    }

    pub(super) fn refresh_attach_picker(&mut self) {
        let ComposerMode::Picker(open) = self.input_ui.composer() else {
            return;
        };
        if open.action != PickerAction::AttachSubagent {
            return;
        }
        let cursor = open.cursor();
        let candidates = self.subagent_panel.candidates();
        if candidates.is_empty() {
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("no running subagents");
            return;
        }
        let mut next = picker(&candidates);
        next.restore_cursor(&cursor);
        self.input_ui.set_composer(ComposerMode::Picker(next));
    }

    pub(super) fn submit_attach_selection(&mut self, run_id: &str) {
        let Some(target) = self.subagent_panel.attach_target(run_id) else {
            self.set_status("that subagent is no longer running");
            return;
        };
        self.activate_subagent_row(&target, std::time::Instant::now());
    }
}

#[cfg(test)]
#[path = "attach_picker_tests.rs"]
mod tests;
