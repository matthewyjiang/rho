//! A daemon session's events as the transcript changes the TUI shows (#1667).
//!
//! [`DaemonEvents`] reads one session's [`SessionEvent`]s, replayed or live,
//! and says what each changes as [`DaemonView`]s, which `daemon_transcript`
//! applies through the TUI's own transcript paths: history entries for
//! messages, notices, and finished tool cards, and the live tool batch for
//! cards still running. The daemon draws each tool card with the shared
//! presenter (`ToolCallUpdate::display`).
//!
//! The job's state says whether a turn runs ([`TurnState`]), which drives
//! the TUI's busy chrome; steers carry their id, so the TUI's pending panel
//! drops a steer once the daemon applied it. A reset's replayed state is history:
//! the attach says which turn the replay ends in, by id, and only that
//! turn, while under way, shows busy before the replay is over, so the TUI
//! never times or announces a turn it only read back (#1777).
//!
//! Input requests open and close through [`OpenInputs`], which keeps those
//! still open, so the ones a replay leaves open show once it ends
//! ([`DaemonEvents::end_replay`]).
//!
//! Otherwise it holds only what the events leave implicit: which message the
//! last text chunk extended, the reply the last turn streamed, each open
//! call's last card, which a call cut off shows as interrupted, and why a
//! turn failed, which its job state leaves out.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use rho_protocol::{
    display::{Presentation, ToolCard, ToolCardStatus},
    CompactionStatus, CompactionTrigger, ContentBlock, InputRequest, InputRequestId, JobState,
    LoginId, LoginOutcome, LoginPrompt, MessageId, PlanEntry, PlanStatus, Replay, RetryReason,
    SessionEvent, SteerId, ToolCallId as WireCallId, ToolCallUpdate, ToolStatus, TurnId,
    TurnOutcome,
};
use rho_sdk::{ProviderErrorKind, ProviderStreamResetReason, SteeringId, ToolCallId};

use super::{
    activity::ProviderRetryHint,
    compaction_display::{CompactionDisplayFacts, CompactionUiOutcome},
    daemon_inputs::{AnswerRefusal, OpenInputs},
    transcript_events::final_answer_delta,
    FinalAnswerDelta, StreamKind,
};
use crate::{
    cache_warm::WarmProgress,
    tools::todo::{TodoItem, TodoList, TodoStatus},
};

/// Why the daemon did not answer a report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReportRefusal {
    pub(super) message: String,
    /// The session was in a turn or a change: the report can be read once
    /// that ends.
    pub(super) busy: bool,
}

impl From<String> for ReportRefusal {
    fn from(message: String) -> Self {
        Self {
            message,
            busy: false,
        }
    }
}

