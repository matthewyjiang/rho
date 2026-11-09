//! A daemon session's events as the transcript changes the TUI shows (#1667).
//!
//! [`DaemonEvents`] reads one session's [`SessionEvent`]s, replayed or live,
//! and says what each changes as [`DaemonView`]s, which `daemon_transcript`
//! applies through the TUI's own transcript paths: history entries for
//! messages, notices, and finished tool cards, and the live tool batch for
//! cards still running. The daemon draws each tool card with the presenter
//! the in-process TUI uses (`ToolCallUpdate::display`), so a card reads the
//! same either way.
//!
//! The job's state says whether a turn runs ([`TurnState`]), which drives
//! the TUI's busy chrome; steers carry their id, so the TUI's pending panel
//! drops a steer once the daemon applied it.
//!
//! Input requests open and close through [`OpenInputs`], which keeps those
//! still open, so the ones a replay leaves open show once it ends
//! ([`DaemonEvents::end_replay`]).
//!
//! Otherwise it holds only what the events leave implicit: which message the
//! last text chunk extended, the reply the last turn streamed, each open
//! call's last card, which a call cut off shows as interrupted, and why the
//! last turn failed, which its job state leaves out.

use std::collections::HashMap;

use rho_protocol::{
    display::{Presentation, ToolCard, ToolCardStatus},
    CompactionStatus, ContentBlock, InputRequest, InputRequestId, JobState, MessageId, PlanEntry,
    PlanStatus, Replay, SessionEvent, SteerId, ToolCallId as WireCallId, ToolCallUpdate,
    ToolStatus, TurnOutcome,
};
use rho_sdk::{SteeringId, ToolCallId};

use super::{
    compaction_display::{
        cancelled_card, compaction_call_id, failed_card, running_card, unchanged_card,
    },
    daemon_inputs::{AnswerRefusal, OpenInputs},
    transcript_events::final_answer_delta,
    FinalAnswerDelta, StreamKind,
};
use crate::tools::todo::{TodoItem, TodoList, TodoStatus};

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
    Text {
        kind: StreamKind,
        text: String,
        opens: bool,
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
    /// The session's checklist, replacing the last; `None` while it has none.
    Plan(Option<TodoList>),
    /// How full the model's context is.
    Context {
        used_tokens: u64,
        window_tokens: Option<u64>,
    },
    /// The daemon's cumulative session spend, independent of context tokens.
    Spend(Option<u64>),
    Title(String),
    Options(Box<rho_protocol::SessionOptions>),
    /// On-demand inventory response; ignored if the person closed the picker.
    McpInventory(Result<Box<rho_protocol::SessionMcpInventory>, String>),
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
    /// Status-line feedback.
    Status(String),
    Notice(String),
    Error(String),
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
    /// Whether the step that streamed `reply` went on to call tools. A
    /// turn's answer is its last agent message, so after calls an answer
    /// other than `reply` is a later step's whose chunks all went missing.
    called_since_reply: bool,
    /// Each open call's last running card.
    running: HashMap<WireCallId, ToolCard>,
    /// Why the last turn failed, which its `failed` job state shows.
    failure: Option<String>,
    inputs: OpenInputs,
}

impl DaemonEvents {
    /// A reset starts over; a tail preserves rows and open cards, but
    /// rebuilds input prompts from the attachment and its replay.
    pub(super) fn replay(&mut self, replay: Replay) -> DaemonView {
        match replay {
            Replay::Reset => {
                *self = Self::default();
                DaemonView::Clear
            }
            Replay::Tail => DaemonView::InputsReset,
        }
    }

    /// Starts an attachment whose response lists `inputs` as the requests
    /// open now. Its replay follows, until [`Self::end_replay`]: none of
    /// its input requests shows before then.
    pub(super) fn attached(&mut self, inputs: Vec<InputRequest>) {
        self.inputs.attached(inputs);
    }

