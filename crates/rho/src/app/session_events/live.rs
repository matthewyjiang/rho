//! Maps a loaded session's live runtime events onto wire events.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use rho_protocol::{
    display::Presentation, CompactionCounts, CompactionId, CompactionStatus, CompactionTrigger,
    ContentBlock, Location, MessageId, PlanEntry, RetryReason, SessionEvent as WireEvent,
    ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
use rho_sdk::{
    model,
    tool::{ToolMetadata, ToolOutput},
    CompactionOutcome, ContextEstimate, ProviderErrorKind, ProviderStreamResetReason, RunEvent,
    SteeringId, ToolCompletion, UserInput,
};

use super::{
    cards, named_call, opened_call, plan_entries,
    replay::{saved_message_id, SavedTurn},
    result_content, status_update, steer_to_wire, text_content, tools, user_content, CallIds,
    UNNAMED_TOOL_TITLE,
};
use crate::{
    app::{
        interactive_presenter::{hosted_activity_card, InteractiveToolPresenter},
        session_runtime::{context_window, InsertedMessage, SessionEvent, SessionTools},
    },
    session::tree::ProjectedMessage,
    tools::{mcp::exported_name::McpToolNames, todo::TodoState},
};

/// Why a compaction failed when the run ended before it reported a result.
const UNFINISHED_COMPACTION: &str = "compaction ended without a result";

/// Turns one session's live runtime events into wire events. One per loaded
/// session; keeps the message ids and open tool calls it has announced.
///
/// Proposed tool arguments are held until [`RunEvent::ToolStarted`], so a
/// call starts with its title, kind, arguments and locations together. Every
/// call gets a full card, even one the run finishes without starting,
/// such as a call to a tool that does not exist. When the run's events end
/// ([`SessionEvent::RunClosed`]), calls still open close `failed`: the SDK
/// does not finish calls a cancel interrupts. A compaction reports as
/// events of its own and always ends: `cancelled` with a cancelled run,
/// else `failed` when the run ends first (#1670).
///
/// Agent message and thought chunks carry a message id. Consecutive chunks
/// of one kind share it; a chunk of the other kind, a new model step, or any
/// other event in between, such as a tool call, starts a new message. Only a
/// steer's delivery, which lands mid-response, does not.
///
/// A steer the daemon accepted ([`Self::steered`]) goes out as a user
/// message carrying its steer id once the run applies it
/// ([`RunEvent::SteeringApplied`]), and as a delivery while the provider
/// takes it mid-request ([`RunEvent::SteeringDelivered`]). A steer the
/// daemon did not record, such as a delegated child's parent's, shows no
/// user message; its saved row shows once replayed.
///
/// Each start, progress update and result also carries the card Rho's TUI
/// draws for the call, from the same presenter (`ToolCallUpdate::display`).
/// The presenter sees a proposal only once its call starts or reports, so a
/// discarded response's proposals never reach it. A call opens `pending`
/// while the provider still streams its arguments, with the card they draw
/// so far, and keeps its id once it starts; a discarded response closes the
/// calls it opened that way as failed.
///
/// Each time the run asks the provider again, a provider retry says why. A
/// tool the provider ran itself, such as its web search, shows as a call
/// that opens finished.
///
/// The session's checklist goes out as a plan whenever a finished tool
/// changed it. Each model step reports its context fill as usage, as do a
/// finished compaction and the run's end; the daemon adds the session's
/// cost, and asks for the fill again ([`Self::streamed_usage`]) to price a
/// response while it streams.
///
/// It also replays the session's saved rows ([`Self::replay`]), so a call
/// id a provider reuses is numbered the same way live and replayed, and the
/// turn in progress never reuses a wire id the saved transcript holds.
pub(crate) struct LiveEvents {
    /// The session's working directory; titles show paths relative to it.
    workspace: PathBuf,
    /// Identifies MCP calls, whose capped names cannot be decoded.
    mcp_tools: McpToolNames,
    /// The context window of the session's model as configured, which
    /// usage reports ahead of the one the provider reports.
    configured_window: Option<u64>,
    /// The window the latest usage reported, at first the configured one,
    /// which a compaction's usage reports again: its outcome carries no
    /// window.
    window: Option<u64>,
    /// The session's checklist, shared with its `todo` tool.
    todo: TodoState,
    /// The plan clients last got.
    sent_plan: Vec<PlanEntry>,
    /// The prompt the next [`SessionEvent::MessageInserted`] saved.
    prompt: Option<Vec<ContentBlock>>,
    proposed: HashMap<String, model::ToolCall>,
    /// Draws the run's calls as the TUI does, for `ToolCallUpdate::display`.
    presenter: InteractiveToolPresenter,
    /// The run's announced calls, by provider call id.
    announced: BTreeMap<String, AnnouncedCall>,
    /// The context fill last reported, which usage the provider counts
    /// within a response repeats.
    used_tokens: Option<u64>,
    /// The cards of the calls the step under way is still streaming, by
    /// the calls' place in the provider's output.
    previews: HashMap<usize, StreamedPreview>,
    /// Wire ids of the calls opened live.
    call_ids: CallIds,
    /// Wire ids of the calls [`Self::replay`] opened.
    replayed_ids: CallIds,
    /// How many of the session's saved display rows [`Self::replay`] has
    /// replayed.
    replayed_rows: usize,
    /// The compaction under way.
    compaction: Option<CompactionId>,
    /// The message the latest chunk belongs to, while later chunks of its
    /// kind still continue it.
    message: Option<OpenMessage>,
    /// Whether the provider attempt under way showed clients its output:
    /// text, or a hosted tool's card. A reset discards a response only when
    /// its attempt showed one.
    attempt_streamed: bool,
    /// The content of each steer the daemon recorded that has not applied,
    /// by the id the run accepted it under. The run's end clears it.
    steers: HashMap<SteeringId, Vec<ContentBlock>>,
    /// Steers that applied before the daemon recorded them: the run's
    /// receipt and its events reach the daemon apart. The run's end clears
    /// it, so steers the daemon never records do not pile up.
    applied_unseen: HashSet<SteeringId>,
    /// User messages of steers recorded after they applied, which the next
    /// [`Self::map`] sends first, so they keep the mapper's order.
    late_steers: Vec<WireEvent>,
}

