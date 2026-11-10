//! The subagent rail of a daemon session: the status of each run the
//! daemon's jobs name (`daemon_jobs`), read from the run's artifact
//! directory, which the daemon writes under the rho home this process
//! shares. `/attach` then opens a run's artifacts from the same directory.

use std::{collections::HashMap, path::PathBuf};

use rho_protocol::JobSummary;

use super::{daemon_jobs::ENDED_RETENTION, App};
use crate::{
    session::SessionPersistence,
    subagent::{self, RunStatus},
    tools::agent::SubagentSnapshot,
};

/// What the subagent rail remembers between job lists.
#[derive(Default)]
pub(super) struct SubagentRuns {
    /// Each run's artifact directory, resolved once.
    directories: HashMap<String, PathBuf>,
}

impl SubagentRuns {
    /// The status of `run`, read from its artifact directory.
    fn status(&mut self, run: &str) -> Option<RunStatus> {
        if !self.directories.contains_key(run) {
            let directory = subagent::resolve_run_directory(run).ok()?;
            self.directories.insert(run.to_owned(), directory);
        }
        subagent::read_status(&self.directories[run].join(subagent::RESULT_FILE_NAME))
    }
}

impl App {
    /// Puts `jobs`, the followed session's subagent jobs worth showing at
    /// `now` (Unix seconds), on the subagent rail. A run whose status
    /// cannot be read has no row.
    pub(super) fn show_daemon_subagents(&mut self, jobs: &[&JobSummary], now: u64) {
        let snapshots = jobs
            .iter()
            .filter_map(|job| job.run.clone())
            .filter_map(|run| {
                let status = self.daemon_rail.subagents.status(&run)?;
                let done = status.state.is_terminal();
                let ended_long_ago = status.finished_at.is_some_and(|finished| {
                    now.saturating_sub(finished) >= ENDED_RETENTION.as_secs()
                });
                (!done || !ended_long_ago).then(|| SubagentSnapshot {
                    id: run,
                    agent_id: status.agent_id.clone().unwrap_or_else(|| "agent".into()),
                    title: status.title.clone(),
                    elapsed: status.elapsed_duration(now).unwrap_or_default(),
                    done,
                    prior_notices: Vec::new(),
                    // The rail does not show it.
                    persistence: SessionPersistence::Saved,
                    status,
                })
            })
            .collect();
        if self.subagent_panel.ingest(snapshots) {
            self.refresh_attach_picker();
        }
    }
}
