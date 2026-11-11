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
use rho_sdk::{CancellationToken, RunEvent, UserInput};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

use super::{
    super::jobs::TurnEnd, budget, cost, inputs::Inputs, options::available_auths, saved_replay,
    start::StartedTurn, LoadedSession,
};
use crate::{
    app::{
        session_commands::{
            self, BuiltInCommand, CommandScope, CommandSet, Hosting, PromptCommand,
        },
        session_events::{outcome, CompactionEnd},
        session_runtime::{
            InsertedMessage, PlanReport, PromptScope, SessionEvent, SessionRuntime, TitleJob, Turn,
            TurnControl, TurnIo, TurnOutcome, TurnView, UnsavedCompaction, WorkflowStart,
        },
        workflow_cli::discover::WorkflowLaunch,
    },
    config::Config,
};
use wake::WakePrompt;

/// How a prompt's turn began, as the request that sent the prompt, and one
/// that sends it again with its key, learn it. A turn that completed
/// `Began`. A turn the daemon's stop interrupted before it began stays
/// `Pending`: the request answers `DaemonStopping` once the stop begins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PromptStart {
    /// The turn has neither begun nor ended yet.
    Pending,
    /// The turn began: it saved its prompt, or, for `/compact`, its
    /// compaction started; or it must wait for capacity in the daemon-wide
    /// budget (see [`TurnTask::budget`]). It runs on.
    Began,
    /// The turn ended before it began, as this says: cancelled, timed out,
    /// or failed.
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
/// turn and a command that ran to its end both complete with an outcome.
pub(super) enum Ran {
    Completed(wire::TurnOutcome),
    Cancelled,
}

enum TurnInput {
    Prompt(UserInput),
    Goal(Turn),
}

/// One turn of a session whose job already recorded the turn's start.
pub(super) struct TurnTask {
    pub(super) session: Arc<LoadedSession>,
    /// The turn as it started: its id, its cancel, which a cancel sent for
    /// this turn trips even before the turn began its prompt, and its
    /// places in the queue on the session's runtime and, for a session
    /// another session started (#1714, see `budget`), in the daemon-wide
    /// budget's line.
    pub(super) turn: StartedTurn,
    /// Cancelled once the daemon stops: a turn cancelled then was
    /// interrupted, not cancelled by a client.
    pub(super) stopping: CancellationToken,
    /// When the daemon cancels the turn as timed out; `None` sets no limit.
    pub(super) limit: Option<TurnLimit>,
}

impl TurnTask {
    /// Runs the turn of `input` in a daemon task, which tells
    /// [`StartedTurn::start`] how the turn began.
    pub(super) fn spawn(self, input: UserInput) {
        tokio::spawn(self.run(TurnInput::Prompt(input)));
    }

    /// Start a host-constructed goal turn without parsing its displayed command.
    pub(super) fn spawn_goal(self, input: Turn) {
        tokio::spawn(self.run(TurnInput::Goal(input)));
    }

