//! How a `rho run` or `rho attach` ends: what a failed run's JSONL stream's
//! `turn_ended` says, and the reason, exit code, and message of a turn's
//! end.

use rho_protocol::{TurnError, TurnErrorKind, TurnOutcome};

use super::{AutomationExit, TerminalReason};
use crate::app::session_events::outcome;

/// How a turn that failed with `error` failed, in a session whose model
/// runs on `provider` with `auth`: as the daemon tells its clients, unless
/// `rho run` itself ended the prompt with an [`AutomationExit`], whose
/// reason and message say it.
pub(super) fn turn_error(error: &anyhow::Error, provider: &str, auth: &str) -> TurnError {
    let Some(exit) = error.downcast_ref::<AutomationExit>() else {
        return outcome::turn_error(error, provider, auth);
    };
    let kind = match exit.end.reason {
        TerminalReason::Authentication => TurnErrorKind::Authentication,
        TerminalReason::ProviderError => TurnErrorKind::Provider,
        TerminalReason::ToolHostError => TurnErrorKind::ToolHost,
        TerminalReason::ConfigurationError => TurnErrorKind::Configuration,
        TerminalReason::Completed
        | TerminalReason::MaxSteps
        | TerminalReason::Timeout
        | TerminalReason::Interrupted
        | TerminalReason::Cancelled
        | TerminalReason::Detached
        | TerminalReason::OutputError
        | TerminalReason::OtherError => TurnErrorKind::Other,
    };
    TurnError {
        kind,
        message: exit.to_string(),
    }
}

/// The exit of a run that failed with `error`, told as `turn`: an
/// [`AutomationExit`] in the chain decides it, else the failure's kind,
/// with its message.
pub(super) fn failure(error: &anyhow::Error, turn: TurnError) -> AutomationExit {
    if let Some(exit) = error.downcast_ref::<AutomationExit>() {
        return exit.clone();
    }
    failed(turn)
}

/// The exit of a turn that ended as `outcome`: none for an answer.
pub(in crate::app) fn outcome_exit(outcome: &TurnOutcome) -> Result<(), AutomationExit> {
    match outcome {
        TurnOutcome::Answered { .. } => Ok(()),
        TurnOutcome::MaxSteps { .. } => Err(AutomationExit::new(
            124,
            TerminalReason::MaxSteps,
            "the turn reached its model-step limit",
        )),
        TurnOutcome::Cancelled => Err(AutomationExit::new(
            1,
            TerminalReason::Cancelled,
            "the turn was cancelled",
        )),
        TurnOutcome::Failed { error } => Err(failed(error.clone())),
        // `TurnOutcome` is non-exhaustive; an end a newer daemon sends is
        // another error.
        _ => Err(AutomationExit::new(
            1,
            TerminalReason::OtherError,
            "the turn ended in a way this rho does not know",
        )),
    }
}

/// The exit of a turn that failed as `turn` says.
fn failed(turn: TurnError) -> AutomationExit {
    let TurnError { kind, message } = turn;
    let (reason, code) = match kind {
        TurnErrorKind::Authentication => (TerminalReason::Authentication, 1),
        TurnErrorKind::Provider => (TerminalReason::ProviderError, 1),
        TurnErrorKind::ToolHost => (TerminalReason::ToolHostError, 1),
        TurnErrorKind::Configuration => (TerminalReason::ConfigurationError, 2),
        // `TurnErrorKind` is non-exhaustive; a kind a newer protocol adds is
        // another error.
        _ => (TerminalReason::OtherError, 1),
    };
    AutomationExit::new(code, reason, message)
}
