use super::*;
use pretty_assertions::assert_eq;

// Covers: token/request limits and spontaneous cancellation cannot report Ok,
// and a local cancel stays Stopped even when an end_turn response won the race.
// Owner: protocol stop-reason classification, without process-exit policy.
#[test]
fn stop_reason_and_cancellation_origin_matrix() {
    let cases = [
        (StopReason::EndTurn, TurnClassification::Success),
        (
            StopReason::MaxTokens,
            TurnClassification::Failure {
                subtype: "max_tokens",
            },
        ),
        (
            StopReason::MaxTurnRequests,
            TurnClassification::Failure {
                subtype: "max_turn_requests",
            },
        ),
        (
            StopReason::Refusal,
            TurnClassification::Failure { subtype: "refusal" },
        ),
        (
            StopReason::Cancelled,
            TurnClassification::Failure {
                subtype: "cancelled",
            },
        ),
    ];
    for (stop, spontaneous) in cases {
        assert_eq!(classify_stop(&stop, /*cancelled_by_us*/ false), spontaneous);
        assert_eq!(
            classify_stop(&stop, /*cancelled_by_us*/ true),
            TurnClassification::Stopped
        );
    }
}
