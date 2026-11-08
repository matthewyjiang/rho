//! A turn's side of the session's T3 wake watchers (see
//! [`super::super::t3_wake`]): telling the watchers a prompt started, so
//! the armed one stops signalling while the prompt delivers results itself,
//! and arming one as the turn ends with work outstanding; and, in the
//! watcher's own session, running `/rho-wake <token>` as the watcher's
//! wait, without a model turn.

use rho_protocol as wire;
use rho_sdk::UserInput;

use super::{
    super::{
        t3_wake::{Background, PromptOrigin, WakeSignal, WAKE_COMMAND},
        LoadedSession,
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
    let origin = wake.lock().await.prompt_started(text);
    if let Some(signal) = WakeSignal::recorded(session.id.as_str()) {
        return WakePrompt::Watch(Ok(signal));
    }
    let token = text
        .and_then(|text| text.strip_prefix(WAKE_COMMAND))
        .filter(|rest| rest.starts_with(char::is_whitespace));
    match token {
        Some(token) => WakePrompt::Watch(
            WakeSignal::parse(token)
                .ok_or_else(|| CommandRejected(format!("usage: {WAKE_COMMAND} <token>"))),
        ),
        None => WakePrompt::Prompt(origin),
    }
}

/// Call as `session`'s turn on `runtime` ended as `ended`, before its end
/// is told: arms a watcher while background work runs on. A cancelled turn
/// usually gives way to the next prompt, whose end arms instead, so only
/// other ends tell clients that arming failed.
pub(super) async fn prompt_ended(
    session: &LoadedSession,
    runtime: &SessionRuntime,
    ended: &anyhow::Result<Ran>,
) {
    let Some(wake) = &session.wake else {
        return;
    };
    let tools = runtime.tools();
    let background = Background::of(tools, runtime.session_id().to_string());
    let Err(error) = wake.lock().await.prompt_ended(tools, background).await else {
        return;
    };
    match ended {
        Ok(Ran::Cancelled) => {
            tracing::debug!(%error, "could not arm T3 wake watcher after a cancelled turn");
        }
        Ok(Ran::Completed(_)) | Err(_) => {
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
    // Nothing rejects the prompt from here on; its request need not wait
    // for the parent.
    view.began();
    let session_id = view.session.id.as_str();
    let Some(summary) = signal.wait(session_id, scope.cancellation()).await else {
        // A cancel, as T3's task_cancel sends by closing the watcher's
        // session, or the daemon's stop ends the wait for good: a session
        // that saved nothing does not outlive its daemon.
        signal.abandon(session_id);
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
