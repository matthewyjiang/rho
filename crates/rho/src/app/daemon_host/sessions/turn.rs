//! One prompt's turn, run in a task the daemon owns, so it goes on when the
//! client that sent the prompt disconnects.
//!
//! The turn reports to a [`DaemonView`], which sends its events to the
//! session's attached clients (#1657) and opens and withdraws the session's
//! input requests as the turn asks (#1660): its job is `waiting_for_input`
//! while an approval, plan approval, or question is open, `running`
//! otherwise. A request is never denied because no client is attached.

use std::{future::Future, sync::Arc};

use rho_sdk::{CancellationToken, UserInput};
use tokio::sync::{mpsc, oneshot};

use super::{
    super::{
        jobs::{JobEvent, JobState, TurnEnd},
        wire::state_event,
    },
    inputs::Inputs,
    LoadedSession,
};
use crate::{
    app::session_runtime::{
        PlanReport, SessionEvent, SessionRuntime, Turn, TurnControl, TurnIo, TurnOutcome, TurnView,
    },
    config::Config,
};

/// How a prompt's turn began, as the request that sent the prompt learns
/// it. A turn that completed saved its prompt, so it began `Saved`. A turn
/// the daemon's stop interrupted before it saved its prompt sends no
/// answer: the request answers `DaemonStopping` once the stop begins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PromptStart {
    /// The turn saved the prompt and runs on.
    Saved,
    /// A client cancelled the turn before it saved the prompt.
    Cancelled,
    /// The turn failed before it saved the prompt, for this reason.
    Failed(String),
}

impl PromptStart {
    /// How a turn that ended with `end` before it reported saving its
    /// prompt began, or `None` when the daemon's stop answers the request.
    fn ended(end: &TurnEnd) -> Option<Self> {
        match end {
            TurnEnd::Completed => Some(Self::Saved),
            TurnEnd::Cancelled => Some(Self::Cancelled),
            TurnEnd::Interrupted => None,
            TurnEnd::Failed(error) => Some(Self::Failed(error.clone())),
        }
    }
}

/// One turn of a session whose job already recorded the turn's start.
pub(super) struct TurnTask {
    pub(super) session: Arc<LoadedSession>,
    /// Cancelled once the daemon stops: a turn cancelled then was
    /// interrupted, not cancelled by a client.
    pub(super) stopping: CancellationToken,
    /// Tripped by a cancel sent for this turn, even one sent before the
    /// turn began its prompt.
    pub(super) cancel: CancellationToken,
}

impl TurnTask {
    /// Runs the turn of `text` in a daemon task. The receiver learns how
    /// the turn began; it closes without an answer only once the daemon
    /// stops (see [`PromptStart`]).
    pub(super) fn spawn(self, text: String) -> oneshot::Receiver<PromptStart> {
        let (started, start) = oneshot::channel();
        tokio::spawn(self.run(text, started));
        start
    }

    async fn run(self, text: String, started: oneshot::Sender<PromptStart>) {
        let Self {
            session,
            stopping,
            cancel,
        } = self;
        // Locked for the whole turn and released only after the turn's end
        // is recorded, so the daemon's stop, which waits for the lock, finds
        // every turn it waited for recorded. Free once the session's last
        // turn recorded its end, apart from the moment it takes to release.
        let mut runtime = session.runtime.lock().await;
        // Declared after the lock, so even a panic records the end (see
        // `DaemonView`'s drop) before the lock is released.
        let mut view = DaemonView::new(Arc::clone(&session), started);
        // The prompt that spawned this turn recorded its job running.
        view.publish_state(&JobState::Running);
        let ended = match &mut *runtime {
            Some(runtime) => run_turn(runtime, &mut view, text, &session.config, &cancel).await,
            // Only the daemon's stop takes the runtime, after its last turn.
            None => Ok(TurnOutcome::Cancelled),
        };
        // Before the end is recorded, so the state event that reports it
        // follows the transcript's cut: a client reset from then on gets the
        // saved turn, then how it ended.
        if let Some(runtime) = &*runtime {
            save_transcript(&session, runtime);
        }
        view.end(match ended {
            Ok(TurnOutcome::Completed(_)) => TurnEnd::Completed,
            Ok(TurnOutcome::Cancelled) if stopping.is_cancelled() => TurnEnd::Interrupted,
            Ok(TurnOutcome::Cancelled) => TurnEnd::Cancelled,
            Err(error) => TurnEnd::Failed(format!("{error:#}")),
        });
    }
}

