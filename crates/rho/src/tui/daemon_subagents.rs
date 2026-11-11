//! The subagent rail of a daemon session: the status of each run the
//! daemon's jobs name (`daemon_jobs`), read from the run's artifact
//! directory, which the daemon writes under the rho home this process
//! shares. `/attach` then opens a run's artifacts from the same directory.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use rho_protocol::{display::NotificationCard, JobSummary};

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
    /// The finished runs whose result the transcript shows delivered:
    /// their rows left the rail with it, as in process.
    delivered: HashSet<String>,
    /// The rows the last job list put on the rail.
    shown: Vec<SubagentSnapshot>,
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
        let snapshots: Vec<_> = jobs
            .iter()
            .filter_map(|job| job.run.clone())
            .filter_map(|run| {
                if self.daemon_rail.subagents.delivered.contains(&run) {
                    return None;
                }
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
        self.daemon_rail.subagents.shown.clone_from(&snapshots);
        if self.subagent_panel.ingest(snapshots) {
            self.refresh_attach_picker();
        }
    }

    /// The transcript shows `card`, a result or message the model was
    /// given. When it names a finished run, that run's result is delivered,
    /// so its row leaves the rail now, as an in-process row does, instead
    /// of once its retention passed.
    pub(super) fn daemon_result_delivered(&mut self, card: &NotificationCard) {
        let Some(run) = &card.reference else {
            return;
        };
        let runs = &mut self.daemon_rail.subagents;
        // A running subagent's message names its run too; its row stays.
        if !runs
            .status(run)
            .is_some_and(|status| status.state.is_terminal())
        {
            return;
        }
        runs.delivered.insert(run.clone());
        runs.shown.retain(|shown| &shown.id != run);
        let shown = runs.shown.clone();
        if self.subagent_panel.ingest(shown) {
            self.refresh_attach_picker();
        }
    }
}
