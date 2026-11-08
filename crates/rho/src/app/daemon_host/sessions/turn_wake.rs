//! A turn's side of the session's T3 wake watchers (see
//! [`super::super::t3_wake`]): telling the watchers a prompt started, so
//! the armed one stops signalling while the prompt delivers results itself,
//! and arming one as the turn ends with work outstanding; and, in the
//! watcher's own session, running `/rho-wake <token>` as the watcher's
//! wait, without a model turn.

use rho_protocol as wire;
use rho_sdk::{CancellationToken, UserInput};

use super::{
    super::{
        t3_wake::{wake_command, Background, PromptOrigin, WakeSignal},
        LoadedSession, TurnKind,
    },
    DaemonView, Ran,
};
use crate::app::{
    session_commands::leading_text,
    session_runtime::{CommandRejected, InsertedMessage, PromptScope, SessionRuntime},
};

/// Added for the model to a prompt that mentions a watcher, normally T3's
/// notice that it finished. T3 asks the model to read the task, but a
/// watcher holds no result: the background results follow at the first
/// boundary. The notice stays whole, since T3 batches other tasks into it.
const WATCHER_NOTE: &str = "Delegated tasks titled `waiting: ...` are Rho's background-work \
                            watchers, not agents, and hold no result. Results of background work \
                            that finished, if any, follow.";

/// What a prompt of a session is, as far as its wake watchers go.
pub(super) enum WakePrompt {
    /// A model turn or another command; a watcher notice adds
    /// [`WATCHER_NOTE`] to a model turn's input.
    Prompt(PromptOrigin),
    /// The watcher's wait: `/rho-wake <token>`, or any prompt to a watcher
    /// whose wait is recorded, which resumes the wait or repeats its result.
    /// Rejected when the token is malformed.
    Watch(Result<WakeSignal, CommandRejected>),
}

/// `model`, a model turn's input, with [`WATCHER_NOTE`] appended when
/// `origin` says the prompt mentions a watcher.
pub(super) fn model_input(origin: &PromptOrigin, model: UserInput) -> UserInput {
    match origin {
        PromptOrigin::WatcherNotice => {
            let mut blocks = model.blocks().to_vec();
            blocks.push(rho_sdk::model::ContentBlock::Text(WATCHER_NOTE.to_owned()));
            UserInput::content(blocks).expect("adding a text block keeps the input valid")
        }
        PromptOrigin::Other => model,
    }
}

/// Call as `session`'s prompt of `input` starts, before its turn runs.
/// Every prompt of a session its host wakes stops the armed watcher's
/// signalling until the turn ends ([`prompt_ended`]).
pub(super) async fn prompt_started(session: &LoadedSession, input: &UserInput) -> WakePrompt {
    let Some(wake) = &session.wake else {
        return WakePrompt::Prompt(PromptOrigin::Other);
    };
    let text = leading_text(input);
    let mut wake = wake.lock().await;
    let origin = wake.prompt_started(text);
    if let Some(signal) = WakeSignal::recorded(wake.dir(), session.id.as_str()) {
        return WakePrompt::Watch(Ok(signal));
    }
    match wake_command(wake.dir(), text) {
        Some(signal) => WakePrompt::Watch(signal),
        None => WakePrompt::Prompt(origin),
    }
}

/// Call as `session`'s turn on `runtime` ended as `ended`, before its end
/// is told: arms a watcher while background work runs on, unless the
/// daemon is `stopping`, which takes the session's work down with it. The
/// daemon's stop, or a cancel sent meanwhile, interrupts the host calls. A
/// cancelled turn usually gives way to the next prompt, whose end arms
/// instead, so only other ends that were not interrupted tell clients that
/// arming failed.
pub(super) async fn prompt_ended(
    session: &LoadedSession,
    runtime: &SessionRuntime,
    ended: &anyhow::Result<Ran>,
    stopping: &CancellationToken,
) {
    let Some(wake) = &session.wake else {
        return;
    };
    if stopping.is_cancelled() {
        return;
    }
    // A fresh cancel for the rest of the turn: the turn's own may have
    // tripped already, ending its run, and a cancel from here on must still
    // reach the host calls.
    let cancel = {
        let mut turn = session.turn();
        turn.cancel = CancellationToken::new();
        turn.cancel.clone()
    };
    let interrupt = CancellationToken::new();
    let tools = runtime.tools();
    let background = Background::of(tools, runtime.session_id().to_string());
    let arming = async {
        wake.lock()
            .await
            .prompt_ended(tools, background, &interrupt)
            .await
    };
    tokio::pin!(arming);
    let armed = tokio::select! {
        armed = &mut arming => armed,
        () = async {
            tokio::select! {
                () = stopping.cancelled() => {}
                () = cancel.cancelled() => {}
            }
        } => {
            // The host calls give up at once; arming keeps what they got to.
            interrupt.cancel();
            arming.await
        }
    };
    let Err(error) = armed else {
        return;
    };
    match (ended, interrupt.is_cancelled()) {
        (Ok(Ran::Cancelled), _) | (_, true) => {
            tracing::debug!(%error, "could not arm T3 wake watcher after a cancel or stop");
        }
        (Ok(Ran::Completed(_)) | Err(_), false) => {
            tracing::warn!(%error, "could not arm T3 wake watcher");
            session.feed.publish([wire::SessionEvent::Notice {
                text: format!(
                    "could not watch background work: {error}; its results arrive with your \
                     next message"
                ),
            }]);
        }
    }
}

/// Runs the watcher's wait for `signal` within `scope`, without a model
/// turn or a saved prompt. Clients see the prompt, `display`, then the
/// parent's summary as the agent's reply, which the turn answers with too.
/// The watcher's session saves nothing, so it leaves no transcript behind.
pub(super) async fn watch(
    view: &mut DaemonView,
    scope: &PromptScope,
    display: &UserInput,
    signal: WakeSignal,
) -> anyhow::Result<Ran> {
    let shown = view
        .session
        .live()
        .shown_prompt(display, &InsertedMessage::Unreplayed);
    view.session.feed.publish([shown]);
    // A prompt sent while the wait runs resumes it.
    view.session.turn().kind = TurnKind::WakeWait;
    // Nothing rejects the prompt from here on; its request need not wait
    // for the parent.
    view.began();
    let Some(summary) = signal
        .wait(view.session.id.as_str(), scope.cancellation())
        .await
    else {
        // A cancel keeps the wait recorded, so the session's next prompt
        // resumes it: T3 Code cancels its prompts as it quits, then sends
        // "continue" once it starts again. A watcher T3 cancelled is never
        // prompted again; the parent's retire and the directory's pruning
        // clean up after it.
        return Ok(Ran::Cancelled);
    };
    let reply = view
        .session
        .live()
        .shown_reply(&InsertedMessage::Unreplayed, summary.clone());
    view.session.feed.publish([reply]);
    Ok(Ran::Completed(wire::TurnOutcome::Answered {
        text: summary,
        stop: wire::AnswerStop::EndTurn,
    }))
}
