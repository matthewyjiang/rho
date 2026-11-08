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

use super::{
    super::jobs::TurnEnd, cost, inputs::Inputs, options::available_auths, queued::Queued,
    saved_replay, LoadedSession,
};
use crate::{
    app::{
        session_commands::{
            self, BuiltInCommand, CommandScope, CommandSet, Hosting, PromptCommand,
        },
        session_events::{outcome, CompactionEnd},
        session_runtime::{
            PlanReport, PromptScope, SessionEvent, SessionRuntime, TitleJob, Turn, TurnControl,
            TurnIo, TurnOutcome, TurnView, WorkflowStart,
        },
        workflow_cli::discover::StartCommand,
    },
    config::Config,
};

/// How a prompt's turn began, as the request that sent the prompt learns
/// it. A turn that completed began `Saved`. A turn the daemon's stop
/// interrupted before it saved its prompt sends no answer: the request
/// answers `DaemonStopping` once the stop begins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PromptStart {
    /// The turn saved the prompt, or began `/compact`, which saves none,
    /// and runs on.
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

/// The commands a daemon session runs, which its menu lists too: every
/// command a session host runs, so the TUI's own commands are the only ones
/// it refuses.
pub(super) const DAEMON_COMMANDS: CommandSet = CommandSet {
    built_ins: &[BuiltInCommand::Compact, BuiltInCommand::Init],
    workflows: Hosting::Runs,
    agents: Hosting::Runs,
    sessions_label: "daemon sessions",
};

/// How a prompt's turn ran, before the daemon records its end: a model
/// turn and a command that ran to its end both finish with an outcome.
enum Ran {
    Finished(wire::TurnOutcome),
    Cancelled,
}

/// One turn of a session whose job already recorded the turn's start.
pub(super) struct TurnTask {
    pub(super) session: Arc<LoadedSession>,
    /// The turn's place in the queue on the session's runtime, joined as
    /// the turn started (see `Sessions::prompt`).
    pub(super) runtime: Queued<Option<SessionRuntime>>,
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
            runtime,
            stopping,
            cancel,
            limit,
        } = self;
        // Locked for the whole turn and released only after the turn's end
        // is recorded, so the daemon's stop, which waits for the lock, finds
        // every turn it waited for recorded. Free once the session's last
        // turn recorded its end and the settings changes counted before
        // this turn started applied (see `configure`).
        let mut runtime = runtime.acquire().await;
        // Declared after the lock, so even a panic records the end (see
        // `DaemonView`'s drop) before the lock is released.
        let mut view = DaemonView::new(Arc::clone(&session), started);
        // With the limit, in milliseconds, when it cancelled the turn: not
        // when a client's cancel came first and the turn was still winding
        // down at the deadline.
        let (ended, timed_out) = match &mut *runtime {
            // The daemon began stopping while the turn waited for the
            // runtime, behind changes or the last turn: it ends interrupted
            // without beginning its prompt, as the turns the stop stops do.
            Some(_) if stopping.is_cancelled() => (Ok(Ran::Cancelled), None),
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
                session.refresh_options(runtime, &available_auths().await);
                (
                    ended.map_err(|error| failure(&session, runtime, &error)),
                    timed_out,
                )
            }
            // Only the daemon's stop takes the runtime, after its last turn.
            None => (Ok(Ran::Cancelled), None),
        };
        let title = runtime.as_mut().and_then(SessionRuntime::take_title);
        let (outcome, end) = match (ended, timed_out) {
            (Ok(Ran::Finished(outcome)), _) => (Some(outcome), TurnEnd::Completed),
            // The daemon's stop already detached every client, so none
            // hears how the turn ended; the job records it interrupted.
            (Ok(Ran::Cancelled), _) if stopping.is_cancelled() => (None, TurnEnd::Interrupted),
            (Ok(Ran::Cancelled), Some(timeout_ms)) => (
                Some(wire::TurnOutcome::TimedOut { timeout_ms }),
                TurnEnd::TimedOut { timeout_ms },
            ),
            (Ok(Ran::Cancelled), None) => (Some(wire::TurnOutcome::Cancelled), TurnEnd::Cancelled),
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
    client_error(runtime, error)
}

