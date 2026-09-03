//! Registry of in-process delegated agent runs.
//!
//! Spawn, observe, stop, cost accounting, host-input bind, notices, and rail
//! summaries live here so tools call a manager API instead of executor internals.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use {
    crate::agent::AgentDefinition,
    crate::app::{
        agent_executor::{AgentExecutor, AgentLaunchRequest, AgentRunHandle},
        subagent_host_input::{SubagentHostInputBridge, SubagentHostInputRequest},
        subagent_messaging::{SubagentNotice, SubagentNoticeBridge},
    },
    crate::subagent::{self, RunStatus},
};

pub(crate) use super::subagent_messaging::ValidatedMessage;

/// How long host rails keep serving a just-finished row.
///
/// Process and subagent managers both use this. UI linger windows must stay
/// below it so a row can fade before the manager forgets it.
pub(crate) const RAIL_TERMINAL_RETENTION: Duration = Duration::from_secs(10);

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct SubagentSnapshot {
    pub id: String,
    pub agent_id: String,
    pub title: Option<String>,
    pub elapsed: Duration,
    pub status: RunStatus,
    pub done: bool,
}

struct AgentEntry {
    agent_id: String,
    background: bool,
    started: Instant,
    handle: AgentRunHandle,
    session_id: Option<String>,
    observed: bool,
    /// Whether this run's terminal cost has already been folded into a parent
    /// session total. Independent of [`Self::observed`] so cost still counts
    /// when a run is delivered through `status`/`stop` instead of notification.
    cost_accounted: bool,
}