/// One change to what the TUI shows of a daemon session.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum DaemonView {
    /// Drop the transcript, live cards, and checklist: a reset replay
    /// starts the session over.
    Clear,
    /// Drop input prompts before rebuilding them from a tail replay.
    InputsReset,
    /// A user message, as the transcript shows it.
    User(String),
    /// More of the agent's reply or reasoning. `opens` starts a new
    /// transcript row, as a new message does, instead of extending the last.
    /// `replayed` text was said before this TUI followed: history, to
    /// write as it stands rather than stream.
    Text {
        kind: StreamKind,
        text: String,
        opens: bool,
        replayed: bool,
    },
    /// A tool call's running card, opened or replaced.
    ToolRunning {
        call: ToolCallId,
        card: ToolCard,
    },
    /// A tool call closed with this presentation.
    ToolFinished {
        call: ToolCallId,
        presentation: Presentation,
    },
    /// A compaction started: its card opens. `command` when a `/compact`
    /// prompt started it, so the turn is the compaction.
    CompactionStarted {
        command: bool,
    },
    /// The compaction under way ended: its card closes, and the status
    /// line says how.
    CompactionEnded(CompactionUiOutcome),
    /// The turn a `/compact` prompt started is over, live or replayed:
    /// what follows shows as any turn's does.
    CompactTurnOver,
    /// The session's checklist, replacing the last; `None` while it has none.
    Plan(Option<TodoList>),
    /// How full the model's context is.
    Context {
        used_tokens: u64,
        window_tokens: Option<u64>,
    },
    /// The daemon's cumulative session spend, independent of context tokens.
    Spend(Option<u64>),
    /// The host's keep-warm: its ping counter while it warms the prompt
    /// cache, and what its pings have cost since the daemon loaded the
    /// session, which [`Self::Spend`] counts too.
    CacheWarm {
        warming: Option<WarmProgress>,
        cost_usd_micros: u64,
    },
    Title(String),
    Options(Box<rho_protocol::SessionOptions>),
    /// On-demand inventory response; ignored if the person closed the picker.
    McpInventory(Result<Box<rho_protocol::SessionMcpInventory>, String>),
    /// The session's MCP servers stopped connecting, so a held prompt may
    /// go.
    ToolSourcesLoaded,
    /// A report the person asked for, as the daemon presents it.
    Report {
        report: rho_protocol::SessionReport,
        result: Result<serde_json::Value, ReportRefusal>,
    },
    /// What the interactive login the TUI started in the daemon did.
    Login(LoginUpdate),
    /// The daemon's jobs, asked for to refresh the subagent rail.
    Jobs(Result<Vec<rho_protocol::JobSummary>, String>),
    /// The session's processes, asked for to refresh the process rail, and
    /// the output `asked` for the peek view.
    Processes {
        asked: Option<rho_protocol::ProcessOutputParams>,
        result: Result<rho_protocol::SessionProcessesResponse, String>,
    },
    /// The daemon's settings, read once the session attached and again on
    /// each reconnect, or why they could not be read.
    SettingsLoaded(Result<Box<super::daemon_settings::DaemonSettings>, String>),
    /// Settings of a namespace the TUI read changed.
    SettingsChanged(Box<rho_protocol::ConfigChangedParams>),
    /// One background configure request finished, successfully or otherwise.
    ConfigureFinished,
    /// Whether a turn runs, as the job's state says.
    Turn(TurnState),
    /// The steer the TUI sent with this id applied: the user row that
    /// follows shows it.
    SteerApplied(SteeringId),
    /// The provider took the steer mid-request; it can no longer be
    /// withdrawn.
    SteerDelivered(SteeringId),
    /// An input request to put to the person: opened live, or still open
    /// when the replay ended.
    InputOpened(Box<InputRequest>),
    /// Input request `id` closed without the TUI's answer; `notice` says
    /// how.
    InputClosed {
        id: InputRequestId,
        notice: String,
    },
    /// Input request `id` no longer times out: a client held it.
    InputHeld(InputRequestId),
    /// The turn under way started at this instant, before the TUI followed
    /// it: its clock counts from there.
    TurnSince(Instant),
    /// The turn sent the provider a model request and waits for its
    /// response, as an in-process step start shows.
    Requesting,
    /// The turn asks the provider again, and why when the daemon said
    /// more than that it retries.
    Retrying(Option<ProviderRetryHint>),
    /// Status-line feedback.
    Status(String),
    Notice(String),
    /// A background result or a child's message the model was given, as
    /// the card an in-process turn shows for it.
    Card(Box<rho_protocol::display::NotificationCard>),
    Error(String),
}

/// A step of an interactive login the daemon runs for the TUI.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum LoginUpdate {
    /// What the person must do to finish it.
    Prompt { login: LoginId, prompt: LoginPrompt },
    Finished {
        login: LoginId,
        outcome: LoginOutcome,
    },
    /// The connection it belonged to closed, which cancelled it.
    Lost,
}

/// Whether the session runs a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TurnState {
    /// A turn runs.
    Running,
    /// The turn waits for an answer to one of its input requests.
    WaitingForInput,
    /// No turn runs; the last one ended so.
    Resting(Rest),
}

