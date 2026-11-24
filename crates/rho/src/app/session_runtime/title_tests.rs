use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message},
    UserInput,
};
use tokio::sync::mpsc;

use crate::{
    app::session_runtime::{
        test_support::{idle_runtime, runtime_on, HangUntilCancel},
        SessionEvent, SessionRuntime, Turn, TurnIo, TurnOutcome,
    },
    config::Config,
};

fn config() -> Config {
    Config {
        provider: "test".into(),
        model: "model".into(),
        ..Config::default()
    }
}

/// How a test's turn ends, its model never answering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// Cancelled as it starts.
    Cancelled,
    /// Failed by its provider.
    Failed,
}

/// Runs a turn prompted with `message` on `runtime` until its `ending`,
/// and yields what the turn showed.
async fn turn(
    runtime: &mut SessionRuntime,
    config: &Config,
    message: &str,
    ending: Ending,
) -> Vec<SessionEvent> {
    let prompt = runtime.begin_prompt(config).await.unwrap();
    match ending {
        Ending::Cancelled => runtime.handle().cancel(),
        Ending::Failed => {}
    }
    let (events, mut shown) = mpsc::unbounded_channel();
    let (_controls, control) = mpsc::unbounded_channel();
    let input = UserInput::text(message);
    let outcome = runtime
        .run_turn(
            &prompt,
            Turn {
                display: input.clone(),
                model: input,
                calls: Vec::new(),
            },
            TurnIo { events, control },
            config,
        )
        .await;
    drop(prompt);
    let ended = match outcome {
        Ok(TurnOutcome::Cancelled) => Some(Ending::Cancelled),
        Err(_) => Some(Ending::Failed),
        Ok(TurnOutcome::Completed(_)) => None,
    };
    assert_eq!(ended, Some(ending), "{message}");
    std::iter::from_fn(|| shown.try_recv().ok()).collect()
}

/// Whether each turn of `shown` asked for a title.
fn asked(shown: &[Vec<SessionEvent>]) -> Vec<bool> {
    shown
        .iter()
        .map(|turn| {
            turn.iter()
                .any(|event| matches!(event, SessionEvent::TitleRequested(_)))
        })
        .collect()
}

// Covers: a runtime asks its host to name the session as its first prompt
// is saved, right after the message and before the run, not once the model
// answered, so a first turn that never answers is still named; later
// prompts ask nothing.
// Owner: session runtime titles
#[tokio::test]
async fn the_first_saved_prompt_asks_for_a_title_once() {
    let config = config();
    let root = tempfile::tempdir().unwrap();
    let mut runtime = runtime_on(root.path(), &config, Arc::new(HangUntilCancel)).await;
    let shown = [
        turn(
            &mut runtime,
            &config,
            "name this session",
            Ending::Cancelled,
        )
        .await,
        turn(&mut runtime, &config, "and go on", Ending::Cancelled).await,
    ];
    let position = |wanted: fn(&SessionEvent) -> bool| shown[0].iter().position(wanted);
    let saved = position(|event| matches!(event, SessionEvent::MessageInserted(_)));
    let requested = position(|event| matches!(event, SessionEvent::TitleRequested(_)));
    let started = position(|event| matches!(event, SessionEvent::RunStarted));
    assert_eq!(
        (asked(&shown), saved < requested, requested < started),
        (vec![true, false], true, true)
    );
    runtime.shutdown().await;
}

// Covers: what fails a first turn, such as an outage or an expired
// sign-in, can fail the title model with it, so the prompt after a failed
// turn asks again; without that the session stayed unnamed until reloaded.
// Owner: session runtime titles
#[tokio::test]
async fn the_prompt_after_a_failed_turn_asks_for_a_title_again() {
    let config = config();
    let root = tempfile::tempdir().unwrap();
    let mut runtime = idle_runtime(root.path(), &config).await;
    let shown = [
        turn(&mut runtime, &config, "name this session", Ending::Failed).await,
        turn(&mut runtime, &config, "try again", Ending::Cancelled).await,
        turn(&mut runtime, &config, "and go on", Ending::Cancelled).await,
    ];
    assert_eq!(asked(&shown), [true, true, false]);
    runtime.shutdown().await;
}

// Covers: a session reloaded without a title, as one whose title model
// was not signed in, is named from the first message its history holds,
// not from the prompt that came first after the reload, such as
// "continue".
// Owner: session runtime titles
#[tokio::test]
async fn a_reloaded_session_is_named_from_its_first_message() {
    let config = config();
    let root = tempfile::tempdir().unwrap();
    let mut runtime = runtime_on(root.path(), &config, Arc::new(HangUntilCancel)).await;
    let first = [ContentBlock::Text("migrate the billing tables".into())];
    runtime
        .built
        .session
        .append_message(Message::User(first.clone().into()))
        .unwrap();

    let named_from: Vec<_> = turn(&mut runtime, &config, "continue", Ending::Cancelled)
        .await
        .into_iter()
        .filter_map(|event| {
            if let SessionEvent::TitleRequested(job) = event {
                Some(job.request.input)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(named_from, [crate::title::session_input(&first)]);
    runtime.shutdown().await;
}
