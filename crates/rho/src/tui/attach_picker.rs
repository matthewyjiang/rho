//! `/attach` overlay: pick a workspace subagent by role, title, and activity.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::subagent::{self, RunState, RunningRun};
use crate::title::activity_label;

use super::{
    picker::OverlayChrome, App, ComposerMode, PickerBadge, PickerBadgeTone, PickerItem,
    PickerLayout, UiPicker,
};

const RUNNING_ONLY_KEYS_HINT: &str = "↑↓ runs · Ctrl-R show finished";
const ALL_RUNS_KEYS_HINT: &str = "↑↓ runs · Ctrl-R running only";

/// Which attach-picker rows are visible. Listing always returns every workspace run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceRunFilter {
    RunningOnly,
    All,
}

impl WorkspaceRunFilter {
    pub(super) fn toggled(self) -> Self {
        match self {
            Self::RunningOnly => Self::All,
            Self::All => Self::RunningOnly,
        }
    }
}

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
    Ok(subagent::list_workspace_runs(cwd)?
        .into_iter()
        .map(AttachCandidate::from)
        .collect())
}

pub(super) fn merge_live_candidates(
    mut candidates: Vec<AttachCandidate>,
    live: Vec<AttachCandidate>,
) -> Vec<AttachCandidate> {
    let mut missing = Vec::new();
    for live_run in live {
        if let Some(existing) = candidates
            .iter_mut()
            .find(|candidate| candidate.run_id == live_run.run_id)
        {
            *existing = live_run;
        } else {
            missing.push(live_run);
        }
    }
    missing.extend(candidates);
    missing
}

pub(super) fn retire_departed_live_runs(
    candidates: &mut [AttachCandidate],
    live_ids: &std::collections::HashSet<String>,
    previously_live: &std::collections::HashSet<String>,
    mut terminal_state: impl FnMut(&str) -> RunState,
) {
    for candidate in candidates {
        if previously_live.contains(&candidate.run_id)
            && !live_ids.contains(&candidate.run_id)
            && !candidate.state.is_terminal()
        {
            candidate.state = terminal_state(&candidate.run_id);
        }
    }
}

fn finished_run_state(run_id: &str) -> RunState {
    let Ok(directory) = subagent::resolve_run_directory(run_id) else {
        return RunState::Stopped;
    };
    subagent::read_status(&directory.join(subagent::RESULT_FILE_NAME))
        .map(|status| status.state)
        .filter(|state| state.is_terminal())
        .unwrap_or(RunState::Stopped)
}

pub(super) fn candidate_agent_id<'a>(
    candidates: &'a [AttachCandidate],
    run_id: &str,
) -> Option<&'a str> {
    candidates
        .iter()
        .find(|candidate| candidate.run_id == run_id)
        .map(|candidate| candidate.agent_id.as_str())
        .filter(|agent_id| !agent_id.is_empty())
}

pub(super) fn picker(candidates: &[AttachCandidate], filter: WorkspaceRunFilter) -> UiPicker {
    let items = visible_candidates(candidates, filter)
        .into_iter()
        .map(candidate_item)
        .collect();
    let running_only = matches!(filter, WorkspaceRunFilter::RunningOnly);
    UiPicker::attach_subagent("attach subagent", items)
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
        if !picker.is_attach_subagent() || !is_running_filter_toggle(key) {
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
        if !open.is_attach_subagent() {
            return;
        }
        let cursor = open.cursor();
        let mut next = picker(&self.sync_attach_candidates(), self.attach_run_filter);
        next.restore_cursor(&cursor);
        self.input_ui.set_composer(ComposerMode::Picker(next));
    }

    pub(super) fn submit_attach_selection(&mut self, run_id: &str) {
        let agent_id = candidate_agent_id(&self.sync_attach_candidates(), run_id)
            .unwrap_or("agent")
            .to_owned();
        self.activate_subagent_row(&super::subagent_panel::SubagentAttachTarget {
            run_id: run_id.to_owned(),
            agent_id,
        });
    }

    fn open_attach_picker(&mut self) {
        self.attach_seen_live.clear();
        let listing_error = match workspace_candidates(&self.info.runtime.cwd) {
            Ok(disk) => {
                self.attach_disk_candidates = disk;
                None
            }
            Err(error) => {
                self.attach_disk_candidates.clear();
                Some(error)
            }
        };
        let candidates = self.sync_attach_candidates();
        self.input_ui.set_composer(ComposerMode::Picker(picker(
            &candidates,
            self.attach_run_filter,
        )));
        match listing_error {
            Some(error) => self.set_status(format!("could not list workspace runs: {error}")),
            None => self.set_status("attach subagent"),
        }
    }

    fn sync_attach_candidates(&mut self) -> Vec<AttachCandidate> {
        let live = self.subagent_panel.candidates();
        let live_ids = live
            .iter()
            .map(|candidate| candidate.run_id.clone())
            .collect::<std::collections::HashSet<_>>();
        self.attach_disk_candidates =
            merge_live_candidates(std::mem::take(&mut self.attach_disk_candidates), live);
        retire_departed_live_runs(
            &mut self.attach_disk_candidates,
            &live_ids,
            &self.attach_seen_live,
            finished_run_state,
        );
        self.attach_seen_live.extend(live_ids);
        self.attach_disk_candidates.clone()
    }
}

#[cfg(test)]
#[path = "attach_picker_tests.rs"]
mod tests;