/// How a session at rest got there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Rest {
    /// No turn has run, or the last one completed.
    Ready,
    /// The last turn was cancelled, or the daemon stopped before it ended.
    Interrupted,
    /// The last turn failed, or ran out of time, for this reason.
    Failed { error: String },
    /// The TUI no longer follows the session: the attachment ended, and an
    /// error row says why. Its turn may still run in the daemon.
    Unfollowed,
}

/// Reads one daemon session's events into [`DaemonView`]s.
#[derive(Default)]
pub(super) struct DaemonEvents {
    /// The message the last text chunk extended, and its kind.
    open_message: Option<(StreamKind, MessageId)>,
    /// What the turn's last agent message said so far, which its answer
    /// completes when chunks went missing.
    reply: String,
    /// Whether the step that streamed `reply` went on: it called tools, or
    /// the model was given a background result. A turn's answer is its
    /// last agent message, so after either an answer other than `reply` is
    /// a later step's whose text never arrived in chunks.
    stepped_since_reply: bool,
    /// Each open call's last running card.
    running: HashMap<WireCallId, ToolCard>,
    /// Why a turn failed, which that turn's `failed` job state shows.
    failure: Option<(TurnId, String)>,
    compact_turn: CompactTurn,
    /// Where the replay under way ends, as its attach answered: the job's
    /// state, and the turn it is of. `None` once the replay is over.
    replay_end: Option<(JobState, Option<TurnId>)>,
    /// When the turn the replay ends under way started, as its attach
    /// answered; taken once the replay is over.
    turn_since: Option<Instant>,
    /// Whether the last replay is a reset: history, where a tail holds
    /// only events the TUI has yet to see.
    reset: bool,
    inputs: OpenInputs,
}

/// Where a turn that a `/compact` prompt started stands, from its
/// compaction starting until the TUI is told the turn is over
/// ([`DaemonView::CompactTurnOver`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CompactTurn {
    #[default]
    None,
    UnderWay,
    /// Its job rested in a reset's history, which shows no rest. The TUI
    /// may still show the turn busy, so it is over only once the replay's
    /// end rests it, or something later in the replay follows it.
    RestedUnshown,
}

impl DaemonEvents {
    /// A reset starts over; a tail preserves rows and open cards, but
    /// rebuilds input prompts from the attachment and its replay.
    pub(super) fn replay(&mut self, replay: Replay) -> DaemonView {
        match replay {
            Replay::Reset => {
                *self = Self {
                    reset: true,
                    ..Self::default()
                };
                DaemonView::Clear
            }
            Replay::Tail => {
                self.reset = false;
                DaemonView::InputsReset
            }
        }
    }

    /// Starts an attachment whose response lists `inputs` as the requests
    /// open now and says its replay ends with the job in `state`, of turn
    /// `turn`, under way since `turn_since` if it is. The replay follows,
    /// until [`Self::end_replay`]: none of its input requests shows before
    /// then.
    pub(super) fn attached(
        &mut self,
        inputs: Vec<InputRequest>,
        state: JobState,
        turn: Option<TurnId>,
        turn_since: Option<Instant>,
    ) {
        self.inputs.attached(inputs);
        self.replay_end = Some((state, turn));
        self.turn_since = turn_since;
    }

    /// The replay is over, ending where the attach said: whether a turn
    /// runs, then each request still open.
    pub(super) fn end_replay(&mut self) -> Vec<DaemonView> {
        let mut views: Vec<_> = self
            .replay_end
            .take()
            .map(|(state, turn)| {
                let rest = self.turn(state, turn.as_ref());
                // After the rest, which a `/compact` turn the TUI still
                // shows busy rests on as its compaction ended.
                let over = (!state.in_progress())
                    .then(|| self.compact_turn_over())
                    .flatten();
                let since = self
                    .turn_since
                    .take()
                    .filter(|_| state.in_progress())
                    .map(DaemonView::TurnSince);
                std::iter::once(rest).chain(over).chain(since)
            })
            .into_iter()
            .flatten()
            .collect();
        views.extend(self.inputs.end_replay());
        views
    }

