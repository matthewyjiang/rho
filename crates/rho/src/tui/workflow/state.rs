use std::collections::BTreeMap;

use crate::workflow::{NodeId, NodeState, NodeTerminalState, RunLifecycle};

use super::event_adapter::{
    PlanApprovalState, WorkflowEvent, WorkflowNodeSnapshot, WorkflowProgress, WorkflowSnapshot,
};

pub(super) struct WorkflowUiState {
    snapshot: WorkflowSnapshot,
    selected: usize,
    progress: BTreeMap<NodeId, WorkflowProgress>,
    notice: Option<String>,
}

impl WorkflowUiState {
    pub(super) fn new(snapshot: WorkflowSnapshot) -> Self {
        Self {
            snapshot,
            selected: 0,
            progress: BTreeMap::new(),
            notice: None,
        }
    }

    pub(super) fn apply(&mut self, event: WorkflowEvent) {
        match event {
            WorkflowEvent::Snapshot(snapshot) => {
                self.snapshot = snapshot;
                self.selected = self
                    .selected
                    .min(self.snapshot.nodes.len().saturating_sub(1));
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

    pub(super) fn approval(&self) -> PlanApprovalState {
        self.snapshot.approval
    }

    pub(super) fn can_exit(&self) -> bool {
        self.snapshot.exit_is_safe
    }

    pub(super) fn lifecycle(&self) -> RunLifecycle {
        self.snapshot.lifecycle
    }

    pub(super) fn counts(&self) -> StateCounts {
        let mut counts = StateCounts::default();
        for node in &self.snapshot.nodes {
            match &node.state {
                NodeState::Pending => counts.pending += 1,
                NodeState::Ready => counts.ready += 1,
                NodeState::Running { .. } => counts.running += 1,
                NodeState::Terminal { outcome } => match outcome {
                    NodeTerminalState::Success => counts.success += 1,
                    NodeTerminalState::Failure => counts.failure += 1,
                    NodeTerminalState::Denial => counts.denial += 1,
                    NodeTerminalState::Cancellation => counts.cancelled += 1,
                    NodeTerminalState::Skipped => counts.skipped += 1,
                    NodeTerminalState::Blocked => counts.blocked += 1,
                },
            }
        }
        counts
    }
}

#[derive(Default)]
pub(super) struct StateCounts {
    pub(super) pending: usize,
    pub(super) ready: usize,
    pub(super) running: usize,
    pub(super) success: usize,
    pub(super) failure: usize,
    pub(super) denial: usize,
    pub(super) cancelled: usize,
    pub(super) skipped: usize,
    pub(super) blocked: usize,
}
