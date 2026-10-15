//! Session state mirrored to ACP clients alongside the event stream.
//!
//! Context fill goes out as `usage_update` and the todo checklist as `plan`.
//! Neither is a run event: both are read from session-owned state when an
//! event says it may have moved.

use agent_client_protocol::schema::v1::{
    Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionId, SessionNotification,
    SessionUpdate, UsageUpdate,
};
use rho_sdk::RunEvent;

use super::AcpClientPort;
use crate::tools::todo::{TodoItem, TodoState, TodoStatus};

/// Tracks one prompt's context fill and checklist for a session.
///
/// Build one per prompt, since the session handle changes when the runtime is
/// rebound. The host keeps the last checklist it sent across prompts and hands
/// it in, so a checklist restored by `session/load` goes out with the first
/// prompt. Nothing is sent during load itself: hosts such as T3 Code discard
/// every update for a session while its load is in flight.
pub(super) struct SessionSignals {
    session: rho_sdk::Session,
    todo: TodoState,
    /// Configured window for the current model; the provider-reported window
    /// is the fallback, matching the TUI context meter.
    context_window: Option<u64>,
    sent_plan: Vec<PlanEntry>,
}

impl SessionSignals {
    pub(super) fn new(
        session: rho_sdk::Session,
        todo: TodoState,
        context_window: Option<u64>,
        sent_plan: Vec<PlanEntry>,
    ) -> Self {
        Self {
            session,
            todo,
            context_window,
            sent_plan,
        }
    }

    /// The update a run event triggers, if any. A new step or a compaction
    /// moves context fill; a finished tool may have replaced the checklist,
    /// whether it ran directly or inside code mode.
    pub(super) fn observe(&mut self, event: &RunEvent) -> Option<SessionUpdate> {
        match event {
            RunEvent::StepStarted { .. } | RunEvent::CompactionCompleted { .. } => {
                Some(self.usage())
            }
            RunEvent::ToolFinished { .. } => self.plan_change(),
            _ => None,
        }
    }

    /// Current context fill. ACP's `size` is required; 0 means the window is
    /// unknown, which clients render as no maximum.
    pub(super) fn usage(&self) -> SessionUpdate {
        let estimate = self.session.context_estimate();
        let size = self
            .context_window
            .or(estimate.reported_context_window())
            .unwrap_or(0);
        SessionUpdate::UsageUpdate(UsageUpdate::new(estimate.tokens(), size))
    }

    /// Sends the checklist before the run starts if it differs from what the
    /// client last saw, as after a load restored one.
    pub(super) async fn send_plan_change(
        &mut self,
        session_id: &SessionId,
        client: &dyn AcpClientPort,
    ) {
        if let Some(update) = self.plan_change() {
            send(session_id, client, update).await;
        }
    }

    /// Sends current context fill at turn end, outside the event stream.
    pub(super) async fn send_usage(&self, session_id: &SessionId, client: &dyn AcpClientPort) {
        send(session_id, client, self.usage()).await;
    }

    /// The checklist the client last saw, for the next prompt's tracker.
    pub(super) fn into_sent_plan(self) -> Vec<PlanEntry> {
        self.sent_plan
    }

    fn plan_change(&mut self) -> Option<SessionUpdate> {
        let entries = plan_entries(&self.todo);
        if entries == self.sent_plan {
            return None;
        }
        self.sent_plan.clone_from(&entries);
        Some(SessionUpdate::Plan(Plan::new(entries)))
    }
}

/// Best effort: a closed connection fails the next request instead.
async fn send(session_id: &SessionId, client: &dyn AcpClientPort, update: SessionUpdate) {
    let notification = SessionNotification::new(session_id.clone(), update);
    if let Err(error) = client.send_session_notification(notification).await {
        tracing::debug!(%error, "could not send ACP session update");
    }
}

fn plan_entries(todo: &TodoState) -> Vec<PlanEntry> {
    todo.list()
        .map(|list| list.todos.iter().map(plan_entry).collect())
        .unwrap_or_default()
}

fn plan_entry(item: &TodoItem) -> PlanEntry {
    let status = match item.status {
        TodoStatus::Pending => PlanEntryStatus::Pending,
        TodoStatus::InProgress => PlanEntryStatus::InProgress,
        TodoStatus::Completed => PlanEntryStatus::Completed,
    };
    // The todo tool has no priorities; Medium is the neutral ACP value.
    PlanEntry::new(item.content.clone(), PlanEntryPriority::Medium, status)
}

#[cfg(test)]
#[path = "signals_tests.rs"]
mod tests;