    /// The TUI sent an answer to request `id`.
    pub(super) fn answering(&mut self, id: InputRequestId) {
        self.inputs.answering(id);
    }

    /// The daemon took no answer to request `id`, as `refusal` says.
    pub(super) fn answer_refused(
        &mut self,
        id: InputRequestId,
        refusal: AnswerRefusal,
    ) -> Vec<DaemonView> {
        self.inputs.refused(id, refusal)
    }

    /// What `event` changes.
    pub(super) fn map(&mut self, event: SessionEvent) -> Vec<DaemonView> {
        // Whatever a replay holds after a `/compact` turn's unshown rest
        // comes after that turn.
        let mut views: Vec<_> = (self.compact_turn == CompactTurn::RestedUnshown)
            .then(|| self.compact_turn_over())
            .flatten()
            .into_iter()
            .collect();
        views.extend(self.map_event(event));
        views
    }

    fn map_event(&mut self, event: SessionEvent) -> Vec<DaemonView> {
        match event {
            SessionEvent::UserMessage { content, steer, .. } => {
                self.end_messages();
                // A replay cut short of the turn's end still ends it here.
                let mut views: Vec<_> = self.compact_turn_over().into_iter().collect();
                views.extend(
                    steer
                        .as_ref()
                        .and_then(steering_id)
                        .map(DaemonView::SteerApplied),
                );
                let text = user_text(&content);
                if !text.is_empty() {
                    views.push(DaemonView::User(text));
                }
                views
            }
            SessionEvent::SteeringDelivered { id } => steering_id(&id)
                .map(DaemonView::SteerDelivered)
                .into_iter()
                .collect(),
            SessionEvent::AgentMessageChunk { message, text } => {
                vec![self.text(StreamKind::Assistant, message, text)]
            }
            SessionEvent::ThoughtChunk { message, text } => {
                vec![self.text(StreamKind::Reasoning, message, text)]
            }
            SessionEvent::ToolCall(update) => self.tool_call(*update).into_iter().collect(),
            SessionEvent::Plan { entries } => vec![DaemonView::Plan(todo_list(&entries))],
            SessionEvent::Usage {
                used_tokens,
                window_tokens,
                cost_usd_micros,
            } => vec![
                DaemonView::Context {
                    used_tokens,
                    window_tokens,
                },
                DaemonView::Spend(cost_usd_micros),
            ],
            SessionEvent::Compaction { status, .. } => {
                let views = compaction(status);
                if matches!(views[..], [DaemonView::CompactionStarted { command: true }]) {
                    self.compact_turn = CompactTurn::UnderWay;
                }
                views
            }
            SessionEvent::ResponseDiscarded => {
                // What the provider streamed stays; the retry's text starts
                // new messages below this notice.
                self.end_messages();
                vec![DaemonView::Notice(
                    "the provider restarted its response".into(),
                )]
            }
            // Replayed too: a reset into the turn under way shows the phase
            // its latest event set, as for replayed text.
            SessionEvent::ProviderRequest => vec![DaemonView::Requesting],
            // A replayed retry is history, not what the turn does now.
            SessionEvent::ProviderRetry { .. } if self.replay_end.is_some() => Vec::new(),
            SessionEvent::ProviderRetry {
                reason,
                retry_after_ms,
            } => vec![DaemonView::Retrying(retry_hint(reason, retry_after_ms))],
            SessionEvent::Card {
                title,
                plain,
                display,
                ..
            } => {
                // The model was given a result, so what it says next is a
                // later step's.
                self.open_message = None;
                self.stepped_since_reply = true;
                vec![match display {
                    Some(card) => DaemonView::Card(card),
                    None => DaemonView::Notice(card_text(title, &plain)),
                }]
            }
            SessionEvent::Notice { text } => vec![DaemonView::Notice(text)],
            SessionEvent::TurnEnded { turn, outcome } => {
                let views = self.turn_ended(turn, outcome);
                self.end_messages();
                views
            }
            // A replayed rename only sets the title, which the attach
            // already showed; a live one says so too.
            SessionEvent::TitleChanged { title } if self.inputs.replaying() => {
                vec![DaemonView::Title(title)]
            }
            SessionEvent::TitleChanged { title } => vec![
                DaemonView::Notice(format!("session titled: {title}")),
                DaemonView::Title(title),
            ],
            SessionEvent::State { state, turn } => {
                let mut views = match &self.replay_end {
                    // A reset's is history, unless it is of the turn the
                    // replay ends under way, which shows busy from where its
                    // rows start; the replay's end says the rest. A tail's is
                    // news, as live.
                    Some((end, under_way))
                        if self.reset
                            && !(end.in_progress()
                                && state.in_progress()
                                && turn == *under_way) =>
                    {
                        Vec::new()
                    }
                    Some(_) | None => vec![self.turn(state, turn.as_ref())],
                };
                if !state.in_progress() && self.compact_turn == CompactTurn::UnderWay {
                    if views.is_empty() {
                        self.compact_turn = CompactTurn::RestedUnshown;
                    } else {
                        // After the rest, which a `/compact` turn rests on
                        // as its compaction ended.
                        views.extend(self.compact_turn_over());
                    }
                }
                views
            }
            SessionEvent::InputRequested { request } => {
                self.inputs.requested(*request).into_iter().collect()
            }
            SessionEvent::InputResolved { id, resolution } => {
                self.inputs.resolved(id, resolution).into_iter().collect()
            }
            SessionEvent::InputHeld { id } => self.inputs.held(id).into_iter().collect(),
            SessionEvent::OptionsChanged { options } => {
                vec![DaemonView::Options(options)]
            }
            SessionEvent::CacheWarm {
                warming,
                cost_usd_micros,
            } => vec![DaemonView::CacheWarm {
                warming: warming.map(|progress| WarmProgress {
                    pings: progress.pings,
                    max_pings: progress.max_pings,
                }),
                cost_usd_micros: cost_usd_micros.unwrap_or_default(),
            }],
            // The process rail reads `rho/session/processes`, which also
            // carries each process's clock.
            SessionEvent::Background { .. } => Vec::new(),
            // `Unknown`, or an event a newer daemon sends.
            _ => Vec::new(),
        }
    }

