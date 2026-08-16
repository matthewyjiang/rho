//! `/attach` overlay: pick a workspace subagent by role, title, and activity.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::subagent::{self, RunState, RunningRun, WorkspaceRunFilter};
use crate::title::activity_label;

use super::{
    picker_overlay::OverlayChrome, App, ComposerMode, PickerAction, PickerBadge, PickerBadgeTone,
    PickerItem, PickerLayout, UiPicker,
};

const RUNNING_ONLY_KEYS_HINT: &str = "↑↓ runs · Ctrl-R show finished";
const ALL_RUNS_KEYS_HINT: &str = "↑↓ runs · Ctrl-R running only";

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

pub(super) fn is_running_filter_toggle(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
}

pub(super) fn visible_candidates(
    candidates: &[AttachCandidate],
    filter: WorkspaceRunFilter,
) -> Vec<&AttachCandidate> {
    candidates
        .iter()
        .filter(|candidate| {
            !matches!(filter, WorkspaceRunFilter::RunningOnly) || !candidate.state.is_terminal()
        })
        .collect()
}

pub(super) fn workspace_candidates(cwd: &Path) -> anyhow::Result<Vec<AttachCandidate>> {
    Ok(subagent::list_workspace_runs(cwd, WorkspaceRunFilter::All)?
        .into_iter()
        .map(AttachCandidate::from)
        .collect())
}

pub(super) fn merge_live_candidates(
    mut candidates: Vec<AttachCandidate>,
    live: Vec<AttachCandidate>,
) -> Vec<AttachCandidate> {
    for live_run in live {
        if let Some(existing) = candidates
            .iter_mut()
            .find(|candidate| candidate.run_id == live_run.run_id)
        {
            *existing = live_run;
        } else {
            candidates.insert(0, live_run);
        }
    }
    candidates
}

pub(super) fn picker(candidates: &[AttachCandidate], filter: WorkspaceRunFilter) -> UiPicker {
    let items = visible_candidates(candidates, filter)
        .into_iter()
        .map(candidate_item)
        .collect();
    let running_only = matches!(filter, WorkspaceRunFilter::RunningOnly);
    UiPicker::new("attach subagent", items, PickerAction::AttachSubagent)
        .with_layout(PickerLayout::Overlay)
        .with_badge_placement(super::PickerBadgePlacement::Navigation)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " SUBAGENTS".into(),
            detail_label: Some(" ACTIVITY".into()),
            nav_keys_hint: if running_only {
                RUNNING_ONLY_KEYS_HINT
            } else {
                ALL_RUNS_KEYS_HINT
            }
            .into(),
        })
        .with_empty_message(if running_only {
            "no running subagents"
        } else {
            "no subagents in this directory"
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
        self.attach_run_filter = WorkspaceRunFilter::RunningOnly;
        self.open_attach_picker();
        Ok(())
    }

    pub(super) fn toggle_attach_filter_if_requested(&mut self, key: KeyEvent) -> bool {
        let ComposerMode::Picker(picker) = self.input_ui.composer() else {
            return false;
        };
        if picker.action != PickerAction::AttachSubagent || !is_running_filter_toggle(key) {
            return false;
        }
        self.attach_run_filter = self.attach_run_filter.toggled();
        self.refresh_attach_picker();
        true
    }

    pub(super) fn refresh_attach_picker(&mut self) {
        let ComposerMode::Picker(open) = self.input_ui.composer() else {
            return;
        };
        if open.action != PickerAction::AttachSubagent {
            return;
        }
        let cursor = open.cursor();
        let mut next = picker(&self.attach_candidates(), self.attach_run_filter);
        next.restore_cursor(&cursor);
        self.input_ui.set_composer(ComposerMode::Picker(next));
    }

    pub(super) fn submit_attach_selection(&mut self, run_id: &str) {
        let agent_id = self
            .subagent_panel
            .attach_target(run_id)
            .map(|target| target.agent_id)
            .or_else(|| self.selected_attach_agent_id())
            .unwrap_or_else(|| "agent".into());
        self.activate_subagent_row(
            &super::subagent_panel::SubagentAttachTarget {
                run_id: run_id.to_owned(),
                agent_id,
            },
            std::time::Instant::now(),
        );
    }

    fn open_attach_picker(&mut self) {
        self.input_ui.set_composer(ComposerMode::Picker(picker(
            &self.attach_candidates(),
            self.attach_run_filter,
        )));
        self.set_status("attach subagent");
    }

    fn attach_candidates(&self) -> Vec<AttachCandidate> {
        let disk = workspace_candidates(&self.info.runtime.cwd).unwrap_or_default();
        merge_live_candidates(disk, self.subagent_panel.candidates())
    }

    fn selected_attach_agent_id(&self) -> Option<String> {
        let ComposerMode::Picker(picker) = self.input_ui.composer() else {
            return None;
        };
        picker
            .selected_item()
            .and_then(|item| item.section.clone())
            .filter(|agent_id| !agent_id.is_empty())
    }
}

#[cfg(test)]
#[path = "attach_picker_tests.rs"]
mod tests;
