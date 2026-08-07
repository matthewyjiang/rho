use std::{collections::BTreeMap, path::PathBuf};

use crate::workflow::NodeId;

use super::{
    control::{control_policy, ControlPolicy},
    dag::{self, HorizontalDirection},
    details::DetailPane,
    event_adapter::{
        WorkflowEvent, WorkflowNodeSnapshot, WorkflowProgress, WorkflowSession, WorkflowSnapshot,
    },
};

pub(super) struct WorkflowUiState {
    session: WorkflowSession,
    snapshot: WorkflowSnapshot,
    selected: usize,
    node_ranks: Vec<usize>,
    progress: BTreeMap<NodeId, WorkflowProgress>,
    notice: Option<String>,
    details: DetailPane,
}

impl WorkflowUiState {
    pub(super) fn new(
        session: WorkflowSession,
        snapshot: WorkflowSnapshot,
        run_directory: Option<PathBuf>,
    ) -> Self {
        let node_ranks = dag::node_ranks(&snapshot.nodes);
        let mut state = Self {
            session,
            snapshot,
            selected: 0,
            node_ranks,
            progress: BTreeMap::new(),
            notice: None,
            details: DetailPane::default(),
        };
        state.details.set_run_directory(run_directory);
        state.refresh_details(/*reset_scroll*/ true);
        state
    }

    pub(super) fn apply(&mut self, event: WorkflowEvent) {
        match event {
            WorkflowEvent::Snapshot(snapshot) => {
                let previous_id = self.selected_node().map(|node| node.id.clone());
                self.node_ranks = dag::node_ranks(&snapshot.nodes);
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
                        self.refresh_details(/*reset_scroll*/ false);
                        return;
                    }
                }
                if let Some(index) = self.snapshot.nodes.iter().position(|node| {
                    matches!(node.state, crate::workflow::NodeState::Running { .. })
                }) {
                    self.selected = index;
                }
                self.refresh_details(/*reset_scroll*/ true);
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

    pub(super) fn details(&self) -> &DetailPane {
        &self.details
    }

    pub(super) fn details_mut(&mut self) -> &mut DetailPane {
        &mut self.details
    }

    pub(super) fn select_previous(&mut self) {
        let previous = self.selected;
        self.selected = self.selected.saturating_sub(1);
        if self.selected != previous {
            self.refresh_details(/*reset_scroll*/ true);
        }
    }

    pub(super) fn select_next(&mut self) {
        if self.selected + 1 < self.snapshot.nodes.len() {
            self.selected += 1;
            self.refresh_details(/*reset_scroll*/ true);
        }
    }

    pub(super) fn select_horizontal(&mut self, direction: HorizontalDirection) {
        if let Some(index) = dag::horizontal_neighbor(&self.node_ranks, self.selected, direction) {
            self.selected = index;
            self.refresh_details(/*reset_scroll*/ true);
        }
    }

    pub(super) fn can_exit(&self) -> bool {
        self.policy().can_leave
    }

    fn refresh_details(&mut self, reset_scroll: bool) {
        // Avoid borrowing selected node while mutating details via split fields.
        let node = self.snapshot.nodes.get(self.selected).cloned();
        self.details.refresh(node.as_ref(), reset_scroll);
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
