//! Protocol terminal classification; process exit is not an ACP stop reason.

use agent_client_protocol::v1::schema::StopReason;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TurnClassification {
    Success,
    Failure { subtype: &'static str },
    Stopped,
    Invalid,
}

/// Once Rho sent `session/cancel`, the run is stopped whatever the agent
/// reports: an `end_turn` already in flight must not turn a cancel into Ok.
pub(super) fn classify_stop(stop: &StopReason, cancelled_by_us: bool) -> TurnClassification {
    if cancelled_by_us {
        return TurnClassification::Stopped;
    }
    match stop {
        StopReason::EndTurn => TurnClassification::Success,
        StopReason::MaxTokens => TurnClassification::Failure {
            subtype: "max_tokens",
        },
        StopReason::MaxTurnRequests => TurnClassification::Failure {
            subtype: "max_turn_requests",
        },
        StopReason::Refusal => TurnClassification::Failure { subtype: "refusal" },
        StopReason::Cancelled => TurnClassification::Failure {
            subtype: "cancelled",
        },
        _ => TurnClassification::Invalid,
    }
}

impl TurnClassification {
    /// What a run reports for a turn that ended without success; `None` for
    /// success and for a cancel Rho asked for.
    pub(super) fn failure_detail(&self) -> Option<String> {
        match self {
            Self::Success | Self::Stopped => None,
            Self::Failure {
                subtype: "cancelled",
            } => Some("acp: agent cancelled the turn".into()),
            Self::Failure { subtype } => Some(format!("acp: turn ended with {subtype}")),
            Self::Invalid => Some("acp: invalid stop reason".into()),
        }
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