impl AgentEntry {
    fn snapshot(&self, id: &str) -> SubagentSnapshot {
        let status = self.handle.status();
        let elapsed = status
            .elapsed_duration(subagent::unix_now_secs())
            .unwrap_or_else(|| self.started.elapsed());
        SubagentSnapshot {
            id: id.to_string(),
            agent_id: self.agent_id.clone(),
            title: status.title.clone(),
            elapsed,
            done: status.state.is_terminal(),
            status,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SubagentNotification {
    pub snapshot: SubagentSnapshot,
}

#[derive(Clone)]
pub struct SubagentManager {
    inner: Arc<Mutex<HashMap<String, AgentEntry>>>,
    executor: AgentExecutor,
    parent_placement: Arc<Mutex<subagent::RunPlacement>>,
}

impl SubagentManager {
    pub fn new(config: crate::config::Config, config_path: PathBuf, cwd: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            executor: AgentExecutor::new(
                config,
                config_path,
                cwd,
                SubagentHostInputBridge::new(),
                SubagentNoticeBridge::new(),
            ),
            parent_placement: Arc::new(Mutex::new(subagent::RunPlacement::parentless())),
        }
    }

    pub(crate) fn bind_host_input(&self) -> tokio::sync::mpsc::Receiver<SubagentHostInputRequest> {
        self.executor.host_input().bind_parent()
    }

    pub(crate) fn unbind_host_input(&self) {
        self.executor.host_input().unbind_parent();
    }

    /// Atomically replaces the notice binding, retaining in-flight channel notices.
    ///
    /// Pass `None` for the first bind. When rebinding, pass the prior receiver so
    /// posts that already returned `Ok` stay deliverable on the retired generation.
    pub(crate) fn rebind_notices(
        &self,
        old_receiver: Option<tokio::sync::mpsc::Receiver<SubagentNotice>>,
    ) -> crate::app::subagent_messaging::NoticeRebind {
        self.executor.notices().rebind_parent(old_receiver)
    }

    pub(crate) fn unbind_notices(&self) {
        self.executor.notices().unbind_parent();
    }

    pub fn bind_parent_session(&self, placement: subagent::RunPlacement) {
        *self
            .parent_placement
            .lock()
            .expect("delegated session lock") = placement;
    }

    pub fn update_selection(
        &self,
        provider: &str,
        model: &str,
        reasoning: rho_sdk::ReasoningLevel,
        auth: &str,
    ) {
        self.executor
            .update_selection(provider, model, reasoning, auth);
    }

    /// Updates the policy snapshot used by future launches. Already-spawned
    /// agents retain the mode captured when they were launched.
    pub(crate) fn update_permission_mode(&self, mode: crate::permission::PermissionMode) {
        self.executor.update_permission_mode(mode);
    }

    pub(crate) fn concurrency(&self) -> crate::app::agent_concurrency::AgentConcurrency {
        self.executor.concurrency()
    }

    #[cfg(test)]
    pub(crate) fn launch_permission_mode(&self) -> crate::permission::PermissionMode {
        self.executor.launch_permission_mode()
    }

    pub async fn spawn(
        &self,
        definition: &AgentDefinition,
        prompt: &str,
        background: bool,
        cwd: &Path,
    ) -> anyhow::Result<(String, PathBuf)> {
        let placement = self
            .parent_placement
            .lock()
            .expect("delegated session lock")
            .clone();
        let parent_session_id = placement
            .parent_session_id()
            .map(str::to_owned)
            .and_then(|id| rho_sdk::SessionId::from_string(id).ok());
        let session_id = placement.parent_session_id().map(str::to_owned);
        let cwd = cwd.to_path_buf();
        let (id, directory) =
            tokio::task::spawn_blocking(move || subagent::reserve_run_directory(&placement, &cwd))
                .await??;
        let output_file = directory.join(subagent::RESULT_FILE_NAME);
        let launch = AgentLaunchRequest {
            definition: Arc::new(definition.clone()),
            prompt: prompt.to_string(),
            run_id: id.clone(),
            parent_session_id,
            output_file,
        };
        let handle = match self.executor.spawn(launch) {
            Ok(handle) => handle,
            Err(error) => {
                let cleanup_id = id.clone();
                let cleanup_directory = directory.clone();
                let cleanup = tokio::task::spawn_blocking(move || {
                    subagent::release_run_directory(&cleanup_id, &cleanup_directory)
                })
                .await?;
                if let Err(cleanup_error) = cleanup {
                    return Err(anyhow::anyhow!(
                        "{error}; failed to clean up delegated run reservation: {cleanup_error}"
                    ));
                }
                return Err(error);
            }
        };
        self.inner.lock().expect("delegated registry lock").insert(
            id.clone(),
            AgentEntry {
                agent_id: definition.id.to_string(),
                background,
                started: Instant::now(),
                handle,
                session_id,
                observed: false,
                cost_accounted: false,
            },
        );
        Ok((id, directory.join(subagent::LOG_FILE_NAME)))
    }

    /// Fold terminal costs for `session_id` into a parent total once per run.
    ///
    /// Counts every finished run (background or foreground, success or failure)
    /// the first time the parent claims it. Safe to call from any TUI poll path.
    pub fn claim_terminal_costs_usd_micros(&self, session_id: &str) -> u64 {
        let mut entries = self.inner.lock().expect("delegated registry lock");
        let mut total = 0u64;
        for entry in entries.values_mut() {
            if entry.cost_accounted || entry.session_id.as_deref() != Some(session_id) {
                continue;
            }
            let status = entry.handle.status();
            if !status.state.is_terminal() {
                continue;
            }
            entry.cost_accounted = true;
            if let Some(cost) = status.total_cost_usd {
                total = total.saturating_add(subagent::usd_to_micros(cost));
            }
        }
        total
    }

    #[cfg(test)]
    pub(crate) fn insert_completed_for_test(
        &self,
        id: &str,
        session_id: &str,
        total_cost_usd: Option<f64>,
    ) {
        self.insert_completed_status_for_test(
            id,
            session_id,
            crate::subagent::RunStatus {
                state: crate::subagent::RunState::Ok,
                total_cost_usd,
                ..crate::subagent::RunStatus::default()
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn insert_completed_status_for_test(
        &self,
        id: &str,
        session_id: &str,
        status: crate::subagent::RunStatus,
    ) {
        self.inner.lock().expect("delegated registry lock").insert(
            id.to_string(),
            AgentEntry {
                agent_id: "fixture".into(),
                background: true,
                started: Instant::now(),
                handle: AgentRunHandle::completed_for_test(status),
                session_id: Some(session_id.into()),
                observed: false,
                cost_accounted: false,
            },
        );
    }

    pub fn status(&self, id: &str) -> Option<SubagentSnapshot> {
        let id = crate::subagent::normalize_id(id).ok()?;
        self.inner
            .lock()
            .expect("delegated registry lock")
            .get(&id)
            .map(|entry| entry.snapshot(&id))
    }

    pub fn list(&self) -> Vec<SubagentSnapshot> {
        let entries = self.inner.lock().expect("delegated registry lock");
        let mut snapshots = entries
            .iter()
            .map(|(id, entry)| entry.snapshot(id))
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.elapsed));
        snapshots
    }

    /// Live runs plus terminals with a recent `finished_at` stamp.
    ///
    /// Host UI only. `list()` stays the full agent-facing registry.
    pub(crate) fn rail_summaries(&self) -> Vec<SubagentSnapshot> {
        let now = subagent::unix_now_secs();
        let retention = RAIL_TERMINAL_RETENTION.as_secs();
        let entries = self.inner.lock().expect("delegated registry lock");
        let mut snapshots = entries
            .iter()
            .filter_map(|(id, entry)| {
                let snapshot = entry.snapshot(id);
                if snapshot.done {
                    let finished_at = snapshot.status.finished_at?;
                    if now.saturating_sub(finished_at) >= retention {
                        return None;
                    }
                }
                Some(snapshot)
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.elapsed));
        snapshots
    }

    pub fn has_running_for_session(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .expect("delegated registry lock")
            .values()
            .any(|entry| {
                entry.session_id.as_deref() == Some(session_id) && !entry.handle.is_complete()
            })
    }

    pub fn has_active_or_pending_notification(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .expect("delegated registry lock")
            .values()
            .any(|entry| {
                !entry.handle.is_complete()
                    || (entry.session_id.as_deref() == Some(session_id)
                        && entry.background
                        && !entry.observed)
            })
    }

    /// Atomically drains every unobserved terminal background run for the
    /// session and marks the whole batch observed, in launch order so batched
    /// delivery is deterministic.
    pub fn take_notifications(&self, session_id: &str) -> Vec<SubagentNotification> {
        let mut entries = self.inner.lock().expect("delegated registry lock");
        let mut notifications = entries
            .iter_mut()
            .filter_map(|(id, entry)| {
                let snapshot = entry.snapshot(id);
                (entry.background
                    && snapshot.done
                    && !entry.observed
                    && entry.session_id.as_deref() == Some(session_id))
                .then(|| {
                    entry.observed = true;
                    (entry.started, SubagentNotification { snapshot })
                })
            })
            .collect::<Vec<_>>();
        notifications.sort_by(|(a_started, a), (b_started, b)| {
            a_started
                .cmp(b_started)
                .then_with(|| a.snapshot.id.cmp(&b.snapshot.id))
        });
        notifications
            .into_iter()
            .map(|(_, notification)| notification)
            .collect()
    }

    /// Returns drained notifications so a failed turn setup can deliver them
    /// again. Only terminal background entries still present are reopened.
    pub fn restore_notifications(&self, notifications: &[SubagentNotification]) {
        if notifications.is_empty() {
            return;
        }
        let mut entries = self.inner.lock().expect("delegated registry lock");
        for notification in notifications {
            let Some(entry) = entries.get_mut(&notification.snapshot.id) else {
                continue;
            };
            let snapshot = entry.snapshot(&notification.snapshot.id);
            if entry.background && snapshot.done && entry.observed {
                entry.observed = false;
            }
        }
    }

    /// Returns the run snapshot; a terminal snapshot counts as delivered, so
    /// automatic notification will not repeat a result the parent already
    /// read through `status` or `stop`.
    pub fn observe(&self, id: &str) -> Option<SubagentSnapshot> {
        let id = crate::subagent::normalize_id(id).ok()?;
        let mut entries = self.inner.lock().expect("delegated registry lock");
        let entry = entries.get_mut(&id)?;
        let snapshot = entry.snapshot(&id);
        if snapshot.done {
            entry.observed = true;
        }
        Some(snapshot)
    }

    pub async fn wait_done(&self, id: &str) -> Option<SubagentSnapshot> {
        let id = crate::subagent::normalize_id(id).ok()?;
        let mut handle = self
            .inner
            .lock()
            .expect("delegated registry lock")
            .get(&id)?
            .handle
            .clone();
        handle.wait().await;
        self.status(&id)
    }

    pub async fn stop(&self, id: &str) -> anyhow::Result<SubagentSnapshot> {
        let id = crate::subagent::normalize_id(id)?;
        let mut handle = self
            .inner
            .lock()
            .expect("delegated registry lock")
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown delegated run '{id}'"))?
            .handle
            .clone();
        handle.cancel();
        tokio::time::timeout(SHUTDOWN_TIMEOUT, handle.wait())
            .await
            .map_err(|_| anyhow::anyhow!("timed out stopping delegated run '{id}'"))?;
        // Stopping hands the terminal snapshot to the caller, so it counts
        // as delivered and is not repeated by automatic notification.
        self.observe(&id)
            .ok_or_else(|| anyhow::anyhow!("delegated run '{id}' disappeared"))
    }

    /// Stages a parent plain-text message for a running delegated agent.
    pub(crate) async fn message(&self, id: &str, message: &ValidatedMessage) -> anyhow::Result<()> {
        let id = crate::subagent::normalize_id(id)?;
        let handle = self
            .inner
            .lock()
            .expect("delegated registry lock")
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown delegated run '{id}'"))?
            .handle
            .clone();
        handle.message_from_parent(message).await
    }

    pub async fn shutdown(&self) {
        let handles = self
            .inner
            .lock()
            .expect("delegated registry lock")
            .values()
            .map(|entry| entry.handle.clone())
            .collect::<Vec<_>>();
        for handle in &handles {
            handle.cancel();
        }
        let waits = handles.into_iter().map(|mut handle| async move {
            handle.wait().await;
        });
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, futures_util::future::join_all(waits)).await;
    }
}
