//! The subagent rail of a daemon session: the status of each run the
//! daemon's jobs name (`daemon_jobs`), read from the run's artifact
//! directory, which the daemon writes under the rho home this process
//! shares. `/attach` then opens a run's artifacts from the same directory.
//! A finished run's cost there joins the session's subagent cost, as the
//! in-process subagent manager's does (`app_loop.rs`).

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
    /// The finished runs whose cost the session's subagent cost counts.
    costed: HashSet<String>,
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

    /// What `run`, as `status` has it, adds to the session's subagent cost
    /// in USD micros: its cost the first time it is seen finished, else
    /// nothing.
    fn claim_cost(&mut self, run: &str, status: &RunStatus) -> u64 {
        if !status.state.is_terminal() || !self.costed.insert(run.to_owned()) {
            return 0;
        }
        status.total_cost_usd.map_or(0, subagent::usd_to_micros)
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
                let runs = &mut self.daemon_rail.subagents;
                let status = runs.status(&run)?;
                let cost = runs.claim_cost(&run, &status);
                self.usage.extra.subagent = self.usage.extra.subagent.saturating_add(cost);
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
        let Some(status) = runs.status(run).filter(|status| status.state.is_terminal()) else {
            return;
        };
        // Its row leaves before a job list may see it finished.
        let cost = runs.claim_cost(run, &status);
        self.usage.extra.subagent = self.usage.extra.subagent.saturating_add(cost);
        runs.delivered.insert(run.clone());
        runs.shown.retain(|shown| &shown.id != run);
        let shown = runs.shown.clone();
        if self.subagent_panel.ingest(shown) {
            self.refresh_attach_picker();
        }
    }
}