/// `error` on `runtime` as clients are told it, hiding an authentication
/// failure's text, which may echo credentials.
fn client_error(runtime: &SessionRuntime, error: &anyhow::Error) -> wire::TurnError {
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
) -> anyhow::Result<Ran> {
    let scope = runtime.begin_prompt(config).await?;
    // A cancel sent before the prompt began only tripped `cancel`: the turn
    // ends here, before it saves the prompt or calls the model, and dropping
    // `scope` ends the prompt. A cancel from here on reaches the prompt
    // through the session's handle.
    if cancel.is_cancelled() {
        return Ok(Ran::Cancelled);
    }
    let home = crate::paths::home_dir();
    let command = {
        let commands = CommandScope::of(runtime, home.as_deref(), config);
        // Expansion can wait on an MCP server, so a cancel drops it.
        tokio::select! {
            biased;
            () = scope.cancellation().cancelled() => return Ok(Ran::Cancelled),
            command = session_commands::resolve(input.clone(), &commands, DAEMON_COMMANDS) => command,
        }
    };
    match command? {
        PromptCommand::Turn {
            input: model,
            calls,
        } => {
            // The command stays the shown message; the model gets its
            // expansion.
            let turn = Turn {
                display: input,
                model,
                calls,
            };
            model_turn(runtime, view, &scope, turn, config).await
        }
        PromptCommand::Compact { instructions } => {
            compact(runtime, view, &scope, instructions).await
        }
        PromptCommand::StartWorkflow(start) => {
            start_workflow(runtime, view, &scope, input, start, config).await
        }
        PromptCommand::RunAsAgent(task) => {
            runtime.rebind_agent(&scope, &task, config).await?;
            let auths = available_auths().await;
            view.rebound(runtime, &auths);
            // The command stays the shown message; the model gets the task.
            let turn = Turn {
                display: input,
                model: task.input,
                calls: Vec::new(),
            };
            model_turn(runtime, view, &scope, turn, config).await
        }
    }
}

/// Runs `turn` as `scope`'s model turn.
async fn model_turn(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    scope: &PromptScope,
    turn: Turn,
    config: &Config,
) -> anyhow::Result<Ran> {
    view.session.live().prompt(&turn.display);
    // A turn builds a large future; keep it on the heap.
    let ended = Box::pin(runtime.run_prompt_turn(view, scope, turn, config)).await?;
    Ok(match ended {
        TurnOutcome::Completed(run) => Ran::Finished(outcome::finished(&run)),
        TurnOutcome::Cancelled => Ran::Cancelled,
    })
}

/// Runs `/compact` within `scope`, with optional `instructions` on what to
/// keep, and shows it as a model turn's automatic compaction shows: the
/// runtime's compaction reports no run events. It answers with what the
/// compaction did. The command is not saved as a user message: the
/// runtime saves the compacted context alone.
async fn compact(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    scope: &PromptScope,
    instructions: Option<String>,
) -> anyhow::Result<Ran> {
    let started = view.session.live().compaction_started();
    view.session.feed.publish(started);
    // Nothing rejects the command from here on; the prompt's request need
    // not wait for the compaction.
    view.began();
    let compacted = runtime.compact(scope, instructions).await;
    let end = match &compacted {
        Ok(Some(outcome)) => CompactionEnd::Completed(outcome),
        Ok(None) => CompactionEnd::Cancelled,
        Err(error) => CompactionEnd::Failed(client_error(runtime, error).message),
    };
    let ended = view.session.live().compaction_ended(end);
    let ended = cost::priced(ended, &view.session.id).await;
    view.session.feed.publish(ended);
    Ok(match compacted? {
        Some(outcome) => Ran::Finished(wire::TurnOutcome::Answered {
            text: crate::compaction::outcome_summary(&outcome),
            stop: wire::AnswerStop::EndTurn,
        }),
        None => Ran::Cancelled,
    })
}

/// Starts `start`'s workflow within `scope`, without a model turn. The
/// runtime saves the prompt, `display`, with the start's notice as its
/// reply; clients see both as a reload replays them, and the turn answers
/// with the notice.
async fn start_workflow(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    scope: &PromptScope,
    display: UserInput,
    start: StartCommand,
    config: &Config,
) -> anyhow::Result<Ran> {
    let (notice, inserted) = match runtime
        .start_workflow(scope, &display, start, config)
        .await?
    {
        WorkflowStart::Started { notice, inserted } => (notice, inserted),
        WorkflowStart::Cancelled => return Ok(Ran::Cancelled),
    };
    let shown = {
        let mut live = view.session.live();
        live.prompt(&display);
        let mut shown = live.map(&SessionEvent::MessageInserted(inserted.clone()));
        shown.push(live.saved_reply(&inserted, notice.clone()));
        shown
    };
    view.session.feed.publish(shown);
    view.began();
    Ok(Ran::Finished(wire::TurnOutcome::Answered {
        text: notice,
        stop: wire::AnswerStop::EndTurn,
    }))
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
    /// Answered `Saved` once the turn saved its prompt or began `/compact`,
    /// or with how the turn ended if it ends first.
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

    /// Tells the request that sent the prompt, if it still waits, that the
    /// turn began (see [`PromptStart::Saved`]).
    fn began(&mut self) {
        if let Some(started) = self.started.take() {
            let _ = started.send(PromptStart::Saved);
        }
    }

    /// Follows `runtime` once `/agent:` rebound it: its events map with
    /// the agent's tools, and clients see the agent's options, its model
    /// among them, before its turn's output. `auths` as for
    /// [`LoadedSession::refresh_options`].
    fn rebound(&self, runtime: &SessionRuntime, auths: &[String]) {
        self.session.live().rebind_tools(
            runtime.workspace_root(),
            runtime.tools(),
            runtime.configured_context_window(),
        );
        self.session.refresh_options(runtime, auths);
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
                self.began();
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
        let session = Arc::clone(&self.session);
        async move {
            session.refresh_options(runtime, &available_auths().await);
        }
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
