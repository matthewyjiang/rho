use pretty_assertions::assert_eq;

use super::{PromptEnd, StopCause};
use crate::app::{session_runtime::TurnOutcome, shutdown_signal::ShutdownSignal};

/// How a prompt ended, comparable as a whole.
#[derive(Debug, PartialEq)]
enum Ended {
    Finished,
    Stopped(StopCause),
    Failed(String),
}

fn ended_as(end: PromptEnd) -> Ended {
    match end {
        PromptEnd::Finished(_) => Ended::Finished,
        PromptEnd::Stopped(cause) => Ended::Stopped(cause),
        PromptEnd::Failed(error) => Ended::Failed(error.to_string()),
    }
}

// Covers: a stop wins over however its prompt ended, so an interrupted run
// whose turn failed reports the interrupt, and a prompt cancelled though
// nothing stopped it fails instead of passing for a stop.
// Owner: pure unit (headless prompt end)
#[test]
fn a_stop_wins_and_an_unasked_cancel_fails() {
    let interrupted = StopCause::Interrupted(ShutdownSignal::Interrupt);
    for (ended, stop, expected) in [
        (
            Err(anyhow::anyhow!("provider unavailable")),
            Some(interrupted),
            Ended::Stopped(interrupted),
        ),
        (
            Ok(TurnOutcome::Cancelled),
            None,
            Ended::Failed("the prompt was cancelled".into()),
        ),
        (
            Err(anyhow::anyhow!("provider unavailable")),
            None,
            Ended::Failed("provider unavailable".into()),
        ),
    ] {
        assert_eq!(
            ended_as(PromptEnd::of(ended, stop)),
            expected,
            "stopped by {stop:?}"
        );
    }
}