/// How a compaction ended, as [`LiveEvents::compaction_ended`] reports it.
pub(crate) enum CompactionEnd<'a> {
    Completed(&'a CompactionOutcome),
    Cancelled,
    /// Failed with this error, which clients see: never a provider's raw
    /// text, which may echo a credential.
    Failed(String),
}

/// Which message stream a chunk belongs to.
#[derive(Clone, Copy, PartialEq)]
enum ChunkKind {
    Agent,
    Thought,
}

struct OpenMessage {
    kind: ChunkKind,
    id: MessageId,
}

/// Whether clients may still be waiting for a call's final status.
#[derive(Clone, Copy, PartialEq)]
enum CallState {
    /// Opened while the provider still streams its arguments; it has not
    /// started.
    Pending,
    Open,
    Finished,
}

/// A call clients were sent, under its wire id.
struct AnnouncedCall {
    id: ToolCallId,
    state: CallState,
}

impl LiveEvents {
    /// A mapper for the session in `workspace` whose tools are `tools`. A
    /// checklist the session already has counts as sent: a [`SavedTurn`]'s
    /// replay ends with it. When the session's tools are rebuilt, the checklist
    /// with them, follow them with [`Self::rebind_tools`]. Usage reports
    /// `configured_window`, the context window the session's model is
    /// configured with, when it has one (`session_runtime::context_window`).
    pub(crate) fn new(
        workspace: &Path,
        tools: SessionTools<'_>,
        configured_window: Option<u64>,
    ) -> Self {
        let todo = tools.todo_state();
        let mcp_tools = tools.mcp_report().tool_names();
        Self {
            workspace: workspace.to_owned(),
            presenter: InteractiveToolPresenter::new(workspace.to_owned(), mcp_tools.clone()),
            mcp_tools,
            configured_window,
            window: configured_window,
            sent_plan: plan_entries(&todo),
            todo,
            prompt: None,
            proposed: HashMap::new(),
            announced: BTreeMap::new(),
            previews: HashMap::new(),
            used_tokens: None,
            call_ids: CallIds::default(),
            replayed_ids: CallIds::default(),
            replayed_rows: 0,
            compaction: None,
            message: None,
            attempt_streamed: false,
            steers: HashMap::new(),
            applied_unseen: HashSet::new(),
            late_steers: Vec::new(),
        }
    }

    /// Notes `input`, steer `id`, which the run accepted, so its
    /// [`RunEvent::SteeringApplied`] shows it as a user message. When that
    /// already arrived, the next [`Self::map`] sends the message first.
    pub(crate) fn steered(&mut self, id: SteeringId, input: &UserInput) {
        let content = user_content(input.blocks());
        if self.applied_unseen.remove(&id) {
            self.late_steers.push(steer_message(&id, content));
        } else {
            self.steers.insert(id, content);
        }
    }

