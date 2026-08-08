use std::{collections::BTreeMap, path::PathBuf};

use crate::workflow::NodeId;

use super::{
    control::{control_policy, ControlPolicy},
    dag::{self, DagRender, SpatialDirection},
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

    /// Render the graph for the current snapshot. Node labels carry the
    /// freshest progress message that matches the node's current attempt;
    /// `render_dag` falls back to the node's own work text while it runs.
    pub(super) fn render_dag(&self) -> DagRender {
        let activities = self
            .snapshot
            .nodes
            .iter()
            .map(|node| self.progress(node).map(|progress| progress.message.clone()))
            .collect::<Vec<_>>();
        dag::render_dag(&self.snapshot.nodes, self.selected, &activities)
    }

    /// Move the selection to the spatially nearest node on the rendered
    /// canvas. Keyboard navigation returns the viewport to follow mode.
    pub(super) fn select_spatial(&mut self, direction: SpatialDirection) {
        let rects = self.render_dag().node_rects;
        let next = if rects.is_empty() {
            self.index_fallback(direction)
        } else {
            dag::spatial_neighbor(&rects, self.selected, direction)
        };
        self.dag_pane.clear_manual_offset();
        if let Some(index) = next.filter(|&index| index != self.selected) {
            self.selected = index;
            self.refresh_details(/*reset_scroll*/ true);
        }
    }

    /// Keep Up and Down usable when the graph renders no node geometry
    /// because it exceeds the render budget.
    fn index_fallback(&self, direction: SpatialDirection) -> Option<usize> {
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
