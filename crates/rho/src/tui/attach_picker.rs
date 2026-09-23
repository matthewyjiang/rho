//! `/attach` overlay: pick a workspace subagent by role, title, and activity.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::subagent::{self, RunState, RunStatus, RunningRun};
use crate::title::activity_label;

use super::{
    picker::{DetailBlock, DetailField, DetailSheet, DetailTone, ExcerptAnchor, OverlayChrome},
    usage_cost::format_token_count,
    App, ComposerMode, PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
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

/// Rows shown in the attach-picker TASK excerpt.
const TASK_EXCERPT_ROWS: usize = 4;
/// Rows shown in the LATEST excerpt, which keeps the newest output.
const LATEST_EXCERPT_ROWS: usize = 8;

/// One run the attach picker can list. `status` is the run's `result.json`
/// view (live from the panel or read from disk); `prompt` is its launch task.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct AttachCandidate {
    pub run_id: String,
    pub agent_id: String,
    pub elapsed_seconds: u64,
    pub status: RunStatus,
    pub prompt: Option<String>,
}

impl AttachCandidate {
    pub(super) fn state(&self) -> RunState {
        self.status.state
    }
}

impl From<RunningRun> for AttachCandidate {
    fn from(run: RunningRun) -> Self {
        Self {
            run_id: run.id,
            agent_id: run.agent_id,
            elapsed_seconds: run.elapsed_seconds,
            status: run.status,
            prompt: run.prompt,
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
            !matches!(filter, WorkspaceRunFilter::RunningOnly) || !candidate.state().is_terminal()
        })
        .collect()
}

pub(super) fn workspace_candidates(cwd: &Path) -> anyhow::Result<Vec<AttachCandidate>> {
    Ok(subagent::list_workspace_runs(cwd)?
        .into_iter()
        .map(AttachCandidate::from)
        .collect())
}