/// Moves what the turn saved into the session's feed transcript, so a
/// client that attaches from now on replays it from the saved rows.
fn save_transcript(session: &LoadedSession, runtime: &SessionRuntime) {
    // A session that has saved nothing yet has no file to replay.
    let Some(saved) = runtime.saved_history() else {
        return;
    };
    match saved.display_rows() {
        Ok(rows) => {
            // A turn that saved no rows stays in the buffer, as one whose
            // rows could not be read does, until a later turn saves.
            if let Some(replayed) = session.live().replay(&rows, saved.cwd(), saved.tools()) {
                session.feed.end_turn(replayed);
            }
        }
        Err(error) => tracing::warn!(
            job = %session.job,
            error = format!("{error:#}"),
            "could not read the turn's saved rows; clients that reset replay it live until a later turn saves"
        ),
    }
}

/// Begins a prompt of `text` on `runtime` and runs its model turns. Slash
/// commands are not expanded yet: the text goes to the model as written.
async fn run_turn(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    text: String,
    config: &Config,
    cancel: &CancellationToken,
) -> anyhow::Result<TurnOutcome> {
    let scope = runtime.begin_prompt(config).await?;
    // A cancel sent before the prompt began only tripped `cancel`: the turn
    // ends here, before it saves the prompt or calls the model, and dropping
    // `scope` ends the prompt. A cancel from here on reaches the prompt
    // through the session's handle.
    if cancel.is_cancelled() {
        return Ok(TurnOutcome::Cancelled);
    }
    let input = UserInput::text(text);
    view.session.live().prompt(&input);
    let turn = Turn {
        display: input.clone(),
        model: input,
        calls: Vec::new(),
    };
    // A turn builds a large future; keep it on the heap.
    Box::pin(runtime.run_prompt_turn(view, &scope, turn, config)).await
}

/// The message a turn records, and a prompt still waiting for it gets,
/// when its task stops without ending it.
const UNEXPECTED_STOP: &str = "the turn stopped unexpectedly; see the daemon log";

/// Shows a session's turn to its clients and its job: the session's input
/// requests follow the turn's, and the request that sent the prompt learns
/// how the turn began.
///
/// Records the turn's end exactly once: through [`Self::end`], or, when the
/// view drops without it, as the turn task panicking, failed with
/// [`UNEXPECTED_STOP`].
struct DaemonView {
    session: Arc<LoadedSession>,
    /// Answered `Saved` once the turn saved its prompt, or with how the
    /// turn ended if it ends first.
    started: Option<oneshot::Sender<PromptStart>>,
    /// Whether the turn's end is recorded.
    ended: bool,
}

impl DaemonView {
    fn new(session: Arc<LoadedSession>, started: oneshot::Sender<PromptStart>) -> Self {
        Self {
            session,
            started: Some(started),
            ended: false,
        }
    }

    /// Records that the turn ended with `end`.
    fn end(mut self, end: TurnEnd) {
        self.finish(end);
    }

    /// Records that the turn ended with `end`, and tells a request still
    /// waiting for the turn to begin.
    fn finish(&mut self, end: TurnEnd) {
        let start = PromptStart::ended(&end);
        // A turn that ended waits for nothing.
        self.session.change_inputs(Inputs::withdraw_all);
        self.session.record(JobEvent::TurnEnded(end));
        self.ended = true;
        if let (Some(started), Some(start)) = (self.started.take(), start) {
            let _ = started.send(start);
        }
    }

    fn publish_state(&self, state: &JobState) {
        self.session.feed.publish([state_event(state)]);
    }

    /// Takes the model turn's events until it drops its sender, opening
    /// and withdrawing input requests whose answers go to `control`. Holds
    /// `control` until then: a turn whose control channel closes fails, and
    /// requests nobody answered yet must stay open.
    async fn show(
        &mut self,
        mut events: mpsc::UnboundedReceiver<SessionEvent>,
        control: mpsc::UnboundedSender<TurnControl>,
    ) {
        while let Some(event) = events.recv().await {
            let wire = self.session.live().map(&event);
            self.session.feed.publish(wire);
            if matches!(event, SessionEvent::MessageInserted(_)) {
                if let Some(started) = self.started.take() {
                    let _ = started.send(PromptStart::Saved);
                }
            }
            self.session
                .change_inputs(|inputs| inputs.note(&control, &event));
        }
    }
}

impl Drop for DaemonView {
    fn drop(&mut self) {
        if !self.ended {
            self.finish(TurnEnd::Failed(UNEXPECTED_STOP.to_owned()));
        }
    }
}

impl TurnView for DaemonView {
    fn open_turn<'v>(
        &'v mut self,
        _runtime: &SessionRuntime,
    ) -> (TurnIo, impl Future<Output = ()> + Send + use<'v>) {
        let (events, event_receiver) = mpsc::unbounded_channel();
        let (control_sender, control) = mpsc::unbounded_channel();
        (
            TurnIo { events, control },
            self.show(event_receiver, control_sender),
        )
    }

    /// The handoff reaches clients with the turn's events, in #1657.
    fn report_plan(
        &mut self,
        _runtime: &SessionRuntime,
        _process_config: &Config,
        _report: PlanReport,
    ) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
