//! Workflows of a daemon session: the hub's launches, which the daemon
//! runs, and the workflow rail, which shows the runs the session started.
//!
//! The hub lists sources, plans, and runs from the workspace's store, which
//! this process shares with the daemon. Starting one is a `/workflow:`
//! prompt, so the daemon plans and drives the run and it outlives this
//! process. The rail takes its rows from the daemon's jobs
//! (`daemon_jobs` asks for them) and each run's progress from the store.

use std::collections::HashMap;

use rho_protocol::{JobState, JobSummary};

use super::{daemon_jobs::may_still_change, turn_prompt::TurnPrompt, App, DefaultTerminal, Entry};
use crate::{
    app::workflow_cli::{
        discover::{discover_workflow_sources, DiscoveredWorkflow},
        WorkflowOps,
    },
    tools::workflow_tracker::WorkflowRailSummary,
    workflow::RunId,
};

impl App {
    /// Sends `command`, a `/workflow:` prompt, for the daemon to start its
    /// run. The prompt is the hub's spelling, not something to type: it
    /// stays out of the input history, and a prompt that could not be sent
    /// is reported without touching the composer's draft.
    pub(super) async fn launch_daemon_workflow(
        &mut self,
        command: String,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.input_ui.set_composer(super::ComposerMode::Input);
        let turn = TurnPrompt::command(command.clone(), command);
        let sent = self.request_daemon_turn(&turn, &[]).await;
        // Why the prompt found no attachment shows first.
        self.show_taken_daemon_views(terminal).await?;
        if let Err(error) = sent {
            self.insert_entry(&Entry::Error(format!(
                "could not start the workflow: {error:#}"
            )));
            self.set_status("start failed");
        }
        Ok(())
    }

    /// Starts the workspace workflow at `relative_path`, a hub row, with
    /// its default inputs. The daemon takes a workflow by its command name,
    /// so one whose name cannot be a command is refused here.
    pub(super) async fn start_daemon_workflow_source(
        &mut self,
        relative_path: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let source = discover_workflow_sources(&self.info.runtime.cwd)
            .into_iter()
            .find(|workflow| workflow.relative_path == relative_path);
        let refused = match source.as_ref().map(DiscoveredWorkflow::command_name) {
            Some(Some(command)) => {
                return self
                    .launch_daemon_workflow(format!("/{command}"), terminal)
                    .await
            }
            Some(None) => "its name has spaces; rename the file",
            None => "the file is gone",
        };
        self.input_ui.set_composer(super::ComposerMode::Input);
        self.insert_entry(&Entry::Error(format!(
            "could not start {relative_path}: {refused}"
        )));
        self.set_status("start failed");
        Ok(())
    }

    /// Puts `jobs`, the followed session's workflow jobs worth showing at
    /// `now` (Unix seconds), on the workflow rail: one row for each run,
    /// from the newest job that drives it, since a run resumed has a job
    /// for each drive. A run whose saved state cannot be read has no row.
    pub(super) fn show_daemon_workflows(&mut self, jobs: &[&JobSummary], now: u64) {
        if jobs.is_empty() {
            self.daemon_rail.workflows = WorkflowRuns::default();
            self.workflow_panel.ingest(Vec::new());
            return;
        }
        if self.daemon_rail.workflows.ops.is_none() {
            self.daemon_rail.workflows.ops = self.workflow_ops().ok();
        }
        let runs = &mut self.daemon_rail.workflows;
        let rows = newest_drives(jobs)
            .into_iter()
            .filter_map(|(run, job)| {
                let until = if may_still_change(job.state) {
                    now
                } else {
                    job.updated_at
                };
                Some(WorkflowRailSummary {
                    failed: job.state == JobState::Failed,
                    elapsed_seconds: until.saturating_sub(job.created_at),
                    ..runs.progress(run)?
                })
            })
            .collect();
        self.workflow_panel.ingest(rows);
    }
}

/// What the workflow rail remembers between job lists.
#[derive(Default)]
pub(super) struct WorkflowRuns {
    /// The workspace's store, opened with the first list that names a run.
    ops: Option<WorkflowOps>,
    /// Each run's row as of the revision it was read at. Loading a run
    /// replays its journal and checks its artifacts, so it is read again
    /// only once its revision moved.
    loaded: HashMap<RunId, (u64, WorkflowRailSummary)>,
}

impl WorkflowRuns {
    /// `run`'s progress as its saved state has it now, with no drive's
    /// failure or time.
    fn progress(&mut self, run: RunId) -> Option<WorkflowRailSummary> {
        let ops = self.ops.as_ref()?;
        let revision = ops.run_revision(run).ok()?;
        if let Some((_, row)) = self.loaded.get(&run).filter(|(at, _)| *at == revision) {
            return Some(row.clone());
        }
        let stored = ops.load_run_id(run).ok()?;
        let row = WorkflowRailSummary::from_stored(&stored);
        self.loaded
            .insert(run, (stored.state.state.revision, row.clone()));
        Some(row)
    }
}

/// Each run `jobs` drive with the job that began driving it last, in the
/// order the runs first appear.
fn newest_drives<'a>(jobs: &[&'a JobSummary]) -> Vec<(RunId, &'a JobSummary)> {
    let mut drives: Vec<(RunId, &JobSummary)> = Vec::new();
    for job in jobs {
        let Some(run) = job
            .workflow_run
            .as_ref()
            .and_then(|run| run.parse::<RunId>().ok())
        else {
            continue;
        };
        match drives.iter_mut().find(|(known, _)| *known == run) {
            Some((_, newest)) if newest.created_at <= job.created_at => *newest = job,
            Some(_) => {}
            None => drives.push((run, job)),
        }
    }
    drives
}

#[cfg(test)]
#[path = "daemon_workflows_tests.rs"]
mod tests;