    /// Forgets steer `id`, which was withdrawn before it applied.
    pub(crate) fn withdrawn(&mut self, id: &SteeringId) {
        self.steers.remove(id);
    }

    /// Notes `input`, the display input of the turn about to start
    /// (`Turn::display`). Runtime events name the user message a turn saved
    /// but do not carry it, so its [`SessionEvent::MessageInserted`] maps to
    /// this input.
    pub(crate) fn prompt(&mut self, input: &UserInput) {
        self.prompt = Some(user_content(input.blocks()));
        self.call_ids.cover(&self.replayed_ids);
    }

    /// Maps with `tools` from now on, as a session that `/agent:` rebound to
    /// an agent with tools of its own does, against `configured_window`, the
    /// agent model's: as a new mapper in the same workspace, except that
    /// the call ids it gave and the rows it replayed stay counted, so no
    /// call id is reused and nothing replays twice. Call between turns.
    pub(crate) fn rebind_tools(&mut self, tools: SessionTools<'_>, configured_window: Option<u64>) {
        let rebound = Self::new(&self.workspace, tools, configured_window);
        let Self {
            call_ids,
            replayed_ids,
            replayed_rows,
            ..
        } = std::mem::replace(self, rebound);
        self.call_ids = call_ids;
        self.replayed_ids = replayed_ids;
        self.replayed_rows = replayed_rows;
    }

    /// `display`, the prompt of a host command that runs no model turn, such
    /// as `/workflow:` or `/compact`, shown as the user message `inserted`
    /// names.
    pub(crate) fn shown_prompt(
        &mut self,
        display: &UserInput,
        inserted: &InsertedMessage,
    ) -> WireEvent {
        self.message = None;
        WireEvent::UserMessage {
            message: inserted_message_id(inserted),
            content: user_content(display.blocks()),
            steer: None,
        }
    }

    /// `text`, a host command's reply, shown as one agent message under the
    /// id of the row `reply` names, which no chunk continues.
    pub(crate) fn shown_reply(&mut self, reply: &InsertedMessage, text: String) -> WireEvent {
        self.message = None;
        WireEvent::AgentMessageChunk {
            message: inserted_message_id(reply),
            text,
        }
    }

    /// The rows of `rows`, the session's saved display rows, that this
    /// mapper has not replayed yet, as a turn of the saved transcript (see
    /// [`SavedTurn`]); `None` when no row is new, so nothing saved since the
    /// last replay.
    pub(crate) fn replay(
        &mut self,
        rows: &[ProjectedMessage],
        workspace: &Path,
        tools: SessionTools<'_>,
    ) -> Option<SavedTurn> {
        let from = std::mem::replace(&mut self.replayed_rows, rows.len());
        match rows.get(from..) {
            Some([]) => None,
            Some(new) => Some(SavedTurn::capture(
                new,
                workspace,
                tools,
                &mut self.replayed_ids,
            )),
            // Display rows only grow along a daemon session's branch.
            None => {
                tracing::warn!(
                    from,
                    rows = rows.len(),
                    "a session's saved rows shrank; its replay keeps the rows it had"
                );
                None
            }
        }
    }

    /// Forgets which saved rows were replayed, so the next [`Self::replay`]
    /// replays every row: the session's conversation moved to another
    /// branch of its tree, whose rows are not the ones replayed so far.
    pub(crate) fn forget_replayed(&mut self) {
        self.replayed_rows = 0;
        self.replayed_ids = Default::default();
        // Numbered again from the rows replayed next, as a fresh mapper's.
        self.call_ids = Default::default();
    }

    /// The usage event for `estimate`, the session's context between
    /// turns, with no cost yet.
    pub(crate) fn context_usage(&mut self, estimate: ContextEstimate) -> WireEvent {
        self.usage(estimate)
    }