/// Overlay live panel rows onto the listed rows. Live rows carry no prompt,
/// so a matching row keeps the one it already has, and a run listed for the
/// first time reads it once through `read_prompt`.
pub(super) fn merge_live_candidates(
    mut candidates: Vec<AttachCandidate>,
    live: Vec<AttachCandidate>,
    mut read_prompt: impl FnMut(&str) -> Option<String>,
) -> Vec<AttachCandidate> {
    let mut missing = Vec::new();
    for mut live_run in live {
        if let Some(existing) = candidates
            .iter_mut()
            .find(|candidate| candidate.run_id == live_run.run_id)
        {
            live_run.prompt = live_run.prompt.or_else(|| existing.prompt.take());
            *existing = live_run;
        } else {
            live_run.prompt = live_run.prompt.or_else(|| read_prompt(&live_run.run_id));
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
            && !candidate.state().is_terminal()
        {
            candidate.status.state = terminal_state(&candidate.run_id);
        }
    }
}

fn journal_prompt(run_id: &str) -> Option<String> {
    let directory = subagent::resolve_run_directory(run_id).ok()?;
    crate::run_artifacts::read_prompt(&directory)
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
            detail_label: Some(" RUN".into()),
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
    PickerItem {
        section: Some(candidate.agent_id.clone()),
        label: candidate_title(candidate).to_owned(),
        detail: Some(candidate_detail(candidate, subagent::unix_now_secs()).into()),
        preview: None,
        badge: Some(nav_badge(candidate)),
        value: candidate.run_id.clone(),
        selection_verb: Some("attach"),
        allow_filter_completion: true,
    }
}

fn candidate_title(candidate: &AttachCandidate) -> &str {
    candidate
        .status
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .unwrap_or("untitled")
}

/// Nav row badge: what a live run is doing, or how a finished one ended.
/// Colour carries the outcome so a failed run stands out while scanning.
fn nav_badge(candidate: &AttachCandidate) -> PickerBadge {
    match candidate.state() {
        RunState::Running => PickerBadge {
            text: activity_label(candidate.status.last_activity.as_deref()).to_owned(),
            tone: PickerBadgeTone::Internal,
        },
        RunState::Starting | RunState::Ok | RunState::Error | RunState::Stopped => {
            state_badge(candidate.state())
        }
    }
}

fn state_badge(state: RunState) -> PickerBadge {
    let (text, tone) = match state {
        RunState::Starting => ("starting", PickerBadgeTone::Muted),
        RunState::Running => ("● running", PickerBadgeTone::Internal),
        RunState::Ok => ("✓ done", PickerBadgeTone::Healthy),
        RunState::Error => ("✗ error", PickerBadgeTone::Error),
        RunState::Stopped => ("✗ stopped", PickerBadgeTone::Muted),
    };
    PickerBadge {
        text: text.into(),
        tone,
    }
}

/// Run card: identity and outcome, the facts that tell runs apart, what the
/// run was asked, then what it produced.
fn candidate_detail(candidate: &AttachCandidate, now: u64) -> DetailSheet {
    let status = &candidate.status;
    // The role is the nav section header beside this card, so it is not
    // repeated here.
    let mut fields = Vec::new();
    if candidate.state() == RunState::Running {
        fields.push(DetailField::new(
            "Activity",
            activity_label(status.last_activity.as_deref()),
            DetailTone::Normal,
        ));
    }
    fields.push(model_field(status));
    fields.push(elapsed_field(candidate, now));
    fields.push(tokens_field(status));

    let mut blocks = vec![
        DetailBlock::Title {
            text: candidate_title(candidate).to_owned(),
            tag: Some(state_badge(candidate.state())),
        },
        DetailBlock::Rule,
        DetailBlock::Fields(fields),
        DetailBlock::Rule,
        DetailBlock::Heading {
            label: "TASK".into(),
            status: String::new(),
        },
    ];
    blocks.push(match candidate.prompt.as_deref().map(str::trim) {
        Some(prompt) if !prompt.is_empty() => DetailBlock::Excerpt {
            text: prompt.to_owned(),
            rows: TASK_EXCERPT_ROWS,
            anchor: ExcerptAnchor::Start,
        },
        _ => DetailBlock::Muted("(task not recorded)".into()),
    });
    blocks.push(DetailBlock::Rule);
    blocks.extend(output_blocks(status));
    DetailSheet { blocks }
}

/// Resolved model when the runtime reported one, with reasoning inline and
/// the runtime as a note.
fn model_field(status: &RunStatus) -> DetailField {
    let model = status
        .claude_model
        .as_deref()
        .or(status.model.as_deref())
        .filter(|model| !model.is_empty());
    let Some(model) = model else {
        return DetailField::new("Model", "not reported", DetailTone::Muted);
    };
    let value = match status.reasoning {
        Some(level) => format!("{model} · {level}"),
        None => model.to_owned(),
    };
    let field = DetailField::new("Model", value, DetailTone::Normal);
    match status.runtime {
        Some(runtime) => field.with_note(runtime.as_str()),
        None => field,
    }
}

/// Elapsed time, noting how long ago a finished run ended so old runs are
/// easy to tell from fresh ones.
fn elapsed_field(candidate: &AttachCandidate, now: u64) -> DetailField {
    let field = DetailField::new(
        "Elapsed",
        subagent::format_elapsed_secs(candidate.elapsed_seconds),
        DetailTone::Normal,
    );
    match candidate.status.finished_at {
        Some(finished_at) if candidate.state().is_terminal() => field.with_note(format!(
            "finished {}",
            super::session_picker::format_updated_ago(finished_at, now)
        )),
        _ => field,
    }
}

fn tokens_field(status: &RunStatus) -> DetailField {
    let (input, output) = (status.input_tokens, status.output_tokens);
    if input.is_none() && output.is_none() {
        return DetailField::new("Tokens", "not reported", DetailTone::Muted);
    }
    let count = |tokens: Option<u64>| tokens.map_or_else(|| "?".into(), format_token_count);
    let field = DetailField::new(
        "Tokens",
        format!("{} in · {} out", count(input), count(output)),
        DetailTone::Normal,
    );
    match status.total_cost_usd {
        Some(cost) if cost > 0.0 => field.with_note(format!("${cost:.2}")),
        _ => field,
    }
}

/// What the run produced: the newest output while live, the answer once it
/// finishes, or the failure in the error colour.
fn output_blocks(status: &RunStatus) -> Vec<DetailBlock> {
    let heading = |label: &str| DetailBlock::Heading {
        label: label.into(),
        status: String::new(),
    };
    let text_of = |text: &Option<String>| {
        text.as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    match status.state {
        RunState::Starting | RunState::Running => vec![
            heading("LATEST"),
            match text_of(&status.last_text) {
                Some(text) => DetailBlock::Excerpt {
                    text,
                    rows: LATEST_EXCERPT_ROWS,
                    anchor: ExcerptAnchor::End,
                },
                None => DetailBlock::Muted("(no output yet)".into()),
            },
        ],
        RunState::Ok => vec![
            heading("RESULT"),
            match text_of(&status.result) {
                Some(text) => DetailBlock::Paragraph(text),
                None => DetailBlock::Muted("(no result text)".into()),
            },
        ],
        RunState::Error => vec![
            heading("ERROR"),
            DetailBlock::Error(text_of(&status.error).unwrap_or_else(|| "run failed".into())),
        ],
        RunState::Stopped => vec![
            heading("STOPPED"),
            match text_of(&status.result).or_else(|| text_of(&status.last_text)) {
                Some(text) => DetailBlock::Muted(text),
                None => DetailBlock::Muted("(stopped before any output)".into()),
            },
        ],
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
        let selected = open.selected_item().map(|item| item.value.clone());
        let detail_scroll = open.detail_scroll;
        let mut next = picker(&self.sync_attach_candidates(), self.attach_run_filter);
        next.restore_cursor(&cursor);
        // Live runs refresh every tick; keep the reader's place in the card
        // while the same run stays selected. Rendering clamps the offset.
        if next.selected_item().map(|item| &item.value) == selected.as_ref() {
            next.detail_scroll = detail_scroll;
        }
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
        self.attach_disk_candidates = merge_live_candidates(
            std::mem::take(&mut self.attach_disk_candidates),
            live,
            journal_prompt,
        );
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