    /// Ends the `/compact` turn under way, if any.
    fn compact_turn_over(&mut self) -> Option<DaemonView> {
        (std::mem::take(&mut self.compact_turn) != CompactTurn::None)
            .then_some(DaemonView::CompactTurnOver)
    }

    /// Whether a turn runs once the job is in `state`, of turn `turn`, as
    /// an event or the attach response says.
    fn turn(&mut self, state: JobState, turn: Option<&TurnId>) -> DaemonView {
        let turn = match state {
            JobState::Running | JobState::Queued => TurnState::Running,
            JobState::WaitingForInput => TurnState::WaitingForInput,
            JobState::Idle | JobState::Completed => TurnState::Resting(Rest::Ready),
            JobState::Cancelled | JobState::Interrupted => TurnState::Resting(Rest::Interrupted),
            JobState::Failed | JobState::TimedOut => TurnState::Resting(Rest::Failed {
                error: self
                    .failure
                    .take()
                    .filter(|(failed, _)| Some(failed) == turn)
                    .map_or_else(|| "the turn failed".into(), |(_, error)| error),
            }),
            // `Unknown`, or a state a newer daemon sends: shown at rest
            // rather than as a turn that never ends.
            _ => TurnState::Resting(Rest::Ready),
        };
        DaemonView::Turn(turn)
    }

