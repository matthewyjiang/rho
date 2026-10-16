//! Prepare one turn-boundary delivery without coupling display policy to scheduling.

use crate::{
    app::{boundary_results::BoundaryResults, interactive_runtime::ComputerNotice},
    display_transcript::{DisplayRow, DisplayTranscript},
};

use super::InteractiveRuntime;

/// Drained work held until an accepted provider start or runtime checkpoint.
#[derive(Default)]
pub(super) struct TurnBoundaryBatch {
    pub(super) runtime_context: Option<ComputerNotice>,
    pub(super) subagent_notifications: Vec<crate::tools::agent::SubagentNotification>,
    pub(super) notices: Vec<crate::app::subagent_messaging::SubagentNotice>,
    pub(super) workflow_notifications: Vec<crate::tools::workflow_tracker::WorkflowNotification>,
    pub(super) process_notifications: Vec<crate::tools::process::ProcessNotification>,
}

impl TurnBoundaryBatch {
    pub(super) fn requires_parent_action(&self) -> bool {
        self.notices
            .iter()
            .any(|notice| notice.delivery.requires_parent_action())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.runtime_context.is_none()
            && self.notices.is_empty()
            && self.subagent_notifications.is_empty()
            && self.workflow_notifications.is_empty()
            && self.process_notifications.is_empty()
    }

    /// Resolve task identity once so saved and live cards describe the same delivery.
    pub(super) fn prepare(self, agent: &InteractiveRuntime) -> TurnBoundaryDelivery {
        let mut message = BoundaryResults {
            notices: &self.notices,
            subagents: &self.subagent_notifications,
            workflows: &self.workflow_notifications,
            processes: &self.process_notifications,
        }
        .render(agent.subagents());
        if let Some(notice) = &self.runtime_context {
            message.model.push(notice.model.clone());
            message
                .rows
                .push(DisplayRow::Notice(notice.display.clone()));
        }
        TurnBoundaryDelivery {
            model: message.model.join("\n\n"),
            transcript: DisplayTranscript(message.rows),
            batch: self,
        }
    }

    pub(super) fn notice_count(&self) -> usize {
        self.notices.len()
    }
}

/// Prepared prompts and display plus the restorable drained work for one boundary.
pub(super) struct TurnBoundaryDelivery {
    pub(super) model: String,
    pub(super) transcript: DisplayTranscript,
    pub(super) batch: TurnBoundaryBatch,
}
