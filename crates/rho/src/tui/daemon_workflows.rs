//! Workflows of a daemon session: the hub's launches, which the daemon
//! runs, and the workflow rail, which shows the runs the session started.
//!
//! The hub lists sources, plans, and runs from the workspace's store, which
//! this process shares with the daemon. Starting one is a `/workflow:`
//! prompt, so the daemon plans and drives the run and it outlives this
//! process. The rail takes its rows from the daemon's jobs
//! (`daemon_jobs` asks for them) and each run's progress from the store.

use std::collections::HashMap;

use rho_protocol::{JobState, JobSummary, TurnOutcome};

use super::{
    daemon_jobs::may_still_change,
    idle_input::{HeldOrigin, HeldTurn},
    turn_prompt::TurnPrompt,
    App, DefaultTerminal, Entry,
};
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
    /// run. The daemon starts it only once the session's MCP servers
    /// connected, and the TUI would wait for it meanwhile: until they
    /// have, the launch is held, as a prompt is.
    pub(super) async fn launch_daemon_workflow(
        &mut self,
        command: String,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.input_ui.set_composer(super::ComposerMode::Input);
        let turn = TurnPrompt::command(command.clone(), command);
        if self
            .info
            .session
            .daemon
            .as_ref()
            .is_some_and(super::DaemonSession::tool_sources_loading)
        {
            self.hold(HeldTurn {
                turn,
                media: Vec::new(),
                paste_segments: Vec::new(),
                origin: HeldOrigin::WorkflowHub,
            });
            return Ok(());
        }
        self.send_daemon_workflow(turn, terminal).await
    }

    /// Sends `turn`, a workflow launch. The prompt is the hub's spelling,
    /// not something to type: it stays out of the input history, the user
    /// message the daemon shows for it gets no row, and a prompt that could
    /// not be sent is reported without touching the composer's draft. A
    /// session reopened later replays the prompt as the daemon saved it.
    pub(super) async fn send_daemon_workflow(
        &mut self,
        turn: TurnPrompt,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.daemon_hub_launch = Some(turn.display.clone());
        let sent = self.request_daemon_turn(&turn, &[]).await;
        // Why the prompt found no attachment shows first.
        self.show_taken_daemon_views(terminal).await?;
        let Err(error) = sent else {
            return Ok(());
        };
        // No turn began, so no user message comes back.
        self.daemon_hub_launch = None;
        let ended = error
            .downcast_ref::<rho_protocol::ClientError>()
            .and_then(rho_protocol::ClientError::turn_ended);
        match ended {
            // The daemon's rows say how the start ended.
            Some(
                TurnOutcome::Failed { .. } | TurnOutcome::Cancelled | TurnOutcome::TimedOut { .. },
            ) => {}
            // Not sent, or ended in a way this rho cannot show.
            Some(_) | None => self.insert_entry(&Entry::Error(format!(
                "could not start the workflow: {error:#}"
            ))),
        }
        self.set_status("start failed");
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
        let sources = discover_workflow_sources(&self.info.runtime.cwd);
        let source = sources
            .iter()
            .find(|workflow| workflow.relative_path == relative_path);
        // The daemon finds the workflow again by its name, ignoring case:
        // a name two files share starts neither, so say which row it was.
        let twin = source.and_then(|source| {
            sources.iter().find(|other| {
                other.relative_path != source.relative_path
                    && other.label.eq_ignore_ascii_case(&source.label)
            })
        });
        let refused = match (source.map(DiscoveredWorkflow::command_name), twin) {
            (Some(_), Some(twin)) => {
                format!("{} has the same name; rename one", twin.relative_path)
            }
            (Some(Some(command)), None) => {
                return self
                    .launch_daemon_workflow(format!("/{command}"), terminal)
                    .await
            }
            (Some(None), None) => "its name has spaces; rename the file".to_owned(),
            (None, _) => "the file is gone".to_owned(),
        };
        self.input_ui.set_composer(super::ComposerMode::Input);
        self.insert_entry(&Entry::Error(format!(
            "could not start {relative_path}: {refused}"
        )));
        self.set_status("start failed");
        Ok(())
    }

    /// Puts `jobs`, the followed session's workflow jobs at `now` (Unix
    /// seconds), on the workflow rail: one row for each run still being
    /// driven or whose result the session has yet to be given (see
    /// [`shown_drives`]). A run whose saved state cannot be read has no row.
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
        let rows = shown_drives(jobs)
            .into_iter()
            .filter_map(|(run, job)| Some(drive_row(job, runs.progress(run)?, now)))
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

/// The rail row of a run whose newest drive is `job`, from `progress`, its
/// saved state, at `now` (Unix seconds). The daemon ends a drive `Failed`
/// for a failed, denied, or blocked outcome too, which the saved state
/// tells apart, so only a drive that failed while its run still reads as
/// live failed on its own, as a drive that errors does in process.
fn drive_row(job: &JobSummary, progress: WorkflowRailSummary, now: u64) -> WorkflowRailSummary {
    let until = if may_still_change(job.state) {
        now
    } else {
        job.updated_at
    };
    WorkflowRailSummary {
        failed: job.state == JobState::Failed && progress.is_live(),
        elapsed_seconds: until.saturating_sub(job.created_at),
        ..progress
    }
}

/// Each run of `jobs` the rail shows with the job that drove it last, in
/// the order the runs first appear. A run resumed has a job for each drive,
/// and the one that began last says where the run is. A run shows while
/// that drive may still change, and then until the session is given its
/// result, as in process.
fn shown_drives<'a>(jobs: &[&'a JobSummary]) -> Vec<(RunId, &'a JobSummary)> {
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
    drives.retain(|(_, job)| may_still_change(job.state) || job.result_pending);
    drives
}

#[cfg(test)]
#[path = "daemon_workflows_tests.rs"]
mod tests;