    /// The wire events for `event`, in order; empty for events the wire does
    /// not carry yet.
    pub(crate) fn map(&mut self, event: &SessionEvent) -> Vec<WireEvent> {
        let late_steers = std::mem::take(&mut self.late_steers);
        if !late_steers.is_empty() {
            // A user message ends any message.
            self.message = None;
        }
        // Text after a call's arguments says they stopped growing, so a
        // card held for more of them goes out before the text. Left to the
        // host's pause, it would land between two chunks of one message
        // and split the row clients show it in.
        let held = match event {
            SessionEvent::Run(
                RunEvent::AssistantTextDelta { .. }
                | RunEvent::ReasoningDelta { .. }
                | RunEvent::ReasoningSummaryDelta { .. },
            ) => self.held_previews(),
            _ => Vec::new(),
        };
        let events = match event {
            SessionEvent::MessageInserted(inserted) => {
                self.user_message(inserted).into_iter().collect()
            }
            SessionEvent::Run(event) => self.run_event(event),
            SessionEvent::BoundaryApplied { transcript } => cards(transcript).collect(),
            SessionEvent::Notice { text } => vec![WireEvent::Notice { text: text.clone() }],
            SessionEvent::RunClosed => self.close_unfinished(),
            // The committed turn is the settled context; in-run updates
            // were estimates.
            SessionEvent::RunFinished { context } => vec![self.usage(*context)],
            // The daemon reports the job's state itself: a turn's start, its
            // waits for input, and its end.
            SessionEvent::RunStarted => Vec::new(),
            // The host opens these as input requests with ids of its own
            // (the daemon's `sessions::inputs`, #1660).
            SessionEvent::ApprovalRequested { .. }
            | SessionEvent::ApprovalWithdrawn { .. }
            | SessionEvent::PlanApprovalRequested { .. }
            | SessionEvent::PlanApprovalWithdrawn { .. }
            | SessionEvent::QuestionAsked { .. }
            | SessionEvent::QuestionWithdrawn { .. } => Vec::new(),
        };
        if events.iter().any(|event| {
            !matches!(
                event,
                WireEvent::AgentMessageChunk { .. }
                    | WireEvent::ThoughtChunk { .. }
                    | WireEvent::SteeringDelivered { .. }
            )
        }) {
            self.message = None;
        }
        late_steers.into_iter().chain(held).chain(events).collect()
    }

    /// The prompt as the user message the turn saved. Nothing when no
    /// prompt was noted, since the event does not carry the message.
    fn user_message(&mut self, inserted: &InsertedMessage) -> Option<WireEvent> {
        let content = self.prompt.take()?;
        Some(WireEvent::UserMessage {
            message: inserted_message_id(inserted),
            content,
            steer: None,
        })
    }

    /// The user messages of the steers `ids` names, in order. One the
    /// daemon has not recorded yet waits for [`Self::steered`].
    fn steering_applied(&mut self, ids: &[SteeringId]) -> Vec<WireEvent> {
        ids.iter()
            .filter_map(|id| match self.steers.remove(id) {
                Some(content) => Some(steer_message(id, content)),
                None => {
                    self.applied_unseen.insert(id.clone());
                    None
                }
            })
            .collect()
    }

