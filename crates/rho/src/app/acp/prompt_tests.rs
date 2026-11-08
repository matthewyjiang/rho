use agent_client_protocol::{schema::v1::StopReason, Error as AcpError, ErrorCode};
use pretty_assertions::assert_eq;
use rho_protocol::{AnswerStop, TurnError, TurnErrorKind, TurnOutcome};

use super::stop_reason;

fn failed(kind: TurnErrorKind, message: &str) -> TurnOutcome {
    TurnOutcome::Failed {
        error: TurnError {
            kind,
            message: message.into(),
        },
    }
}

fn answered(stop: AnswerStop) -> TurnOutcome {
    TurnOutcome::Answered {
        text: "done".into(),
        stop,
    }
}

// Covers: how a turn ended answers the host's `session/prompt`: an answer
// keeps the provider's stop, a step limit is ACP's turn limit, a cancel or
// timeout is a cancel, and a failure is an error whose code tells the host
// whether signing in or the session's config can fix it
// Owner: ACP prompt
#[test]
fn turn_outcomes_answer_the_prompt() {
    let cases = [
        (
            "answer",
            answered(AnswerStop::EndTurn),
            Ok(StopReason::EndTurn),
        ),
        (
            "output limit",
            answered(AnswerStop::MaxTokens),
            Ok(StopReason::MaxTokens),
        ),
        (
            "refusal",
            answered(AnswerStop::Refusal),
            Ok(StopReason::Refusal),
        ),
        (
            "unknown stop",
            answered(AnswerStop::Unknown),
            Ok(StopReason::EndTurn),
        ),
        (
            "step limit",
            TurnOutcome::MaxSteps {
                text: String::new(),
            },
            Ok(StopReason::MaxTurnRequests),
        ),
        ("cancel", TurnOutcome::Cancelled, Ok(StopReason::Cancelled)),
        (
            "timeout",
            TurnOutcome::TimedOut { timeout_ms: 1000 },
            Ok(StopReason::Cancelled),
        ),
        (
            "sign-in needed",
            failed(TurnErrorKind::Authentication, "run rho login openai"),
            Err(AcpError::new(
                i32::from(ErrorCode::AuthRequired),
                "run rho login openai",
            )),
        ),
        (
            "bad config",
            failed(TurnErrorKind::Configuration, "no such model"),
            Err(AcpError::invalid_params().data("no such model")),
        ),
        (
            "provider failure",
            failed(TurnErrorKind::Provider, "503"),
            Err(AcpError::internal_error().data("503")),
        ),
        (
            "unknown outcome",
            TurnOutcome::Unknown,
            Ok(StopReason::EndTurn),
        ),
    ];
    for (label, outcome, expected) in cases {
        assert_eq!(stop_reason(outcome), expected, "{label}");
    }
}
