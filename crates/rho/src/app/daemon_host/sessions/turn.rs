//! One prompt's turn, run in a task the daemon owns, so it goes on when the
//! client that sent the prompt disconnects.
//!
//! The turn reports to a [`DaemonView`], which sends its events to the
//! session's attached clients (#1657) and opens and withdraws the session's
//! input requests as the turn asks (#1660): its job is `waiting_for_input`
//! while an approval, plan approval, or question is open, `running`
//! otherwise. A request is never denied because no client is attached.

use std::{
    future::Future,
    sync::{Arc, Weak},
    time::{Duration, SystemTime},
};

use rho_protocol as wire;
use rho_sdk::{CancellationToken, UserInput};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

use super::{super::jobs::TurnEnd, cost, inputs::Inputs, saved_replay, LoadedSession};
use crate::{
    app::{
        session_commands::{
            self, BuiltInCommand, CommandScope, CommandSet, Hosting, PromptCommand,
        },
        session_events::outcome,
        session_runtime::{
            CommandRejected, PlanReport, SessionEvent, SessionRuntime, TitleJob, Turn, TurnControl,
            TurnIo, TurnOutcome, TurnView,
        },
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
    /// The turn ended before it saved the prompt, as this says: cancelled,
    /// timed out, or failed.
    Ended(wire::TurnOutcome),
}

/// A turn's time limit (#1669), counted from when the daemon took its
/// prompt, waiting for the session included: the limit is the caller's
/// wall clock.
#[derive(Clone, Copy, Debug)]
pub(super) struct TurnLimit {
    timeout_ms: u64,
    deadline: Instant,
}

impl TurnLimit {
    pub(super) fn starting_now(timeout_ms: u64) -> Self {
        Self {
            timeout_ms,
            deadline: Instant::now() + Duration::from_millis(timeout_ms),
        }
    }
}

/// Waits until `limit` passes, and returns it in milliseconds; forever
/// without one.
async fn expired(limit: Option<TurnLimit>) -> u64 {
    match limit {
        Some(TurnLimit {
            timeout_ms,
            deadline,
        }) => {
            tokio::time::sleep_until(deadline).await;
            timeout_ms
        }
        None => std::future::pending().await,
    }
}

/// The commands a daemon session runs, which its menu lists too.
/// `/compact`, workflows, and `/agent:` are not among them yet.
pub(super) const DAEMON_COMMANDS: CommandSet = CommandSet {
    built_ins: &[BuiltInCommand::Init],
    workflows: Hosting::Refused,
    agents: Hosting::Refused,
    sessions_label: "daemon sessions",
};

/// One turn of a session whose job already recorded the turn's start.
pub(super) struct TurnTask {
    pub(super) session: Arc<LoadedSession>,
    /// Cancelled once the daemon stops: a turn cancelled then was
    /// interrupted, not cancelled by a client.
    pub(super) stopping: CancellationToken,
    /// Tripped by a cancel sent for this turn, even one sent before the
    /// turn began its prompt.
    pub(super) cancel: CancellationToken,
    /// When the daemon cancels the turn as timed out; `None` sets no limit.
    pub(super) limit: Option<TurnLimit>,
}

impl TurnTask {
    /// Runs the turn of `input` in a daemon task. The receiver learns how
    /// the turn began; it closes without an answer only once the daemon
    /// stops (see [`PromptStart`]).
    pub(super) fn spawn(self, input: UserInput) -> oneshot::Receiver<PromptStart> {
        let (started, start) = oneshot::channel();
        tokio::spawn(self.run(input, started));
        start
    }

    async fn run(self, input: UserInput, started: oneshot::Sender<PromptStart>) {
        let Self {
            session,
            stopping,
            cancel,
            limit,
        } = self;
        // Locked for the whole turn and released only after the turn's end
        // is recorded, so the daemon's stop, which waits for the lock, finds
        // every turn it waited for recorded. Free once the session's last
        // turn recorded its end, apart from the moment it takes to release.
        let mut runtime = session.runtime.lock().await;
        // Declared after the lock, so even a panic records the end (see
        // `DaemonView`'s drop) before the lock is released.
        let mut view = DaemonView::new(Arc::clone(&session), started);
        // With the limit, in milliseconds, when it cancelled the turn: not
        // when a client's cancel came first and the turn was still winding
        // down at the deadline.
        let (ended, timed_out) = match &mut *runtime {
            Some(runtime) => {
                let (ended, timed_out) = {
                    let turn = run_turn(runtime, &mut view, input, &session.config, &cancel);
                    tokio::pin!(turn);
                    // The deadline first, so a limit already past cancels
                    // the turn before it saves its prompt.
                    tokio::select! {
                        biased;
                        timeout_ms = expired(limit) => {
                            let timed_out = (!cancel.is_cancelled()).then_some(timeout_ms);
                            session.cancel_turn();
                            (turn.await, timed_out)
                        }
                        ended = &mut turn => (ended, None),
                    }
                };
                // Before the end is recorded, so the state event that
                // reports it follows the transcript's cut: a client reset
                // from then on gets the saved turn, then how it ended.
                save_transcript(&session, runtime);
                // The turn may have left new skills, prompts, or MCP prompts
                // behind; clients see them before they see the turn end.
                session.refresh_options(runtime);
                (
                    ended.map_err(|error| failure(&session, runtime, &error)),
                    timed_out,
                )
            }
            // Only the daemon's stop takes the runtime, after its last turn.
            None => (Ok(TurnOutcome::Cancelled), None),
        };
        let title = runtime.as_mut().and_then(SessionRuntime::take_title);
        let (outcome, end) = match (ended, timed_out) {
            (Ok(TurnOutcome::Completed(run)), _) => {
                (Some(outcome::finished(&run)), TurnEnd::Completed)
            }
            // The daemon's stop already detached every client, so none
            // hears how the turn ended; the job records it interrupted.
            (Ok(TurnOutcome::Cancelled), _) if stopping.is_cancelled() => {
                (None, TurnEnd::Interrupted)
            }
            (Ok(TurnOutcome::Cancelled), Some(timeout_ms)) => (
                Some(wire::TurnOutcome::TimedOut { timeout_ms }),
                TurnEnd::TimedOut { timeout_ms },
            ),
            (Ok(TurnOutcome::Cancelled), None) => {
                (Some(wire::TurnOutcome::Cancelled), TurnEnd::Cancelled)
            }
            (Err(error), _) => {
                let (outcome, end) = failed(error);
                (Some(outcome), end)
            }
        };
        view.end(outcome, end);
        // After the end, so the title follows the turn's events.
        if let Some(title) = title {
            tokio::spawn(report_title(Arc::downgrade(&session), title));
        }
    }
}

/// The error a turn that failed with `error` on `runtime` reports. Logged
/// whole, to the daemon's own log only: the error it reports hides an
/// authentication failure's text, which may echo credentials, from clients.
fn failure(
    session: &LoadedSession,
    runtime: &SessionRuntime,
    error: &anyhow::Error,
) -> wire::TurnError {
    tracing::warn!(job = %session.job, error = format!("{error:#}"), "a turn failed");
    // The model the turn ran on, which a configure may have switched from
    // the config's.
    let current = runtime.current_model();
    outcome::turn_error(error, &current.provider, &current.auth)
}

/// Moves what the turn saved into the session's feed transcript, so a
/// client that attaches from now on replays it from the saved rows.
fn save_transcript(session: &LoadedSession, runtime: &SessionRuntime) {
    // A turn that saved no rows stays in the buffer, as one whose rows could
    // not be read does, until a later turn saves.
    if let Some(saved) = saved_replay(&mut session.live(), runtime, &session.job) {
        session.feed.end_turn(saved);
    }
}

/// Generates the title the session's first answered turn asked for, which
/// saves it, and tells the session's clients (#1670). It lands seconds
/// after the turn; a session the daemon unloaded by then is titled all the
/// same, with no clients to tell. Holds the session weakly, so a pending
/// title never keeps an unloaded session's feed alive; a session reloaded
/// once the title saved starts its feed with it, and reports it on attach.
async fn report_title(session: Weak<LoadedSession>, title: TitleJob) {
    if let Some(title) = title.generate().await {
        if let Some(session) = session.upgrade() {
            session
                .feed
                .publish([wire::SessionEvent::TitleChanged { title }]);
        }
    }
}

/// Begins a prompt of `input` on `runtime` and runs it: its slash command
/// when it starts with one the daemon runs, else a model turn on it. A
/// command the daemon cannot run fails the turn as a configuration error
/// before the prompt is saved.
async fn run_turn(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    input: UserInput,
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
    let home = crate::paths::home_dir();
    let command = {
        let commands = CommandScope::of(runtime, home.as_deref(), config);
        // Expansion can wait on an MCP server, so a cancel drops it.
        tokio::select! {
            biased;
            () = scope.cancellation().cancelled() => return Ok(TurnOutcome::Cancelled),
            command = session_commands::resolve(input.clone(), &commands, DAEMON_COMMANDS) => command,
        }
    };
    let (model, calls) = match command? {
        PromptCommand::Turn { input, calls } => (input, calls),
        // `DAEMON_COMMANDS` runs no `/compact`, workflow, or `/agent:`, so
        // the resolver rejects them first.
        PromptCommand::Compact { .. }
        | PromptCommand::StartWorkflow(_)
        | PromptCommand::RunAsAgent(_) => {
            return Err(CommandRejected(
                "could not run the command: not available in daemon sessions".into(),
            )
            .into())
        }
    };
    view.session.live().prompt(&input);
    // The command stays the shown message; the model gets its expansion.
    let turn = Turn {
        display: input,
        model,
        calls,
    };
    // A turn builds a large future; keep it on the heap.
    Box::pin(runtime.run_prompt_turn(view, &scope, turn, config)).await
}

/// The outcome of a turn that failed with `error`, and the end its job
/// records: failed, with the error's message.
fn failed(error: wire::TurnError) -> (wire::TurnOutcome, TurnEnd) {
    let end = TurnEnd::Failed(error.message.clone());
    (wire::TurnOutcome::Failed { error }, end)
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

    /// Records that the turn ended as `end`, telling clients `outcome`
    /// when there are clients to tell (`None` while the daemon stops).
    fn end(mut self, outcome: Option<wire::TurnOutcome>, end: TurnEnd) {
        self.finish(outcome, end);
    }

    /// Tells clients the turn ended with `outcome`, if any, records it as
    /// `end`, and tells a request still waiting for the turn to begin.
    fn finish(&mut self, outcome: Option<wire::TurnOutcome>, end: TurnEnd) {
        // How the turn began, if it had not reported saving its prompt yet.
        let start = match end {
            TurnEnd::Completed => Some(PromptStart::Saved),
            TurnEnd::Interrupted => None,
            TurnEnd::Cancelled | TurnEnd::TimedOut { .. } | TurnEnd::Failed(_) => {
                outcome.clone().map(PromptStart::Ended)
            }
        };
        // A turn that ended waits for nothing.
        self.session.change_inputs(Inputs::withdraw_all);
        {
            // See `prompt`: the next turn's `running` cannot come before this.
            let _turn = self.session.turn();
            self.session.end_turn(outcome, end);
        }
        self.ended = true;
        if let (Some(started), Some(start)) = (self.started.take(), start) {
            let _ = started.send(start);
        }
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
            let wire = cost::priced(wire, &self.session.id).await;
            self.session.feed.publish(wire);
            if matches!(event, SessionEvent::MessageInserted(_)) {
                if let Some(started) = self.started.take() {
                    let _ = started.send(PromptStart::Saved);
                }
            }
            self.session
                .change_inputs(|inputs| inputs.note(&control, &event, SystemTime::now()));
        }
    }
}

impl Drop for DaemonView {
    fn drop(&mut self) {
        if !self.ended {
            let (outcome, end) = failed(wire::TurnError {
                kind: wire::TurnErrorKind::Other,
                message: UNEXPECTED_STOP.to_owned(),
            });
            self.finish(Some(outcome), end);
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

    /// The handoff reaches clients with the turn's events, in #1657. An
    /// approval that switched the permission mode changes the session's
    /// options, which clients see before the implementation turn.
    fn report_plan(
        &mut self,
        runtime: &SessionRuntime,
        _process_config: &Config,
        _report: PlanReport,
    ) -> impl Future<Output = ()> + Send {
        self.session.refresh_options(runtime);
        std::future::ready(())
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