    /// The replay is over, ending in the job's `state`, as the attach
    /// response said: whether a turn runs, then each request still open.
    pub(super) fn end_replay(&mut self, state: JobState) -> Vec<DaemonView> {
        let mut views = vec![self.turn(state)];
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
        match event {
            SessionEvent::UserMessage { content, steer, .. } => {
                self.end_messages();
                let mut views: Vec<_> = steer
                    .as_ref()
                    .and_then(steering_id)
                    .map(DaemonView::SteerApplied)
                    .into_iter()
                    .collect();
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
            SessionEvent::Compaction { status, .. } => vec![compaction(status)],
            SessionEvent::ResponseDiscarded => {
                // What the provider streamed stays; the retry's text starts
                // new messages below this notice.
                self.end_messages();
                vec![DaemonView::Notice(
                    "the provider restarted its response".into(),
                )]
            }
            SessionEvent::Card { title, plain, .. } => {
                vec![DaemonView::Notice(card_text(title, &plain))]
            }
            SessionEvent::Notice { text } => vec![DaemonView::Notice(text)],
            SessionEvent::TurnEnded { outcome } => {
                let views = self.turn_ended(outcome);
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
            SessionEvent::State { state } => vec![self.turn(state)],
            SessionEvent::InputRequested { request } => {
                self.inputs.requested(*request).into_iter().collect()
            }
            SessionEvent::InputResolved { id, resolution } => {
                self.inputs.resolved(id, resolution).into_iter().collect()
            }
            SessionEvent::InputHeld { id } => self.inputs.held(id).into_iter().collect(),
            SessionEvent::OptionsChanged { options } => {
                vec![DaemonView::Options(Box::new(options))]
            }
            // `Unknown`, or an event a newer daemon sends.
            _ => Vec::new(),
        }
    }

    /// Whether a turn runs once the job is in `state`, as an event or the
    /// attach response says.
    fn turn(&mut self, state: JobState) -> DaemonView {
        let turn = match state {
            JobState::Running | JobState::Queued => {
                self.failure = None;
                TurnState::Running
            }
            JobState::WaitingForInput => TurnState::WaitingForInput,
            JobState::Idle | JobState::Completed => TurnState::Resting(Rest::Ready),
            JobState::Cancelled | JobState::Interrupted => TurnState::Resting(Rest::Interrupted),
            JobState::Failed | JobState::TimedOut => TurnState::Resting(Rest::Failed {
                error: self
                    .failure
                    .take()
                    .unwrap_or_else(|| "the turn failed".into()),
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
                self.called_since_reply = false;
            }
            self.reply.push_str(&text);
        }
        self.open_message = Some((kind, message));
        DaemonView::Text { kind, text, opens }
    }

    /// The next text, of any message, starts a new row.
    fn end_messages(&mut self) {
        self.open_message = None;
        self.reply.clear();
        self.called_since_reply = false;
    }

    fn tool_call(&mut self, update: ToolCallUpdate) -> Option<DaemonView> {
        // The step's text ends at its calls; the next text starts a row.
        self.open_message = None;
        self.called_since_reply = true;
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

    fn turn_ended(&mut self, outcome: TurnOutcome) -> Vec<DaemonView> {
        match outcome {
            TurnOutcome::Answered { text, .. } | TurnOutcome::MaxSteps { text } => {
                // An empty answer: the last step only called tools.
                if self.called_since_reply && !text.is_empty() && text != self.reply {
                    return vec![DaemonView::Text {
                        kind: StreamKind::Assistant,
                        text,
                        opens: true,
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
                self.failure = Some(error.clone());
                vec![DaemonView::Error(error)]
            }
            TurnOutcome::Failed { error } => {
                self.failure = Some(error.message.clone());
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

/// The compaction card the in-process TUI shows for `status`. The daemon
/// reports no counts, so a completed one says only that it compacted; its
/// `/compact` reply says what it did.
fn compaction(status: CompactionStatus) -> DaemonView {
    let call = compaction_call_id();
    let card = match status {
        CompactionStatus::Started => {
            return DaemonView::ToolRunning {
                call,
                card: running_card(),
            }
        }
        CompactionStatus::Completed => unchanged_card("context compacted"),
        CompactionStatus::Cancelled => cancelled_card(),
        CompactionStatus::Failed { error } => failed_card(error),
        // `Unknown`, or a status a newer daemon sends: every status but
        // `started` ends the compaction.
        _ => unchanged_card("compaction ended"),
    };
    DaemonView::ToolFinished {
        call,
        presentation: card.into(),
    }
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