    fn run_event(&mut self, event: &RunEvent) -> Vec<WireEvent> {
        match event {
            RunEvent::AssistantTextDelta { text } => vec![self.chunk(ChunkKind::Agent, text)],
            RunEvent::ReasoningDelta { text } | RunEvent::ReasoningSummaryDelta { text } => {
                vec![self.chunk(ChunkKind::Thought, text)]
            }
            RunEvent::StepStarted {
                context_estimate, ..
            } => {
                self.message = None;
                self.attempt_streamed = false;
                // A step numbers its calls from zero again, and a call the
                // last one streamed has started by now if it ever will.
                let unstarted = self.fail_pending();
                self.presenter.step_started();
                unstarted
                    .into_iter()
                    .chain([WireEvent::ProviderRequest, self.usage(*context_estimate)])
                    .collect()
            }
            RunEvent::ToolProposed { call } => {
                if !call.id.is_empty() {
                    self.proposed.insert(call.id.clone(), call.clone());
                }
                Vec::new()
            }
            RunEvent::ToolStarted {
                call_id,
                name,
                metadata,
                execution: _,
            } => {
                self.present_proposal(call_id.as_str());
                let card = self
                    .presenter
                    .started(call_id.clone(), name.clone(), metadata.clone())
                    .card;
                let arguments = self
                    .proposed
                    .remove(call_id.as_str())
                    .map(|call| call.arguments);
                // A call its streamed arguments already opened keeps its id.
                let (id, superseded) = match self.announced.get_mut(call_id.as_str()) {
                    Some(call) if call.state == CallState::Pending => {
                        call.state = CallState::Open;
                        (call.id.clone(), None)
                    }
                    _ => self.announce(call_id.as_str(), CallState::Open),
                };
                let opened = ToolCallUpdate {
                    display: Some(Presentation::Card(card)),
                    ..opened_call(
                        id,
                        name,
                        arguments,
                        metadata,
                        &self.workspace,
                        &self.mcp_tools,
                    )
                };
                superseded
                    .into_iter()
                    .chain([WireEvent::tool_call(opened)])
                    .collect()
            }
            RunEvent::ToolUpdated { call_id, progress } => {
                self.present_proposal(call_id.as_str());
                let card = self.presenter.updated(call_id, progress).card;
                let shown = CallUpdate {
                    status: ToolStatus::InProgress,
                    content: vec![text_content(progress.message())],
                    locations: None,
                    display: Presentation::Card(card),
                };
                self.tool_update(call_id, CallState::Open, shown)
                    .into_iter()
                    .collect()
            }
            RunEvent::ToolFinished { call_id, result } => {
                self.present_proposal(call_id.as_str());
                let (_ok, shown) = self.presenter.finished(call_id, result);
                let finished = finished_update(result, &self.workspace, shown.presentation);
                let finished = self.tool_update(call_id, CallState::Finished, finished);
                // The call may have replaced the checklist, whether it ran
                // directly or inside code mode.
                finished.into_iter().chain(self.plan_change()).collect()
            }
            RunEvent::ProviderStreamReset { reason, .. } => {
                // The retried response proposes its calls again, and streams
                // them again: the cards its streamed arguments opened close.
                self.proposed.clear();
                self.presenter.step_started();
                let discarded = self.fail_pending();
                // An attempt that failed before streaming, as one that could
                // not connect, has no response to discard: only the retry.
                let streamed = std::mem::take(&mut self.attempt_streamed) || !discarded.is_empty();
                discarded
                    .into_iter()
                    .chain(streamed.then_some(WireEvent::ResponseDiscarded))
                    .chain([provider_retry(reason)])
                    .collect()
            }
            RunEvent::ProviderRequestRetry => vec![WireEvent::ProviderRetry {
                reason: RetryReason::RequestFailed,
                retry_after_ms: None,
            }],
            RunEvent::WebSearch { detail } => {
                self.attempt_streamed = true;
                vec![hosted_activity("web_search", detail)]
            }
            RunEvent::HostedToolActivity { name, detail } => {
                self.attempt_streamed = true;
                vec![hosted_activity(name, detail)]
            }
            RunEvent::ToolCallUpdated {
                index,
                id,
                name,
                arguments_delta,
            } => self.streamed_call(*index, id.as_deref(), name.as_deref(), arguments_delta),
            RunEvent::CompactionStarted { .. } => {
                self.compaction_started(CompactionTrigger::Automatic)
            }
            RunEvent::CompactionCompleted { outcome, .. } => {
                self.compaction_ended(CompactionEnd::Completed(outcome))
            }
            // A cancel ends the compaction under way as cancelled; the
            // calls it interrupted close when the run's events end.
            RunEvent::Cancelled { .. } => self.compaction_ended(CompactionEnd::Cancelled),
            // [`SessionEvent::RunClosed`] closes what the run left open.
            RunEvent::Started { .. } | RunEvent::Completed { .. } | RunEvent::Failed { .. } => {
                Vec::new()
            }
            // [`SessionEvent::BoundaryApplied`] carried the rows it shows.
            RunEvent::BoundaryInputApplied { .. } => Vec::new(),
            RunEvent::SteeringApplied { ids } => self.steering_applied(ids),
            RunEvent::SteeringDelivered { id } => vec![WireEvent::SteeringDelivered {
                id: steer_to_wire(id),
            }],
            // Questions arrive as [`SessionEvent::QuestionAsked`].
            RunEvent::HostInputRequested { .. } | RunEvent::ToolHostInputRequested { .. } => {
                Vec::new()
            }
            // The daemon tells the failed turn's clients the diagnostic
            // itself (`sessions::turn`), unless the turn failed to
            // authenticate: it may echo a credential, and this event does
            // not say which failure it is from.
            RunEvent::ProviderDiagnostic { .. } => Vec::new(),
            // Model-call metrics land as a later minor.
            // The daemon reports usage counted mid-response itself
            // ([`Self::streamed_usage`]).
            RunEvent::UsageUpdated { .. }
            | RunEvent::ProviderContextUpdated { .. }
            | RunEvent::ModelCallCompleted { .. }
            | RunEvent::ProviderServiceTierFallback { .. } => Vec::new(),
            // RunEvent is non_exhaustive; a newer SDK's events wait for the
            // wire to carry them.
            _ => Vec::new(),
        }
    }

    /// Context fill from `estimate`, against the configured window, else
    /// the one the provider reported with it. The daemon prices it.
    fn usage(&mut self, estimate: ContextEstimate) -> WireEvent {
        self.window = context_window(self.configured_window, estimate);
        self.fill(estimate.tokens())
    }

