//! Protocol terminal classification; process exit is not an ACP stop reason.

use agent_client_protocol::schema::v1::StopReason;

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

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
