use std::{sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::{
    hooks::{testing::RecordingObserver, HookEventKind},
    provider::ModelProvider,
    Rho, UserInput,
};
use rho_tools::cancellation::RunCancellation;
use tokio::time::timeout;

use super::{run_prompt, HeadlessView, PromptEnd, StopCause, Stops};
use crate::{
    app::{
        session_runtime::{
            unattended_runtime_on, HangUntilCancel, QuestionSupport, Turn, TurnOutcome,
        },
        shutdown_signal::ShutdownSignal,
    },
    config::Config,
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(5);

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

// Covers: a headless prompt that a stop cancels ends its session to the
// hooks exactly once, as failed, though a cancelled turn reports no
// session end of its own and teardown reports none either.
// Owner: headless host prompt driver
#[tokio::test]
async fn a_stopped_prompt_ends_its_session_once_as_failed() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let observer = Arc::new(RecordingObserver::new());
    let provider: Arc<dyn ModelProvider> = Arc::new(HangUntilCancel);
    let sdk = Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .hook_observer_shared(observer.clone())
        .build()
        .unwrap();
    let runtime = unattended_runtime_on(
        root.path(),
        &config,
        QuestionSupport::Unavailable,
        sdk,
        provider,
    )
    .await;
    // The parent cancelled already; the prompt begins before the stop
    // reaches it, and its turn waits until the stop cancels it.
    let cancellation = RunCancellation::new();
    cancellation.cancel();
    let mut stops = Stops::default().or_cancelled(cancellation);
    let mut view = HeadlessView::new();

    let end = timeout(
        TEST_BUDGET,
        run_prompt(
            &mut stops,
            &mut view,
            &config,
            async { anyhow::Ok(runtime) },
            |runtime, view, scope, process_config| {
                let input = UserInput::text("go");
                let turn = Turn {
                    display: input.clone(),
                    model: input,
                    calls: Vec::new(),
                };
                Box::pin(runtime.run_prompt_turn(view, scope, turn, process_config))
            },
        ),
    )
    .await
    .expect("the stopped prompt ends");

    let session_ends = observer
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                HookEventKind::SessionCompleted | HookEventKind::SessionFailed
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        (ended_as(end), session_ends),
        (
            Ended::Stopped(StopCause::Cancelled),
            vec![HookEventKind::SessionFailed]
        )
    );
}
