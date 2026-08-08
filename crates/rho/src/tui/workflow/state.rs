use std::{collections::BTreeMap, path::PathBuf};

use crate::workflow::NodeId;

use super::{
    control::{control_policy, ControlPolicy},
    dag::{self, SpatialDirection},
    dag_pane::DagPane,
    details::DetailPane,
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
    details: DetailPane,
    dag_pane: DagPane,
}

impl WorkflowUiState {
    pub(super) fn new(
        session: WorkflowSession,
        snapshot: WorkflowSnapshot,
        run_directory: Option<PathBuf>,
    ) -> Self {
        let mut state = Self {
            session,
            snapshot,
            selected: 0,
            progress: BTreeMap::new(),
            notice: None,
            details: DetailPane::default(),
            dag_pane: DagPane::default(),
        };
        state.details.set_run_directory(run_directory);
        state.refresh_details(/*reset_scroll*/ true);
        state
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

    /// Move the selection to the spatially nearest node on the rendered
    /// canvas. Keyboard navigation returns the viewport to follow mode.
    pub(super) fn select_spatial(&mut self, direction: SpatialDirection) {
        // The pane holds last-draw geometry, so a shrinking snapshot can leave
        // a neighbor index past the current node list.
        let next = dag::spatial_neighbor(self.dag_pane.node_rects(), self.selected, direction)
            .or_else(|| self.index_fallback(direction))
            .filter(|&index| index < self.snapshot.nodes.len());
        self.dag_pane.clear_manual_offset();
        if let Some(index) = next.filter(|&index| index != self.selected) {
            self.selected = index;
            self.refresh_details(/*reset_scroll*/ true);
        }
    }

    /// Keep Up and Down usable before the first draw or when the graph is too
    /// large to render and no node geometry exists.
    fn index_fallback(&self, direction: SpatialDirection) -> Option<usize> {
        if !self.dag_pane.node_rects().is_empty() {
            return None;
        }
        match direction {
            SpatialDirection::Up => self.selected.checked_sub(1),
            SpatialDirection::Down => {
                (self.selected + 1 < self.snapshot.nodes.len()).then(|| self.selected + 1)
            }
            SpatialDirection::Left | SpatialDirection::Right => None,
        }
    }

    /// Select a node picked with the mouse without moving the user's view.
    pub(super) fn select_index(&mut self, index: usize) {
        if index < self.snapshot.nodes.len() && index != self.selected {
            self.selected = index;
            self.refresh_details(/*reset_scroll*/ true);
        }
    }

    pub(super) fn dag_pane_mut(&mut self) -> &mut DagPane {
        &mut self.dag_pane
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