    /// The context fill last reported, again, for a host that prices the
    /// response under way when the provider counts more of its usage
    /// ([`RunEvent::UsageUpdated`]). It arrives between a message's chunks
    /// and ends nothing. `None` before any fill was reported.
    pub(crate) fn streamed_usage(&mut self) -> Option<WireEvent> {
        self.used_tokens.map(|used| self.fill(used))
    }

    /// A usage event for a context holding `used_tokens`, which
    /// [`Self::streamed_usage`] repeats.
    fn fill(&mut self, used_tokens: u64) -> WireEvent {
        self.used_tokens = Some(used_tokens);
        WireEvent::Usage {
            used_tokens,
            window_tokens: self.window,
            cost_usd_micros: None,
        }
    }

    /// A chunk of the open message of `kind`, or of a new message when the
    /// previous event was anything else.
    fn chunk(&mut self, kind: ChunkKind, text: &str) -> WireEvent {
        self.attempt_streamed = true;
        let message = match &self.message {
            Some(open) if open.kind == kind => open.id.clone(),
            _ => {
                let id = fresh_message_id();
                self.message = Some(OpenMessage {
                    kind,
                    id: id.clone(),
                });
                id
            }
        };
        let text = text.to_owned();
        match kind {
            ChunkKind::Agent => WireEvent::AgentMessageChunk { message, text },
            ChunkKind::Thought => WireEvent::ThoughtChunk { message, text },
        }
    }

    /// Closing events for the compaction and the calls clients saw open but
    /// never saw finish: each closes `failed`. Ends the run's state, so the
    /// next run starts clean: steers it has not applied were dropped.
    fn close_unfinished(&mut self) -> Vec<WireEvent> {
        self.proposed.clear();
        self.previews.clear();
        self.presenter =
            InteractiveToolPresenter::new(self.workspace.clone(), self.mcp_tools.clone());
        self.steers.clear();
        self.applied_unseen.clear();
        // Never the run's own error: it is not redacted, and may echo a
        // credential; the turn's end carries the redacted outcome (#1670).
        let compaction =
            self.compaction_ended(CompactionEnd::Failed(UNFINISHED_COMPACTION.to_owned()));
        let announced = std::mem::take(&mut self.announced);
        let calls = announced
            .into_values()
            .filter(|call| call.state != CallState::Finished)
            .map(|call| WireEvent::tool_call(status_update(call.id, Some(ToolStatus::Failed))));
        compaction.into_iter().chain(calls).collect()
    }

    /// A compaction starting: the run's automatic one, or one a host runs
    /// between turns, such as `/compact`, which emits no run events.
    pub(crate) fn compaction_started(&mut self, trigger: CompactionTrigger) -> Vec<WireEvent> {
        let id = fresh_compaction_id();
        self.compaction = Some(id.clone());
        // Called outside [`Self::map`] too: a compaction ends any message.
        self.message = None;
        vec![WireEvent::Compaction {
            id,
            status: CompactionStatus::Started {
                trigger: Some(trigger),
            },
        }]
    }

    /// The compaction under way ending as `end`, under the id it started
    /// with; a cancel or failure with no compaction under way reports
    /// nothing.
    pub(crate) fn compaction_ended(&mut self, end: CompactionEnd<'_>) -> Vec<WireEvent> {
        let (id, status, usage) = match end {
            CompactionEnd::Completed(outcome) => (
                // A completion whose start was not seen still gets an id.
                Some(self.compaction.take().unwrap_or_else(fresh_compaction_id)),
                CompactionStatus::Completed {
                    counts: CompactionCounts {
                        previous_messages: outcome.previous_messages() as u64,
                        current_messages: outcome.current_messages() as u64,
                        previous_tokens: outcome.previous_tokens(),
                        current_tokens: outcome.current_tokens(),
                        cost_usd_micros: outcome.cost_usd_micros(),
                    },
                },
                // The compacted context's fill, as ACP reports it.
                Some(outcome.current_tokens()),
            ),
            CompactionEnd::Cancelled => (self.compaction.take(), CompactionStatus::Cancelled, None),
            CompactionEnd::Failed(error) => (
                self.compaction.take(),
                CompactionStatus::Failed { error },
                None,
            ),
        };
        let Some(id) = id else {
            return Vec::new();
        };
        // Called outside [`Self::map`] too: a compaction ends any message.
        self.message = None;
        std::iter::once(WireEvent::Compaction { id, status })
            .chain(usage.map(|used| self.fill(used)))
            .collect()
    }