    async fn run(self, input: TurnInput) {
        let Self {
            session,
            turn:
                StartedTurn {
                    id,
                    cancel,
                    runtime,
                    budget,
                    start,
                },
            stopping,
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
        let mut view = DaemonView::new(Arc::clone(&session), id, start);
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
                    let turn = async {
                        let Ok(slot) = budget::admit(&mut view, budget, &cancel, &stopping).await
                        else {
                            return Ok(Ran::Cancelled);
                        };
                        // Global settings that changed since the last turn,
                        // or while this one waited for its slot, reach the
                        // session now, before its prompt.
                        let config = session.config_for_turn(runtime).await;
                        let turn = async {
                            let ended = match input {
                                TurnInput::Prompt(input) => {
                                    run_turn(runtime, &mut view, input, &config, &cancel).await
                                }
                                TurnInput::Goal(turn) => {
                                    run_goal_turn(runtime, &mut view, turn.into(), &config, &cancel)
                                        .await
                                }
                            };
                            super::goal::drive(
                                runtime,
                                &mut view,
                                ended,
                                super::goal::Halt {
                                    turn_cancel: &cancel,
                                    stopping: &stopping,
                                },
                            )
                            .await
                        };
                        slot.lend_while_waiting(&session.jobs, &session.job, turn)
                            .await
                    };
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
                session.report_goal_outcome();
                // The turn may have left new skills, prompts, or MCP prompts
                // behind; clients see them before they see the turn end.
                session.refresh_options(runtime, &available_auths(runtime.credentials()).await);
                wake::prompt_ended(&session, runtime, &stopping).await;
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
            (Ok(Ran::Completed(outcome)), _) => (Some(outcome), TurnEnd::Completed),
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
pub(super) fn client_error(runtime: &SessionRuntime, error: &anyhow::Error) -> wire::TurnError {
    // The model the turn ran on, which a configure may have switched from
    // the config's.
    let current = runtime.current_model();
    outcome::turn_error(error, &current.provider, &current.auth)
}

/// Moves what the session saved since the last call, a turn's rows or
/// context added between turns, into its feed transcript, so a client that
/// attaches from now on replays it from the saved rows.
pub(super) fn save_transcript(session: &LoadedSession, runtime: &SessionRuntime) {
    // A turn that saved no rows stays in the buffer, as one whose rows could
    // not be read does, until a later turn saves.
    if let Some(saved) = saved_replay(&mut session.live(), runtime, &session.job) {
        session.feed.end_turn(saved);
    }
}

/// Replaces the feed's transcript with the rows the session's file replays
/// now that its conversation moved to another node of its tree, and
/// detaches the session's clients to replay it (see [`Feed::restart`]).
/// The new stream then says that the session rests with no turn to show,
/// since the job's last turn may not be part of the conversation any more
/// and its answer must not show over it, and how full the context is now.
/// The job forgets that turn's outcome for the same reason, so a reopened
/// session's feed does not replay it either.
///
/// [`Feed::restart`]: super::feed::Feed::restart
pub(super) async fn restart_transcript(session: &LoadedSession, runtime: &SessionRuntime) {
    let (saved, usage) = {
        let mut live = session.live();
        live.forget_replayed();
        (
            saved_replay(&mut live, runtime, &session.job),
            live.context_usage(runtime.context_estimate()),
        )
    };
    // The conversation moved either way, so the old transcript cannot
    // stay. Rows that could not be read leave it empty, which must not
    // read as a conversation with nothing in it.
    let unread = saved.is_none()
        && runtime
            .saved_history()
            .is_some_and(|saved| saved.display_rows().is_err());
    let usage = cost::priced(vec![usage], &session.id).await;
    session.jobs.forget_outcome(&session.job);
    session.feed.restart(saved.unwrap_or_default());
    let rests = wire::SessionEvent::State {
        state: wire::JobState::Idle,
        turn: None,
    };
    let unread = unread.then(|| wire::SessionEvent::Notice {
        text: "could not read the conversation's saved rows; the next turn still continues from it"
            .into(),
    });
    session
        .feed
        .publish([rests].into_iter().chain(usage).chain(unread));
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

/// Begins a prompt of `input` on `runtime` and runs it: the T3 wake
/// watcher's wait when it is one, its slash command when it starts with one
/// the daemon runs, else a model turn on it. A command the daemon cannot run
/// fails the turn as a configuration error before the prompt is saved.
async fn run_turn(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    input: UserInput,
    config: &Config,
    cancel: &CancellationToken,
) -> anyhow::Result<Ran> {
    let wake_prompt = wake::prompt_started(&view.session, &input).await;
    let scope = match runtime.begin_prompt(config).await {
        Ok(scope) => scope,
        // A daemon session loads again only once the daemon unloads it.
        Err(error) if error.is::<UnsavedCompaction>() => anyhow::bail!(UNSAVED_COMPACTION),
        Err(error) => return Err(error),
    };
    // A cancel sent before the prompt began only tripped `cancel`: the turn
    // ends here, before it saves the prompt or calls the model, and dropping
    // `scope` ends the prompt. A cancel from here on reaches the prompt
    // through the session's handle.
    if cancel.is_cancelled() {
        return Ok(Ran::Cancelled);
    }
    let origin = match wake_prompt {
        WakePrompt::Prompt(origin) => origin,
        WakePrompt::Watch(signal) => return wake::watch(view, &scope, &input, signal?).await,
    };
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
    let mut turn = match command? {
        PromptCommand::Turn {
            input: model,
            calls,
        } => Turn {
            display: input,
            model: wake::model_input(&origin, model),
            calls,
        },
        PromptCommand::RunAsAgent(task) => {
            runtime.rebind_agent(&scope, &task, config).await?;
            // Events map with the agent's tools from now on, and clients
            // see the agent's options, its model among them, before its
            // turn's output.
            view.session
                .live()
                .rebind_tools(runtime.tools(), runtime.configured_context_window());
            view.session
                .refresh_options(runtime, &available_auths(runtime.credentials()).await);
            Turn {
                display: input,
                model: task.input,
                calls: Vec::new(),
            }
        }
        PromptCommand::Compact { instructions } => {
            return compact(runtime, view, &scope, &input, instructions).await;
        }
        PromptCommand::StartWorkflow(start) => {
            return start_workflow(runtime, view, &scope, input, start, config).await;
        }
    };
    // The command stays the shown message; the model gets its expansion.
    super::goal::prepare_turn(&view.session, &mut turn);
    view.session.live().prompt(&turn.display);
    // A turn builds a large future; keep it on the heap.
    let ended = Box::pin(runtime.run_prompt_turn(view, &scope, turn, config)).await?;
    Ok(match ended {
        TurnOutcome::Completed(run) => Ran::Completed(outcome::finished(&run)),
        TurnOutcome::Cancelled => Ran::Cancelled,
    })
}

/// Run a goal continuation or retry with normal persistence, child boundary
/// delivery, cancellation, and plan handoff. A retry inserts no duplicate prompt.
pub(super) async fn run_goal_turn(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    input: crate::app::session_runtime::TurnStart,
    config: &Config,
    cancel: &CancellationToken,
) -> anyhow::Result<Ran> {
    let scope = runtime.begin_prompt(config).await?;
    if cancel.is_cancelled() {
        return Ok(Ran::Cancelled);
    }
    if let crate::app::session_runtime::TurnStart::Prompt(turn) = &input {
        view.session.live().prompt(&turn.display);
    }
    let ended = Box::pin(runtime.run_prompt_turn(view, &scope, input, config)).await?;
    Ok(match ended {
        TurnOutcome::Completed(run) => Ran::Completed(outcome::finished(&run)),
        TurnOutcome::Cancelled => Ran::Cancelled,
    })
}

/// Runs `/compact` within `scope`, with optional `instructions` on what to
/// keep. Clients see the command, `display`, then the compaction as a model
/// turn's automatic compaction shows (the runtime's compaction reports no
/// run events), then what it did as an agent message, which the turn
/// answers with too. Neither message is saved: the runtime saves the
/// compacted context alone, which a reload replays as its own row.
async fn compact(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    scope: &PromptScope,
    display: &UserInput,
    instructions: Option<String>,
) -> anyhow::Result<Ran> {
    let started = {
        let mut live = view.session.live();
        let mut shown = vec![live.shown_prompt(display, &InsertedMessage::Unreplayed)];
        shown.extend(live.compaction_started());
        shown
    };
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
    let summary = compacted
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(crate::compaction::outcome_summary);
    let ended = {
        let mut live = view.session.live();
        let mut ended = live.compaction_ended(end);
        ended.extend(
            summary
                .clone()
                .map(|text| live.shown_reply(&InsertedMessage::Unreplayed, text)),
        );
        ended
    };
    let ended = cost::priced(ended, &view.session.id).await;
    view.session.feed.publish(ended);
    // A failure's error reaches clients again, redacted, in the turn's end.
    compacted?;
    Ok(match summary {
        Some(text) => Ran::Completed(wire::TurnOutcome::Answered {
            text,
            stop: wire::AnswerStop::EndTurn,
        }),
        None => Ran::Cancelled,
    })
}

/// Starts `launch`'s workflow run within `scope`, without a model turn. The
/// runtime saves the prompt, `display`, with the start's notice as its
/// reply; clients see both as a reload replays them, and the turn answers
/// with the notice.
async fn start_workflow(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    scope: &PromptScope,
    display: UserInput,
    launch: WorkflowLaunch,
    config: &Config,
) -> anyhow::Result<Ran> {
    let (notice, inserted, reply) = match runtime
        .start_workflow(scope, &display, launch, config)
        .await?
    {
        WorkflowStart::Started {
            notice,
            inserted,
            reply,
        } => (notice, inserted, reply),
        WorkflowStart::Cancelled => return Ok(Ran::Cancelled),
    };
    let shown = {
        let mut live = view.session.live();
        vec![
            live.shown_prompt(&display, &inserted),
            live.shown_reply(&reply, notice.clone()),
        ]
    };
    view.session.feed.publish(shown);
    view.began();
    Ok(Ran::Completed(wire::TurnOutcome::Answered {
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

/// Why a session whose compacted context could not be saved refuses every
/// prompt, and what reloads it from its file.
const UNSAVED_COMPACTION: &str =
    "could not continue session: its compacted context was not saved; \
detach every client and wait for the daemon to unload the idle session, so the next prompt or \
attach reopens it from its file";

/// Shows a session's turn to its clients and its job: the session's input
/// requests follow the turn's, and the request that sent the prompt learns
/// how the turn began.
///
/// Records the turn's end exactly once: through [`Self::end`], or, when the
/// view drops without it, as the turn task panicking, failed with
/// [`UNEXPECTED_STOP`].
pub(super) struct DaemonView {
    pub(super) session: Arc<LoadedSession>,
    /// The turn's id, which its end is told under.
    turn: wire::TurnId,
    /// Told `Began` once the turn began (see [`PromptStart::Began`]), or
    /// how the turn ended if it ends first.
    start: watch::Sender<PromptStart>,
    /// Whether the turn's end is recorded.
    ended: bool,
}

impl DaemonView {
    fn new(
        session: Arc<LoadedSession>,
        turn: wire::TurnId,
        start: watch::Sender<PromptStart>,
    ) -> Self {
        Self {
            session,
            turn,
            start,
            ended: false,
        }
    }

    /// Tells whoever waits for the turn to begin that it began as `start`,
    /// unless they were told already.
    fn tell_start(&self, start: PromptStart) {
        self.start.send_if_modified(|told| {
            let first = *told == PromptStart::Pending;
            if first {
                *told = start;
            }
            first
        });
    }

    /// Records that the turn ended as `end`, telling clients `outcome`
    /// when there are clients to tell (`None` while the daemon stops).
    fn end(mut self, outcome: Option<wire::TurnOutcome>, end: TurnEnd) {
        self.finish(outcome, end);
    }

    /// Tells clients the turn ended with `outcome`, if any, records it as
    /// `end`, and tells a request still waiting for the turn to begin.
    fn finish(&mut self, outcome: Option<wire::TurnOutcome>, end: TurnEnd) {
        // How the turn began, if it had not reported beginning yet. A
        // turn that completed began, and is recorded so (see `began`).
        let start = match end {
            TurnEnd::Completed => Some(PromptStart::Began),
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
            self.session.end_turn(self.turn.clone(), outcome, end);
        }
        self.ended = true;
        match start {
            Some(PromptStart::Began) => self.began(),
            Some(start @ PromptStart::Ended(_)) => self.tell_start(start),
            Some(PromptStart::Pending) | None => {}
        }
    }

    /// Tells the request that sent the prompt, if it still waits, that the
    /// turn began (see [`PromptStart::Began`]), and records it on the job,
    /// so a later daemon still knows the prompt as one that ran.
    ///
    /// Nothing a turn does before this needs a client: `rho acp` reads none
    /// of the session's events while its prompt waits for this answer.
    pub(super) fn began(&mut self) {
        // Recorded before the request is answered, so a daemon killed right
        // after its answer still left the turn recorded as begun. The
        // registry records it once, and only on this turn.
        self.session.jobs.turn_began(&self.session.job, &self.turn);
        self.tell_start(PromptStart::Began);
    }

    /// Tells the request that sent the prompt that the turn began, though
    /// it only waits for capacity: it has run nothing, so the job does not
    /// record it begun until it does ([`Self::began`]).
    pub(super) fn accepted(&mut self) {
        self.tell_start(PromptStart::Began);
    }

    /// Takes the model turn's events until it drops its sender, opening
    /// and withdrawing input requests whose answers go to `control`. Holds
    /// `control` until then: a turn whose control channel closes fails, and
    /// requests nobody answered yet must stay open.
    async fn show(
        &mut self,
        mut events: mpsc::UnboundedReceiver<SessionEvent>,
        control: mpsc::UnboundedSender<TurnControl>,
        mut cost: cost::TurnCost,
    ) {
        loop {
            // A streamed card held back for more arguments goes out when
            // the stream pauses instead.
            let event = if self.session.live().holds_previews() {
                match tokio::time::timeout(PREVIEW_PAUSE, events.recv()).await {
                    Ok(event) => event,
                    Err(_) => {
                        let held = self.session.live().held_previews();
                        self.session.feed.publish(held);
                        continue;
                    }
                }
            } else {
                events.recv().await
            };
            let Some(event) = event else {
                break;
            };
            let wire = {
                let mut live = self.session.live();
                let mut wire = live.map(&event);
                if matches!(event, SessionEvent::Run(RunEvent::UsageUpdated { .. })) {
                    wire.extend(live.streamed_usage());
                }
                wire
            };
            let wire = cost.priced(&event, wire, &self.session.id).await;
            self.session.feed.publish(wire);
            if matches!(event, SessionEvent::MessageInserted(_)) {
                self.began();
            }
            self.session
                .change_inputs(|inputs| inputs.note(&control, &event, SystemTime::now()));
        }
    }
}

/// How long a model's stream may pause before the card of what it wrote
/// last is sent though its arguments grew little: long enough that a card
/// per pause stays at four a second for a stream that only trickles, short
/// enough to read as the stream itself.
const PREVIEW_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

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
        runtime: &SessionRuntime,
    ) -> (TurnIo, impl Future<Output = ()> + Send + use<'v>) {
        let (events, event_receiver) = mpsc::unbounded_channel();
        let (control_sender, control) = mpsc::unbounded_channel();
        (
            TurnIo { events, control },
            self.show(
                event_receiver,
                control_sender,
                cost::TurnCost::on(&runtime.current_model()),
            ),
        )
    }

    /// A plan the session kept its mode for reaches clients as a notice
    /// saying why. An approval that switched the permission mode changes
    /// the session's options, which clients see before the implementation
    /// turn.
    fn report_plan(
        &mut self,
        runtime: &SessionRuntime,
        _process_config: &Config,
        report: PlanReport,
    ) -> impl Future<Output = ()> + Send {
        let session = Arc::clone(&self.session);
        let notice = match report {
            PlanReport::Kept { notice } => Some(wire::SessionEvent::Notice { text: notice }),
            PlanReport::Switched => {
                // Before the implementation turn starts, so only a change
                // queued during the proposal sees the switch.
                session.turn().plan_switches += 1;
                None
            }
        };
        async move {
            session.feed.publish(notice);
            session.refresh_options(runtime, &available_auths(runtime.credentials()).await);
        }
    }
}

#[path = "turn_wake.rs"]
mod wake;

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
