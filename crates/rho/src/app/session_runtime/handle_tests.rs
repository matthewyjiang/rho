use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelRequest, ModelResponse},
    provider::{ModelProvider, ProviderFuture},
    RunEvent, UserInput,
};
use tokio::{
    sync::{mpsc, Notify},
    time::timeout,
};

use super::SteerError;
use crate::{
    app::session_runtime::{test_support::runtime_on, SessionEvent, Turn, TurnIo, TurnOutcome},
    config::Config,
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(5);

/// A provider whose first request tells `started` it is in flight, then
/// waits for `release`, so a test steers while the run is busy. Every
/// request answers `done`; their messages are recorded.
struct HoldFirstRequest {
    started: Notify,
    release: Notify,
    calls: AtomicUsize,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl ModelProvider for HoldFirstRequest {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("test", "test", "model")
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap()
                .push(request.messages.to_vec());
            if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )]))
        })
    }
}

/// How a steer through the handle went, comparable as a whole.
#[derive(Debug, PartialEq)]
enum Steered {
    Staged,
    NotRunning,
    Refused(String),
}

fn steered<T>(result: &Result<T, SteerError>) -> Steered {
    match result {
        Ok(_) => Steered::Staged,
        Err(SteerError::NotRunning) => Steered::NotRunning,
        Err(SteerError::Refused(error)) => Steered::Refused(error.to_string()),
    }
}

// Covers: a steer through the session's handle reaches the running model
// turn at its next provider request, and is refused, not lost, before the
// run starts, after it ends, and once the prompt is over
// Owner: session runtime handle and prompt gate
#[tokio::test]
async fn steering_through_the_handle_reaches_only_a_running_model_turn() {
    let provider = Arc::new(HoldFirstRequest {
        started: Notify::new(),
        release: Notify::new(),
        calls: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
    });
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let mut runtime = runtime_on(root.path(), &config, provider.clone()).await;
    let handle = runtime.handle();

    let before_prompt = steered(&handle.steer(UserInput::text("too early")));
    let prompt = runtime.begin_prompt(&config).await.unwrap();
    let while_starting = steered(&handle.steer(UserInput::text("still starting")));
    let input = UserInput::text("go");
    let turn = Turn {
        display: input.clone(),
        model: input,
        calls: Vec::new(),
    };
    let (events, mut shown) = mpsc::unbounded_channel();
    let (_controls, control) = mpsc::unbounded_channel();
    let parent = async {
        // The run takes steering once its start is reported, and is busy
        // once its first request is in flight.
        loop {
            match shown.recv().await {
                Some(SessionEvent::RunStarted) => break,
                Some(_) => {}
                None => panic!("the turn ended before its run started"),
            }
        }
        provider.started.notified().await;
        let receipt = handle
            .steer(UserInput::text("change course"))
            .expect("a running model turn takes steering");
        provider.release.notify_one();
        let mut applied = Vec::new();
        while let Some(event) = shown.recv().await {
            if let SessionEvent::Run(RunEvent::SteeringApplied { ids }) = event {
                applied.extend(ids);
            }
        }
        (receipt.await.expect("the run accepted the steer"), applied)
    };

    let (ended, (accepted, applied)) = timeout(TEST_BUDGET, async {
        tokio::join!(
            runtime.run_turn(&prompt, turn, TurnIo { events, control }, &config),
            parent
        )
    })
    .await
    .expect("the turn ends");
    let ended = ended.expect("the turn succeeds");
    assert!(matches!(ended, TurnOutcome::Completed(_)), "{ended:?}");
    let after_run = steered(&handle.steer(UserInput::text("too late")));
    drop(prompt);
    let after_prompt = steered(&handle.steer(UserInput::text("after the prompt")));

    let reached_model = provider
        .requests
        .lock()
        .unwrap()
        .get(1)
        .is_some_and(|messages| messages.contains(&Message::user_text("change course")));
    assert_eq!(
        (
            [before_prompt, while_starting, after_run, after_prompt],
            applied,
            reached_model,
        ),
        (
            [
                Steered::NotRunning,
                Steered::NotRunning,
                Steered::NotRunning,
                Steered::NotRunning,
            ],
            vec![accepted],
            true,
        )
    );
    runtime.shutdown().await;
}

// Covers: a prompt that was cancelled takes no steer while its run winds
// down, since the run could accept it and end without applying it
// Owner: session runtime handle and prompt gate
#[tokio::test]
async fn a_cancelled_prompt_takes_no_steering() {
    let provider = Arc::new(HoldFirstRequest {
        started: Notify::new(),
        release: Notify::new(),
        calls: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
    });
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let mut runtime = runtime_on(root.path(), &config, provider.clone()).await;
    let handle = runtime.handle();
    let prompt = runtime.begin_prompt(&config).await.unwrap();
    let input = UserInput::text("go");
    let turn = Turn {
        display: input.clone(),
        model: input,
        calls: Vec::new(),
    };
    let (events, _shown) = mpsc::unbounded_channel();
    let (_controls, control) = mpsc::unbounded_channel();
    let parent = async {
        provider.started.notified().await;
        handle.cancel();
        let after_cancel = steered(&handle.steer(UserInput::text("too late")));
        provider.release.notify_one();
        after_cancel
    };

    let (_, after_cancel) = timeout(TEST_BUDGET, async {
        tokio::join!(
            runtime.run_turn(&prompt, turn, TurnIo { events, control }, &config),
            parent
        )
    })
    .await
    .expect("the turn ends");
    assert_eq!(after_cancel, Steered::NotRunning);
    drop(prompt);
    runtime.shutdown().await;
}