    /// A progress or result update for a call, sent as a full card when
    /// clients have not seen the call yet. Progress that arrives after a
    /// call finished is dropped, so it cannot reopen the call.
    fn tool_update(
        &mut self,
        call_id: &rho_sdk::ToolCallId,
        next: CallState,
        shown: CallUpdate,
    ) -> Option<WireEvent> {
        let id = call_id.as_str();
        let update = move |call: ToolCallUpdate| ToolCallUpdate {
            status: Some(shown.status),
            content: Some(shown.content),
            locations: shown.locations,
            display: Some(shown.display),
            ..call
        };
        let (wire_id, streamed) = match self.announced.get_mut(id) {
            // Opened by its streamed arguments and never started: this is
            // the first clients hear of the call as proposed.
            Some(call) if call.state == CallState::Pending => {
                call.state = next;
                (call.id.clone(), true)
            }
            Some(call) => {
                if call.state == CallState::Finished && next == CallState::Open {
                    return None;
                }
                call.state = next;
                return Some(WireEvent::tool_call(update(status_update(
                    call.id.clone(),
                    /*status*/ None,
                ))));
            }
            None => (self.announce(id, next).0, false),
        };
        let card = match self.proposed.remove(id) {
            // Titled as replay titles a call whose tool reports nothing.
            Some(call) => named_call(
                wire_id,
                &call.name,
                Some(call.arguments),
                &ToolMetadata::default(),
                &self.workspace,
                &self.mcp_tools,
            ),
            // Its streamed open already named it.
            None if streamed => status_update(wire_id, /*status*/ None),
            None => ToolCallUpdate {
                title: Some(UNNAMED_TOOL_TITLE.to_owned()),
                kind: Some(ToolKind::Other),
                ..status_update(wire_id, /*status*/ None)
            },
        };
        Some(WireEvent::tool_call(update(card)))
    }

    /// Closes as failed the calls streamed arguments opened that never
    /// started, and forgets the step's streamed previews.
    fn fail_pending(&mut self) -> Vec<WireEvent> {
        self.previews.clear();
        self.announced
            .values_mut()
            .filter(|call| call.state == CallState::Pending)
            .map(|call| {
                call.state = CallState::Finished;
                WireEvent::tool_call(status_update(call.id.clone(), Some(ToolStatus::Failed)))
            })
            .collect()
    }

    /// Opens a call under provider id `provider`: its wire id, and the
    /// event closing a call still open under that id, which it supersedes,
    /// as replay closes it.
    fn announce(&mut self, provider: &str, state: CallState) -> (ToolCallId, Option<WireEvent>) {
        let id = self.call_ids.open(provider);
        let call = AnnouncedCall {
            id: id.clone(),
            state,
        };
        let superseded = self
            .announced
            .insert(provider.to_owned(), call)
            .filter(|call| call.state != CallState::Finished)
            .map(|call| WireEvent::tool_call(status_update(call.id, Some(ToolStatus::Failed))));
        (id, superseded)
    }

    /// A plan when the checklist changed since clients last got one.
    fn plan_change(&mut self) -> Option<WireEvent> {
        let entries = plan_entries(&self.todo);
        if entries == self.sent_plan {
            return None;
        }
        self.sent_plan.clone_from(&entries);
        Some(WireEvent::Plan { entries })
    }

    /// Hands the presenter the proposal held for `provider`, as the TUI's
    /// presenter sees it, so the call's card shows its arguments.
    fn present_proposal(&mut self, provider: &str) {
        if let Some(call) = self.proposed.get(provider) {
            self.presenter.proposed(call.clone());
        }
    }
}

/// What a progress or result update shows of a call.
struct CallUpdate {
    status: ToolStatus,
    content: Vec<ToolContent>,
    /// Replace the call's earlier locations when given.
    locations: Option<Vec<Location>>,
    /// The call's card as the TUI draws it at this point.
    display: Presentation,
}

