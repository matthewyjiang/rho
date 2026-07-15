use std::{num::NonZeroUsize, time::Duration};

use crate::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    Error, ProviderError, ProviderErrorKind, Retryability, Rho, RunEvent, RunId, SessionOptions,
    UserInput,
};

use super::{Session, SessionState};

const TEST_TIMEOUT: Duration = Duration::from_secs(1);

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

fn completed_turn(text: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

async fn wait_for_state(session: &Session, expected: SessionState) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        while session.state() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("session did not reach {expected:?}"));
}

#[tokio::test]
async fn completed_terminal_backpressure_keeps_the_run_owner_active() {
    let provider = ScriptedProvider::new(
        identity(),
        [completed_turn("first"), completed_turn("second")],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    assert!(matches!(
        run.next_event().await,
        Some(RunEvent::Started { .. })
    ));
    // Leave `StepStarted` in the sole event slot so terminal delivery cannot complete.
    wait_for_state(&session, SessionState::Completed).await;

    assert!(session.is_running());
    assert!(matches!(
        session.start(UserInput::text("overlap")).await,
        Err(Error::SessionBusy)
    ));
    assert!(matches!(
        run.next_event().await,
        Some(RunEvent::StepStarted { step: 1 })
    ));
    assert!(matches!(
        run.next_event().await,
        Some(RunEvent::Completed { .. })
    ));
    assert!(run.next_event().await.is_none());
    assert_eq!(run.outcome().await.unwrap().text(), "first");
    assert!(!session.is_running());

    assert_eq!(session.complete("retry").await.unwrap().text(), "second");
}

#[tokio::test]
async fn failed_terminal_backpressure_keeps_the_run_owner_active() {
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::failed(ProviderError::new(
                ProviderErrorKind::Other,
                "expected failure",
                Retryability::Permanent,
            )),
            completed_turn("recovered"),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    assert!(matches!(
        run.next_event().await,
        Some(RunEvent::Started { .. })
    ));
    // Leave `StepStarted` in the sole event slot so terminal delivery cannot complete.
    wait_for_state(&session, SessionState::Failed).await;

    assert!(session.is_running());
    assert!(matches!(
        session.start(UserInput::text("overlap")).await,
        Err(Error::SessionBusy)
    ));
    assert!(matches!(
        run.next_event().await,
        Some(RunEvent::StepStarted { step: 1 })
    ));
    assert!(matches!(
        run.next_event().await,
        Some(RunEvent::Failed { .. })
    ));
    assert!(run.next_event().await.is_none());
    assert!(matches!(run.outcome().await, Err(Error::Provider(_))));
    assert!(!session.is_running());

    assert_eq!(session.complete("retry").await.unwrap().text(), "recovered");
}

#[tokio::test]
async fn stale_finalization_cannot_clear_a_newer_run_owner() {
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(identity(), []))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let stale_run = RunId::new();
    let current_run = RunId::new();

    session.core.begin_run(&stale_run).unwrap();
    session.core.finish_run(&stale_run);
    session.core.begin_run(&current_run).unwrap();
    session.core.finish_run(&stale_run);

    assert!(session.is_running());
    assert_eq!(session.state(), SessionState::Running);
    assert!(matches!(
        session.core.begin_run(&RunId::new()),
        Err(Error::SessionBusy)
    ));

    session.core.finish_run(&current_run);
    assert!(!session.is_running());
    assert_eq!(session.state(), SessionState::Idle);
}
