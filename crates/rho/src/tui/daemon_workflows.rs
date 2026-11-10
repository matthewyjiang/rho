//! Workflows of a daemon session: the hub's launches, which the daemon
//! runs, and the workflow rail, which shows the runs the session started.
//!
//! The hub lists sources, plans, and runs from the workspace's store, which
//! this process shares with the daemon. Starting one is a `/workflow:`
//! prompt, so the daemon plans and drives the run and it outlives this
//! process. The rail takes its rows from the daemon's jobs
//! (`daemon_subagents` asks for them) and each run's progress from the
//! store.

use rho_protocol::{JobState, JobSummary};

use super::{daemon_subagents::in_progress, turn_prompt::TurnPrompt, App, DefaultTerminal, Entry};
use crate::{
    app::workflow_cli::discover::discover_workflow_sources,
    tools::workflow_tracker::WorkflowRailSummary, workflow::RunId,
};

impl App {
    /// Sends `command`, a `/workflow:` prompt, for the daemon to start its
    /// run. A refusal shows in the transcript and returns the command to
    /// the composer.
    pub(super) async fn launch_daemon_workflow(
        &mut self,
        command: String,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.input_ui.set_composer(super::ComposerMode::Input);
        self.submit_daemon_turn(
            TurnPrompt::command(command.clone(), command),
            Vec::new(),
            Vec::new(),
            terminal,
        )
        .await
    }

    /// Starts the workspace workflow at `relative_path`, a hub row, with
    /// its default inputs. The daemon takes a workflow by its command name,
    /// so one whose name cannot be a command is refused here.
    pub(super) async fn start_daemon_workflow_source(
        &mut self,
        relative_path: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let command = discover_workflow_sources(&self.info.runtime.cwd)
            .into_iter()
            .find(|workflow| workflow.relative_path == relative_path)
            .and_then(|workflow| workflow.command_name());
        match command {
            Some(command) => {
                self.launch_daemon_workflow(format!("/{command}"), terminal)
                    .await
            }
            None => {
                self.input_ui.set_composer(super::ComposerMode::Input);
                self.insert_entry(&Entry::Error(format!(
                    "could not start {relative_path}: its name has spaces; rename the file"
                )));
                self.set_status("start failed");
                Ok(())
            }
        }
    }

    /// Puts `jobs`, the followed session's workflow jobs worth showing at
    /// `now` (Unix seconds), on the workflow rail. A run whose saved state
    /// cannot be read has no row.
    pub(super) fn show_daemon_workflows(&mut self, jobs: &[&JobSummary], now: u64) {
        let rows = if jobs.is_empty() {
            Vec::new()
        } else {
            let Ok(ops) = self.workflow_ops() else {
                return;
            };
            jobs.iter()
                .filter_map(|job| {
                    let run = job.workflow_run.as_ref()?.parse::<RunId>().ok()?;
                    let run = ops.load_run_id(run).ok()?;
                    let until = if in_progress(job.state) {
                        now
                    } else {
                        job.updated_at
                    };
                    Some(WorkflowRailSummary::from_stored(
                        &run,
                        job.state == JobState::Failed,
                        until.saturating_sub(job.created_at),
                    ))
                })
                .collect()
        };
        self.workflow_panel.ingest(rows);
    }
}
