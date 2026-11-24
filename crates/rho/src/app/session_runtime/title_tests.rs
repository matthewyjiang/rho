use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{model::ContentBlock, UserInput};
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
    let outcome = runtime
        .run_turn(
            &prompt,
            Turn {
                display: UserInput::text(message),
                // What the model is sent, with what a client adds to it.
                model: UserInput::text(format!("context a client added\n\n{message}")),
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

/// What each turn of `shown` asked to name its session from, if it asked.
fn named_from(shown: [Vec<SessionEvent>; 3]) -> [Option<UserInput>; 3] {
    shown.map(|turn| {
        turn.into_iter().find_map(|event| {
            if let SessionEvent::TitleRequested(job) = event {
                Some(job.request.input)
            } else {
                None
            }
        })
    })
}

/// What a session is named from when `message` is the first its user sent.
fn first_message(message: &str) -> Option<UserInput> {
    Some(crate::title::session_input(&[ContentBlock::Text(
        message.into(),
    )]))
}

// Covers: a runtime asks its host to name the session as its first prompt
// is saved, right after the message and before the run, not once the model
// answered, so a first turn that never answers is still named, from the
// message as its user sent it; later prompts ask nothing.
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
        turn(&mut runtime, &config, "and on", Ending::Cancelled).await,
    ];
    // The first turn's order: the message is saved, then the title asked
    // for, before the run starts.
    let order: Vec<_> = shown[0]
        .iter()
        .filter_map(|event| {
            if matches!(event, SessionEvent::MessageInserted(_)) {
                Some("message saved")
            } else if matches!(event, SessionEvent::TitleRequested(_)) {
                Some("title requested")
            } else if matches!(event, SessionEvent::RunStarted) {
                Some("run started")
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        (order, named_from(shown)),
        (
            vec!["message saved", "title requested", "run started"],
            [first_message("name this session"), None, None]
        )
    );
    runtime.shutdown().await;
}

// Covers: what fails a first turn, such as an outage or an expired
// sign-in, can fail the title model with it, so the prompt after a failed
// turn asks again; without that the session stayed unnamed until reloaded.
// It names the session from the first message its transcript shows, as a
// session reloaded untitled does, not from the prompt that came after,
// such as "try again" or "continue".
// Owner: session runtime titles
#[tokio::test]
async fn the_prompt_after_a_failed_turn_asks_to_name_the_first_message_again() {
    let config = config();
    let root = tempfile::tempdir().unwrap();
    let mut runtime = idle_runtime(root.path(), &config).await;
    let shown = [
        turn(&mut runtime, &config, "name this session", Ending::Failed).await,
        turn(&mut runtime, &config, "try again", Ending::Cancelled).await,
        turn(&mut runtime, &config, "and go on", Ending::Cancelled).await,
    ];
    assert_eq!(
        named_from(shown),
        [
            first_message("name this session"),
            first_message("name this session"),
            None
        ]
    );
    runtime.shutdown().await;
}