    fn text(&mut self, kind: StreamKind, message: MessageId, text: String) -> DaemonView {
        let opens = self
            .open_message
            .as_ref()
            .is_none_or(|(open_kind, open)| *open_kind != kind || *open != message);
        if kind == StreamKind::Assistant {
            if opens {
                self.reply.clear();
                self.stepped_since_reply = false;
            }
            self.reply.push_str(&text);
        }
        self.open_message = Some((kind, message));
        DaemonView::Text {
            kind,
            text,
            opens,
            replayed: self.replay_end.is_some(),
        }
    }

    /// The next text, of any message, starts a new row.
    fn end_messages(&mut self) {
        self.open_message = None;
        self.reply.clear();
        self.stepped_since_reply = false;
    }

    fn tool_call(&mut self, update: ToolCallUpdate) -> Option<DaemonView> {
        // The step's text ends at its calls; the next text starts a row.
        self.open_message = None;
        self.stepped_since_reply = true;
        let ToolCallUpdate {
            id,
            status,
            display,
            ..
        } = update;
        let Ok(call) = ToolCallId::from_string(id.as_str()) else {
            tracing::debug!("skipping a daemon tool call without an id");
            return None;
        };
        let closed = match status {
            Some(ToolStatus::Completed) => Some(ToolCardStatus::Ok),
            // A call that closes `failed` without a display was cut off.
            Some(ToolStatus::Failed) => Some(ToolCardStatus::Interrupted),
            Some(ToolStatus::Pending | ToolStatus::InProgress) | None => None,
            // `Unknown`, or a status a newer daemon sends: still open.
            Some(_) => None,
        };
        match (closed, display) {
            (Some(_), Some(presentation)) => {
                self.running.remove(&id);
                Some(DaemonView::ToolFinished { call, presentation })
            }
            (Some(status), None) => {
                // Its last card, closed; a call never drawn has none.
                let mut card = self.running.remove(&id)?;
                card.status = status;
                Some(DaemonView::ToolFinished {
                    call,
                    presentation: card.into(),
                })
            }
            (None, Some(Presentation::Card(card) | Presentation::SummaryCard(card))) => {
                self.running.insert(id, card.clone());
                Some(DaemonView::ToolRunning { call, card })
            }
            // A running call carries a card; a notification is a final
            // presentation, shown once the call closes.
            (None, Some(Presentation::Notification(_))) => None,
            // Its title, input, or content changed, which its card already
            // shows.
            (None, None) => None,
        }
    }

    fn turn_ended(&mut self, turn: TurnId, outcome: TurnOutcome) -> Vec<DaemonView> {
        match outcome {
            TurnOutcome::Answered { text, .. } | TurnOutcome::MaxSteps { text } => {
                // An empty answer: the last step only called tools.
                if self.stepped_since_reply && !text.is_empty() && text != self.reply {
                    return vec![DaemonView::Text {
                        kind: StreamKind::Assistant,
                        text,
                        opens: true,
                        replayed: self.replay_end.is_some(),
                    }];
                }
                match final_answer_delta(&self.reply, &text) {
                    FinalAnswerDelta::None => Vec::new(),
                    // The answer finishes a reply whose last chunks went
                    // missing, or is the whole reply.
                    FinalAnswerDelta::Append(suffix) => vec![DaemonView::Text {
                        kind: StreamKind::Assistant,
                        text: suffix.to_owned(),
                        opens: self.reply.is_empty(),
                        replayed: self.replay_end.is_some(),
                    }],
                    // A retried reply arrives as its own message, which the
                    // transcript already shows after the discarded one.
                    FinalAnswerDelta::Mismatch => Vec::new(),
                }
            }
            // A replayed cancel says so too, though the TUI was never busy
            // with that turn.
            TurnOutcome::Cancelled => vec![DaemonView::Status("model interrupted".into())],
            TurnOutcome::TimedOut { timeout_ms } => {
                let error = format!(
                    "the turn timed out after {}",
                    humantime::format_duration(std::time::Duration::from_millis(timeout_ms))
                );
                self.failure = Some((turn, error.clone()));
                vec![DaemonView::Error(error)]
            }
            TurnOutcome::Failed { error } => {
                self.failure = Some((turn, error.message.clone()));
                vec![DaemonView::Error(error.message)]
            }
            // `Unknown`, or an outcome a newer daemon sends.
            _ => Vec::new(),
        }
    }
}

