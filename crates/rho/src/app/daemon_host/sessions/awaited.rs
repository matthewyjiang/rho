//! The background work a session waits on, whose results the daemon (see
//! `idle_wake`) or the session's host (see `t3_wake`) starts a turn for once
//! they finish while the session is idle.
//!
//! Tasks count; services never do: a background process started as a
//! service neither wakes the session nor keeps it from settling (see
//! `settle`). A session whose prompts hold until their work finishes
//! (`BackgroundWait::HoldPrompt`) waits on nothing between turns.
//!
//! A turn the session started in another session through session control
//! counts too (see `peer_wake`): it runs until it ends, and its notice is a
//! result.

use std::future::Future;

use super::background::Sources;
use crate::{
    app::{
        peer_sessions::PeerInbox, pull_requests::PullRequestInbox, session_runtime::BackgroundWait,
        subagent_manager::SubagentManager,
    },
    tools::{
        process::{LiveProcessSummary, ProcessManager},
        ui::SurfaceHost,
        workflow_tracker::{WorkflowRailSummary, WorkflowRunTracker},
    },
};

/// The work one session waits on.
#[derive(Clone)]
pub(super) struct AwaitedWork {
    session_id: String,
    processes: Option<ProcessManager>,
    workflows: WorkflowRunTracker,
    /// The session's subagents, when the daemon's own turns deliver them
    /// (`BackgroundWait::HostTurns`): one that finished, and a child's
    /// request that the parent act, are results.
    subagents: Option<SubagentManager>,
    /// The session's A2UI surfaces, where the daemon starts a turn for what
    /// the user does on one.
    surfaces: Option<SurfaceHost>,
    /// The session's watched pull requests, whose news is a result. A quiet
    /// watch is neither running work nor a result: it never keeps the
    /// session active or loaded.
    pull_requests: Option<PullRequestInbox>,
    /// Notices of turns the session started in other sessions, which are
    /// results; such a turn still under way is running work.
    peers: Option<PeerInbox>,
}

impl AwaitedWork {
    /// The work a session that waits as `wait` is woken for, from its
    /// tools' managers; `None` for a session nothing wakes.
    pub(super) fn new(
        wait: BackgroundWait,
        session_id: String,
        workflows: WorkflowRunTracker,
        sources: Sources,
    ) -> Option<Self> {
        let Sources {
            processes,
            subagents,
            surfaces,
            pull_requests,
            peers,
        } = sources;
        let (subagents, surfaces, pull_requests, peers) = match wait {
            BackgroundWait::HoldPrompt => return None,
            // The host runs the session's children as its own threads and
            // reports them itself, and its client draws no surfaces.
            BackgroundWait::HostWakes => (None, None, None, None),
            BackgroundWait::HostTurns => (subagents, surfaces, pull_requests, peers),
        };
        Some(Self {
            session_id,
            processes,
            workflows,
            subagents,
            surfaces,
            pull_requests,
            peers,
        })
    }

    /// Resolves once a result may have arrived from now on, whatever waits
    /// already. A service's exit, or a child's informational notice,
    /// resolves it too, though neither is a result: callers look again with
    /// [`Self::has_result`].
    pub(super) fn next_result(&self) -> impl Future<Output = ()> {
        // Subscribed here, not at the first poll.
        let process_exit = self.processes.as_ref().map(ProcessManager::notified_owned);
        let workflow_finish = self.workflows.notified_owned();
        let subagent = self.subagents.as_ref().map(SubagentManager::parent_wake);
        let surface_action = self.surfaces.as_ref().map(SurfaceHost::acted_owned);
        let peer = self.peers.as_ref().map(PeerInbox::arrived_owned);
        let news = self.outside_news();
        async move {
            tokio::select! {
                () = or_pending(process_exit) => {}
                () = workflow_finish => {}
                () = or_pending(subagent) => {}
                () = or_pending(surface_action) => {}
                () = or_pending(peer) => {}
                () = news => {}
            }
        }
    }

    /// Resolves once pull request news may have arrived, which, unlike every
    /// other result, needs no turn of the session's to start it.
    pub(super) fn outside_news(&self) -> impl Future<Output = ()> + Send + 'static {
        let news = self
            .pull_requests
            .as_ref()
            .map(PullRequestInbox::news_owned);
        or_pending(news)
    }

    /// Whether a result is waiting or could still arrive. A workflow counts
    /// until its terminal snapshot is published, which follows its last
    /// runtime event; a subagent from its start until a turn takes its
    /// result, under one lock, so one that just finished is never missed.
    pub(super) fn outstanding(&self) -> bool {
        // Live first, results last: a task only goes from live to finished
        // to delivered, so one that finishes between the two looks is still
        // counted by `has_result`.
        self.processes
            .as_ref()
            .is_some_and(ProcessManager::has_live_task)
            || self
                .workflows
                .has_active_or_pending_notification(&self.session_id)
            || self.subagents.as_ref().is_some_and(|subagents| {
                subagents.has_active_or_pending_notification(&self.session_id)
            })
            || self.peers.as_ref().is_some_and(PeerInbox::expecting)
            || self.has_result()
    }

    /// Whether work that may still give a result runs: the complement, among
    /// the work not yet delivered, of [`Self::has_result`].
    pub(super) fn running(&self) -> bool {
        self.processes
            .as_ref()
            .is_some_and(ProcessManager::has_live_task)
            || self.workflows.has_live(&self.session_id)
            || self
                .subagents
                .as_ref()
                .is_some_and(|subagents| subagents.has_running(&self.session_id))
            || self.peers.as_ref().is_some_and(PeerInbox::expecting)
    }

    /// The session's tasks still running or holding a result not yet
    /// delivered, oldest first; never its services.
    pub(super) fn tasks(&self) -> Vec<LiveProcessSummary> {
        self.processes
            .iter()
            .flat_map(ProcessManager::task_summaries)
            .collect()
    }

    /// The session's workflow runs still running or holding a result not
    /// yet delivered, oldest first.
    pub(super) fn workflows(&self) -> Vec<WorkflowRailSummary> {
        self.workflows.rail_summaries(&self.session_id)
    }

    /// Whether a finished result is waiting for the model.
    pub(super) fn has_result(&self) -> bool {
        self.processes
            .as_ref()
            .is_some_and(ProcessManager::has_task_result)
            || self.workflows.has_pending_notification(&self.session_id)
            || self.subagents.as_ref().is_some_and(|subagents| {
                subagents.has_pending_notification(&self.session_id)
                    || subagents
                        .undelivered_notices(&self.session_id)
                        .iter()
                        .any(|notice| notice.delivery.requires_parent_action())
            })
            || self.surfaces.as_ref().is_some_and(SurfaceHost::has_action)
            || self
                .pull_requests
                .as_ref()
                .is_some_and(PullRequestInbox::has_news)
            || self.peers.as_ref().is_some_and(PeerInbox::has_notices)
    }

    /// Resolves once a result is waiting.
    pub(super) async fn result_ready(self) {
        loop {
            // Subscribe before checking so a finish in between still wakes.
            let another = self.next_result();
            if self.has_result() {
                return;
            }
            #[cfg(debug_assertions)]
            if let Some(subagents) = &self.subagents {
                crate::app::quiet_notice_signal::parent_looked(
                    subagents.undelivered_notices(&self.session_id).len(),
                );
            }
            another.await;
        }
    }
}

/// Resolves as `wake` does, or never without one.
async fn or_pending(wake: Option<impl Future<Output = ()>>) {
    match wake {
        Some(wake) => wake.await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "awaited_tests.rs"]
mod tests;
