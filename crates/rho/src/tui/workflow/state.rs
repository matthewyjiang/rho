use std::collections::BTreeMap;

use crate::workflow::NodeId;

use super::{
    control::{control_policy, ControlPolicy},
    event_adapter::{
        WorkflowEvent, WorkflowNodeSnapshot, WorkflowProgress, WorkflowSession, WorkflowSnapshot,
    },
};

pub(super) struct WorkflowUiState {
    session: WorkflowSession,
    snapshot: WorkflowSnapshot,
    selected: usize,
    progress: BTreeMap<NodeId, WorkflowProgress>,
    notice: Option<String>,
}

impl WorkflowUiState {
    pub(super) fn new(session: WorkflowSession, snapshot: WorkflowSnapshot) -> Self {
        Self {
            session,
            snapshot,
            selected: 0,
            progress: BTreeMap::new(),
            notice: None,
        }
    }

    pub(super) fn apply(&mut self, event: WorkflowEvent) {
        match event {
            WorkflowEvent::Snapshot(snapshot) => {
                let previous_id = self.selected_node().map(|node| node.id.clone());
                self.snapshot = snapshot;
                self.selected = self
                    .selected
                    .min(self.snapshot.nodes.len().saturating_sub(1));
                // Keep the user on the same node when possible; otherwise prefer work in flight.
                if let Some(previous_id) = previous_id {
                    if let Some(index) = self
                        .snapshot
                        .nodes
                        .iter()
                        .position(|node| node.id == previous_id)
                    {
                        self.selected = index;
                        return;
                    }
                }
                if let Some(index) = self.snapshot.nodes.iter().position(|node| {
                    matches!(node.state, crate::workflow::NodeState::Running { .. })
                }) {
                    self.selected = index;
                }
            }
            WorkflowEvent::Progress { node, progress } => {
                self.progress.insert(node, progress);
            }
            WorkflowEvent::Notice(notice) => self.notice = Some(notice),
        }
    }

    pub(super) fn snapshot(&self) -> &WorkflowSnapshot {
        &self.snapshot
    }

    pub(super) fn policy(&self) -> ControlPolicy {
        control_policy(self.session, &self.snapshot)
    }

    pub(super) fn session(&self) -> WorkflowSession {
        self.session
    }

    pub(super) fn selected_node(&self) -> Option<&WorkflowNodeSnapshot> {
        self.snapshot.nodes.get(self.selected)
    }

    pub(super) fn selected_index(&self) -> usize {
        self.selected
    }

    pub(super) fn progress(&self, node: &WorkflowNodeSnapshot) -> Option<&WorkflowProgress> {
        self.progress
            .get(&node.id)
            .filter(|progress| Some(progress.attempt) == node.current_attempt)
    }

    pub(super) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub(super) fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub(super) fn select_next(&mut self) {
        if self.selected + 1 < self.snapshot.nodes.len() {
            self.selected += 1;
        }
    }

    pub(super) fn can_exit(&self) -> bool {
        self.policy().can_leave
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