/// A finished call's status and content, the files its output points at,
/// and `display`, its final presentation.
fn finished_update(result: &ToolCompletion, cwd: &Path, display: Presentation) -> CallUpdate {
    let finished = |status, content| CallUpdate {
        status,
        content,
        locations: result
            .output()
            .and_then(|output| tools::finished_locations(output, cwd)),
        display,
    };
    match result {
        ToolCompletion::Completed(output) => {
            let status = if output.is_failure() {
                ToolStatus::Failed
            } else {
                ToolStatus::Completed
            };
            finished(status, output_content(output, cwd))
        }
        ToolCompletion::Failure(failure) => {
            finished(ToolStatus::Failed, vec![text_content(failure.message())])
        }
        // The model called a tool that does not exist; say so on the card.
        ToolCompletion::Unavailable => finished(
            ToolStatus::Failed,
            vec![text_content("tool is unavailable")],
        ),
        // ToolCompletion is non_exhaustive; unknown results cannot be shown
        // as success.
        _ => finished(ToolStatus::Failed, Vec::new()),
    }
}

/// A tool's output as a call's content: what the model got, as replay shows
/// it, then the files it changed. A change is a diff of whole texts; output
/// that only carries a patch shows the patch as text.
fn output_content(output: &ToolOutput, cwd: &Path) -> Vec<ToolContent> {
    let mut content = result_content(output.content(), output.images());
    let metadata = output.presentation();
    if metadata.file_changes().is_empty() {
        content.extend(metadata.unified_diff().map(text_content));
    } else {
        content.extend(
            metadata
                .file_changes()
                .iter()
                .map(|change| ToolContent::Diff {
                    path: cwd.join(change.path()),
                    old_text: change.old_text().map(str::to_owned),
                    new_text: change.new_text().to_owned(),
                    deleted: change.is_deleted(),
                }),
        );
    }
    content
}

/// The retry that follows a stream reset for `reason`.
fn provider_retry(reason: &ProviderStreamResetReason) -> WireEvent {
    let (reason, retry_after) = match reason {
        ProviderStreamResetReason::RetryableFailure {
            kind: ProviderErrorKind::RateLimit,
            retry_after,
        } => (RetryReason::RateLimit, *retry_after),
        ProviderStreamResetReason::RetryableFailure { retry_after, .. } => {
            (RetryReason::ProviderError, *retry_after)
        }
        ProviderStreamResetReason::InvalidResponse => (RetryReason::InvalidResponse, None),
        ProviderStreamResetReason::ContextOverflow => (RetryReason::ContextOverflow, None),
        // `ProviderStreamResetReason` is non-exhaustive. A reason a newer
        // SDK adds is still the provider failing a request, and a client
        // that shows it so says no more than it knows.
        _ => (RetryReason::ProviderError, None),
    };
    WireEvent::ProviderRetry {
        reason,
        retry_after_ms: retry_after.map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
    }
}

/// Hosted tool `name`'s activity, which the provider ran itself, as a call
/// that opens finished with `detail` and the card the TUI draws for it. Its
/// id is fresh: no provider call or saved row has one like it, and the run
/// never updates it.
fn hosted_activity(name: &str, detail: &str) -> WireEvent {
    let kind = match name {
        "web_search" | "x_search" => ToolKind::Search,
        _ => ToolKind::Other,
    };
    let id = ToolCallId::new(format!("hosted-{}", uuid::Uuid::new_v4()));
    let text = detail.trim();
    WireEvent::tool_call(ToolCallUpdate {
        title: Some(name.to_owned()),
        kind: Some(kind),
        status: Some(ToolStatus::Completed),
        content: (!text.is_empty()).then(|| vec![text_content(text)]),
        display: Some(Presentation::Card(hosted_activity_card(name, detail))),
        ..status_update(id, /*status*/ None)
    })
}

/// The wire id of the message `inserted` names: the one a replay gives its
/// row, or a fresh one for a message no replay shows.
fn inserted_message_id(inserted: &InsertedMessage) -> MessageId {
    match inserted {
        InsertedMessage::Saved(origin) => saved_message_id(origin),
        InsertedMessage::Unreplayed => fresh_message_id(),
    }
}

/// Applied steer `id`, `content`, as a user message.
fn steer_message(id: &SteeringId, content: Vec<ContentBlock>) -> WireEvent {
    WireEvent::UserMessage {
        message: fresh_message_id(),
        content,
        steer: Some(steer_to_wire(id)),
    }
}

/// A live message's id. Replayed messages derive theirs from saved rows.
fn fresh_message_id() -> MessageId {
    MessageId::new(format!("msg-{}", uuid::Uuid::new_v4()))
}

/// Each compaction gets a fresh random id: clients may keep a session's
/// events across daemon restarts, and an id that restarts per process
/// would update an earlier card.
fn fresh_compaction_id() -> CompactionId {
    CompactionId::new(format!("compaction-{}", uuid::Uuid::new_v4()))
}

#[path = "live_previews.rs"]
mod previews;
use previews::StreamedPreview;

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