/// A steer's id as the TUI's pending panel keys it.
fn steering_id(id: &SteerId) -> Option<SteeringId> {
    let id = SteeringId::from_string(id.as_str());
    if id.is_err() {
        tracing::debug!("skipping a daemon steer without an id");
    }
    id.ok()
}

/// A user message's blocks as the transcript shows them, one per line.
fn user_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            ContentBlock::Image { mime_type, .. } => Some(format!("[image: {mime_type}]")),
            // As saved: the resource fenced in the text the model saw.
            ContentBlock::Resource { text, .. } => Some(text.clone()),
            ContentBlock::ResourceLink { name, uri, .. } => Some(format!("[{name}: {uri}]")),
            // `Unknown`, or a block a newer daemon sends.
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The plan as the TUI's checklist; `None` while it is empty.
fn todo_list(entries: &[PlanEntry]) -> Option<TodoList> {
    let todos: Vec<_> = entries
        .iter()
        .map(|entry| TodoItem {
            content: entry.content.clone(),
            status: match entry.status {
                PlanStatus::Pending => TodoStatus::Pending,
                PlanStatus::InProgress => TodoStatus::InProgress,
                PlanStatus::Completed => TodoStatus::Completed,
                // `Unknown`, or a status a newer daemon sends.
                _ => TodoStatus::Pending,
            },
        })
        .collect();
    (!todos.is_empty()).then_some(TodoList { todos })
}

/// The compaction card and status line for `status`.
fn compaction(status: CompactionStatus) -> Vec<DaemonView> {
    let outcome = match status {
        CompactionStatus::Started { trigger } => {
            return vec![DaemonView::CompactionStarted {
                command: trigger == Some(CompactionTrigger::Command),
            }]
        }
        CompactionStatus::Completed { counts } => {
            CompactionUiOutcome::from_facts(CompactionDisplayFacts::from_wire(counts))
        }
        CompactionStatus::Cancelled => CompactionUiOutcome::Cancelled,
        CompactionStatus::Failed { error } => CompactionUiOutcome::failed(error),
        // `Unknown`, or a status a newer daemon sends: every status but
        // `started` ends the compaction.
        _ => CompactionUiOutcome::Unchanged {
            detail: "compaction ended".into(),
        },
    };
    vec![DaemonView::CompactionEnded(outcome)]
}

/// The hint for a retry for `reason`, which reads as the SDK reason it came
/// from. `None` when the provider only sends a failed request again, which
/// shows without a hint.
fn retry_hint(reason: RetryReason, retry_after_ms: Option<u64>) -> Option<ProviderRetryHint> {
    let retry_after = retry_after_ms.map(Duration::from_millis);
    let reason = match reason {
        RetryReason::RateLimit => ProviderStreamResetReason::RetryableFailure {
            kind: ProviderErrorKind::RateLimit,
            retry_after,
        },
        RetryReason::ProviderError => ProviderStreamResetReason::RetryableFailure {
            kind: ProviderErrorKind::Other,
            retry_after,
        },
        RetryReason::InvalidResponse => ProviderStreamResetReason::InvalidResponse,
        RetryReason::ContextOverflow => ProviderStreamResetReason::ContextOverflow,
        RetryReason::RequestFailed => return None,
        // `Unknown`, or a reason a newer daemon sends: it retries, for a
        // reason this TUI cannot say.
        _ => return None,
    };
    Some(ProviderRetryHint { reason })
}

/// A card shown outside the conversation, as a notice: its title, then its
/// body.
fn card_text(title: String, plain: &str) -> String {
    let body = plain.trim();
    if body.is_empty() {
        title
    } else {
        format!("{title}\n{body}")
    }
}

#[cfg(test)]
#[path = "daemon_events_tests.rs"]
mod tests;
