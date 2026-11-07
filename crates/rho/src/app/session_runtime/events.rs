//! What a running turn reports to its host, and what the host answers.
//!
//! A turn sends [`SessionEvent`]s in the order things happen and reads
//! [`TurnControl`]s for the requests it raised. Each request names its own
//! id, so the host may answer in any order, and stops waiting on one when its
//! matching withdrawal arrives.

use rho_sdk::{
    ApprovalDecision, ApprovalRequest, ContextEstimate, HostInputId, HostInputRefusal,
    HostInputRequest, HostInputResponse, RunEvent, ToolCallId,
};

use crate::{
    display_transcript::DisplayTranscript, permission::PermissionMode,
    session::tree::DisplayRowOrigin, tools::plan_exit::PlanPermissionAnswer,
};

/// One tool approval a turn asked for, unique within the turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ApprovalId(pub(super) u64);

#[cfg(test)]
impl ApprovalId {
    /// The id a turn gives its `n`th approval, counting from 0, for host
    /// tests that play the turn.
    pub(crate) fn nth_for_tests(n: u64) -> Self {
        Self(n)
    }
}

/// One plan approval a turn asked for, unique within the turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PlanApprovalId(pub(super) u64);

#[cfg(test)]
impl PlanApprovalId {
    /// The id a turn gives its `n`th plan approval, counting from 0, for
    /// host tests that play the turn.
    pub(crate) fn nth_for_tests(n: u64) -> Self {
        Self(n)
    }
}

/// A plan `exit_plan_mode` asks the user to approve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlanApprovalRequest {
    /// The `exit_plan_mode` call, so the host shows the prompt on its card.
    pub(crate) call_id: ToolCallId,
    pub(crate) plan: String,
    /// The modes the user may approve into, in offer order.
    pub(crate) targets: Vec<PermissionMode>,
}

/// The user message a prompt inserted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InsertedMessage {
    /// Saved as this display row; a reload replays the message from it.
    Saved(DisplayRowOrigin),
    /// Not replayed, such as the prompt of a runtime that a successor
    /// replaced, or of a host command that saves nothing.
    Unreplayed,
}

/// What one turn reports, in order.
pub(crate) enum SessionEvent {
    /// The turn saved the prompt's user message, before its run. Sent once,
    /// and first: before any notice and before [`Self::RunStarted`], so a
    /// host that answers a prompt once its message is in can answer before
    /// it shows anything of the turn. Not sent when the save fails, which
    /// fails the turn.
    MessageInserted(InsertedMessage),
    /// The SDK run started, and [`super::SessionHandle::steer`] reaches it.
    /// Nothing about the run precedes it; the inserted message and a notice
    /// may.
    RunStarted,
    /// An SDK run event, as the run emitted it.
    Run(RunEvent),
    /// The rows to show for a background result the run is about to apply,
    /// sent just before the [`RunEvent::BoundaryInputApplied`] that applies
    /// it. The turn saves the same rows.
    BoundaryApplied { transcript: DisplayTranscript },
    /// A tool needs permission. Answer with [`TurnControl::RespondApproval`].
    ApprovalRequested {
        id: ApprovalId,
        request: ApprovalRequest,
    },
    /// The turn no longer waits on approval `id`: it was cancelled, and the
    /// tool was told so. Answers to it are ignored.
    ApprovalWithdrawn { id: ApprovalId },
    /// `exit_plan_mode` waits for the user to approve its plan. Answer with
    /// [`TurnControl::AnswerPlan`].
    PlanApprovalRequested {
        id: PlanApprovalId,
        request: PlanApprovalRequest,
    },
    /// The turn no longer waits on plan approval `id`: it was cancelled, or
    /// the tool stopped waiting. Answers to it are ignored.
    PlanApprovalWithdrawn { id: PlanApprovalId },
    /// The run asks the user `request`, from the tool call `call_id` when a
    /// tool asks. Answer with [`TurnControl::AnswerQuestion`]. Sent only to
    /// hosts with [`super::QuestionSupport::Available`]; for others the
    /// turn declines the question itself.
    QuestionAsked {
        id: HostInputId,
        call_id: Option<ToolCallId>,
        request: HostInputRequest,
    },
    /// The turn no longer waits on question `id`: it was cancelled or the
    /// run ended. Answers to it are ignored.
    QuestionWithdrawn { id: HostInputId },
    /// Something to tell the user that is not part of the run, such as a
    /// note found when the session was reopened.
    Notice { text: String },
    /// The run's event stream ended; anything it left open, such as tool
    /// calls without a result, should close as `ending` says. Approvals
    /// still open may follow, until each is answered or withdrawn.
    RunClosed { ending: RunEnd },
    /// The run ended, whether it completed, was cancelled, or failed, with
    /// `context` as its settled context estimate. Sent before the turn is
    /// saved; not sent when the host failed the turn. By then the turn reads
    /// no more controls, so a delivery failure the host reports later does
    /// not fail the turn; see [`TurnControl::HostFailed`].
    RunFinished { context: ContextEstimate },
}

/// How the run left calls it never finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RunEnd {
    /// Its events ended: it completed, was cancelled, or its stream stopped
    /// without a terminal event.
    Ended,
    /// The turn failed with this message after the run started.
    Failed(String),
}

/// A host's answer to one request a running turn raised. Answers to ids the
/// turn does not know, or no longer waits on, are ignored.
#[derive(Debug, PartialEq)]
pub(crate) enum TurnControl {
    RespondApproval {
        id: ApprovalId,
        decision: ApprovalDecision,
    },
    AnswerPlan {
        id: PlanApprovalId,
        answer: PlanPermissionAnswer,
    },
    AnswerQuestion {
        id: HostInputId,
        answer: Result<HostInputResponse, HostInputRefusal>,
    },
    /// The host could not deliver what the turn reported. The turn cancels
    /// its run and fails with `message`, if it still reads controls: it
    /// stops once the run's events ended ([`SessionEvent::RunClosed`]) and
    /// nothing waits on the host. A failure that arrives later does not
    /// fail the turn, which is saved and, when its run completed, reported
    /// to `session_completed` as usual. So a host that falls behind or goes
    /// away, such as a dead subscriber, never fails a finished turn; it
    /// reports the failure to its own client instead, as ACP fails the
    /// prompt.
    HostFailed { message: String },
}

/// The decision for an approval that was cancelled before anyone answered:
/// a prompt cancel, or a host that dismissed the request.
pub(crate) fn cancelled_approval() -> ApprovalDecision {
    ApprovalDecision::Deny {
        reason: "permission request cancelled".into(),
    }
}
