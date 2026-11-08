//! Maps a loaded session's live runtime events onto wire events.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use rho_protocol::{
    CompactionId, CompactionStatus, ContentBlock, Location, MessageId, PlanEntry,
    SessionEvent as WireEvent, ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
use rho_sdk::{
    model,
    tool::{ToolMetadata, ToolOutput},
    ContextEstimate, RunEvent, ToolCompletion, UserInput,
};
use serde_json::Value;

use super::{
    cards, named_call, opened_call, plan_entries,
    replay::{self, saved_message_id},
    result_content, status_update, text_content, tools, user_content, CallIds, UNNAMED_TOOL_TITLE,
};
use crate::{
    app::session_runtime::{InsertedMessage, RunEnd, SessionEvent, SessionTools},
    session::tree::ProjectedMessage,
    tools::{mcp::exported_name::McpToolNames, todo::TodoState},
};

/// Why a compaction failed when the run ended before it reported a result.
const UNFINISHED_COMPACTION: &str = "compaction ended without a result";

/// Turns one session's live runtime events into wire events. One per loaded
/// session; keeps the message ids and open tool calls it has announced.
///
/// Proposed tool arguments are held until [`RunEvent::ToolStarted`], so a
/// call opens with its title, kind, arguments and locations together. Every
/// call opens with a full card, even one the run finishes without starting,
/// such as a call to a tool that does not exist. When the run's events end
/// ([`SessionEvent::RunClosed`]), calls still open close `failed`: the SDK
/// does not finish calls a cancel interrupts. A compaction reports as
/// events of its own and always ends: `cancelled` with a cancelled run,
/// else `failed` when the run ends first (#1670).
///
/// Agent message and thought chunks carry a message id. Consecutive chunks
/// of one kind share it; a chunk of the other kind, a new model step, or any
/// other event in between, such as a tool call, starts a new message.
///
/// The session's checklist goes out as a plan whenever a finished tool
/// changed it. Each model step reports its context fill as usage, and the
/// run's end its settled fill; the daemon adds the session's cost
/// (`session_events::priced`).
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
    /// The session's checklist, shared with its `todo` tool.
    todo: TodoState,
    /// The plan clients last got.
    sent_plan: Vec<PlanEntry>,
    /// The prompt the next [`SessionEvent::MessageInserted`] saved.
    prompt: Option<Vec<ContentBlock>>,
    proposed: HashMap<String, model::ToolCall>,
    /// The run's announced calls, by provider call id.
    announced: BTreeMap<String, AnnouncedCall>,
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
    /// checklist the session already has counts as sent: [`super::replay`]
    /// ends with it. Build a new one when the session's tools are rebuilt,
    /// since the checklist lives with them.
    pub(crate) fn new(workspace: &Path, tools: SessionTools<'_>) -> Self {
        let todo = tools.todo_state();
        Self {
            workspace: workspace.to_owned(),
            mcp_tools: tools.mcp_report().tool_names(),
            configured_window: None,
            sent_plan: plan_entries(&todo),
            todo,
            prompt: None,
            proposed: HashMap::new(),
            announced: BTreeMap::new(),
            call_ids: CallIds::default(),
            replayed_ids: CallIds::default(),
            replayed_rows: 0,
            compaction: None,
            message: None,
        }
    }

    /// Reports usage against `window`, the context window the session's
    /// model is configured with, when it has one, rather than the one the
    /// provider reports, as the TUI context meter measures it.
    pub(crate) fn with_configured_window(mut self, window: Option<u64>) -> Self {
        self.configured_window = window;
        self
    }

    /// Notes `input`, the display input of the turn about to start
    /// (`Turn::display`). Runtime events name the user message a turn saved
    /// but do not carry it, so its [`SessionEvent::MessageInserted`] maps to
    /// this input.
    pub(crate) fn prompt(&mut self, input: &UserInput) {
        self.prompt = Some(user_content(input.blocks()));
        self.call_ids.cover(&self.replayed_ids);
    }

    /// The wire events that replay the rows of `rows`, the session's saved
    /// display rows, that this mapper has not replayed yet (see
    /// [`replay::replay`]); `None` when no row is new, so nothing saved since
    /// the last replay.
    pub(crate) fn replay(
        &mut self,
        rows: &[ProjectedMessage],
        workspace: &Path,
        tools: SessionTools<'_>,
    ) -> Option<Vec<WireEvent>> {
        let from = std::mem::replace(&mut self.replayed_rows, rows.len());
        match rows.get(from..) {
            Some([]) => None,
            Some(new) => Some(replay::replay(
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

    /// The wire events for `event`, in order; empty for events the wire does
    /// not carry yet.
    pub(crate) fn map(&mut self, event: &SessionEvent) -> Vec<WireEvent> {
        let events = match event {
            SessionEvent::MessageInserted(inserted) => {
                self.user_message(inserted).into_iter().collect()
            }
            SessionEvent::Run(event) => self.run_event(event),
            SessionEvent::BoundaryApplied { transcript } => cards(transcript).collect(),
            SessionEvent::Notice { text } => vec![WireEvent::Notice { text: text.clone() }],
            SessionEvent::RunClosed { ending } => self.close_unfinished(ending),
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
                WireEvent::AgentMessageChunk { .. } | WireEvent::ThoughtChunk { .. }
            )
        }) {
            self.message = None;
        }
        events
    }

    /// The prompt as the user message the turn saved. Nothing when no
    /// prompt was noted, since the event does not carry the message.
    fn user_message(&mut self, inserted: &InsertedMessage) -> Option<WireEvent> {
        let content = self.prompt.take()?;
        let message = match inserted {
            InsertedMessage::Saved(origin) => saved_message_id(origin),
            InsertedMessage::Unreplayed => fresh_message_id(),
        };
        Some(WireEvent::UserMessage { message, content })
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
                vec![self.usage(*context_estimate)]
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
                let arguments = self
                    .proposed
                    .remove(call_id.as_str())
                    .map(|call| call.arguments);
                let (id, superseded) = self.announce(call_id.as_str(), CallState::Open);
                let opened = opened_call(
                    id,
                    name,
                    arguments,
                    metadata,
                    &self.workspace,
                    &self.mcp_tools,
                );
                superseded
                    .into_iter()
                    .chain([WireEvent::tool_call(opened)])
                    .collect()
            }
            RunEvent::ToolUpdated { call_id, progress } => {
                let shown = CallUpdate {
                    status: ToolStatus::InProgress,
                    content: vec![text_content(progress.message())],
                    locations: None,
                    raw_output: None,
                };
                self.tool_update(call_id, CallState::Open, shown)
                    .into_iter()
                    .collect()
            }
            RunEvent::ToolFinished { call_id, result } => {
                let finished = finished_update(result, &self.workspace);
                let finished = self.tool_update(call_id, CallState::Finished, finished);
                // The call may have replaced the checklist, whether it ran
                // directly or inside code mode.
                finished.into_iter().chain(self.plan_change()).collect()
            }
            RunEvent::ProviderStreamReset { .. } => {
                // The retried response proposes its calls again.
                self.proposed.clear();
                vec![WireEvent::ResponseDiscarded]
            }
            RunEvent::CompactionStarted { .. } => {
                let id = fresh_compaction_id();
                self.compaction = Some(id.clone());
                vec![compaction(id, CompactionStatus::Started)]
            }
            RunEvent::CompactionCompleted { .. } => {
                // A completion whose start was not seen still gets an id.
                let id = self.compaction.take().unwrap_or_else(fresh_compaction_id);
                vec![compaction(id, CompactionStatus::Completed)]
            }
            // A cancel ends the compaction under way as cancelled; the
            // calls it interrupted close when the run's events end.
            RunEvent::Cancelled { .. } => self
                .compaction
                .take()
                .map(|id| compaction(id, CompactionStatus::Cancelled))
                .into_iter()
                .collect(),
            // [`SessionEvent::RunClosed`] closes what the run left open.
            RunEvent::Started { .. } | RunEvent::Completed { .. } | RunEvent::Failed { .. } => {
                Vec::new()
            }
            // [`SessionEvent::BoundaryApplied`] carried the rows it shows.
            RunEvent::BoundaryInputApplied { .. } => Vec::new(),
            // Questions arrive as [`SessionEvent::QuestionAsked`].
            RunEvent::HostInputRequested { .. } | RunEvent::ToolHostInputRequested { .. } => {
                Vec::new()
            }
            // Streaming tool input lands with the TUI's tool cards (#1667);
            // steering, retries, diagnostics, token accounting and
            // model-call metrics land as later minors.
            RunEvent::ToolCallUpdated { .. }
            | RunEvent::UsageUpdated { .. }
            | RunEvent::ProviderContextUpdated { .. }
            | RunEvent::SteeringApplied { .. }
            | RunEvent::SteeringDelivered { .. }
            | RunEvent::ProviderDiagnostic { .. }
            | RunEvent::WebSearch { .. }
            | RunEvent::ProviderRequestRetry
            | RunEvent::ModelCallCompleted { .. }
            | RunEvent::HostedToolActivity { .. }
            | RunEvent::ProviderServiceTierFallback { .. } => Vec::new(),
            // RunEvent is non_exhaustive; a newer SDK's events wait for the
            // wire to carry them.
            _ => Vec::new(),
        }
    }

    /// Context fill from `estimate`, against the configured window, else
    /// the one the provider reported with it. The daemon prices it.
    fn usage(&self, estimate: ContextEstimate) -> WireEvent {
        WireEvent::Usage {
            used_tokens: estimate.tokens(),
            window_tokens: self
                .configured_window
                .or(estimate.reported_context_window()),
            cost: None,
        }
    }

    /// A chunk of the open message of `kind`, or of a new message when the
    /// previous event was anything else.
    fn chunk(&mut self, kind: ChunkKind, text: &str) -> WireEvent {
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
    /// next run starts clean.
    fn close_unfinished(&mut self, ending: &RunEnd) -> Vec<WireEvent> {
        self.proposed.clear();
        let compaction = self.compaction.take().map(|id| {
            let error = match ending {
                RunEnd::Failed(message) => message.clone(),
                RunEnd::Ended => UNFINISHED_COMPACTION.to_owned(),
            };
            compaction(id, CompactionStatus::Failed { error })
        });
        let announced = std::mem::take(&mut self.announced);
        let calls = announced
            .into_values()
            .filter(|call| call.state == CallState::Open)
            .map(|call| WireEvent::tool_call(status_update(call.id, Some(ToolStatus::Failed))));
        compaction.into_iter().chain(calls).collect()
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
            raw_output: shown.raw_output,
            ..call
        };
        if let Some(call) = self.announced.get_mut(id) {
            if call.state == CallState::Finished && next == CallState::Open {
                return None;
            }
            call.state = next;
            return Some(WireEvent::tool_call(update(status_update(
                call.id.clone(),
                /*status*/ None,
            ))));
        }
        let (wire_id, _) = self.announce(id, next);
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
            None => ToolCallUpdate {
                title: Some(UNNAMED_TOOL_TITLE.to_owned()),
                kind: Some(ToolKind::Other),
                ..status_update(wire_id, /*status*/ None)
            },
        };
        Some(WireEvent::tool_call(update(card)))
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
            .filter(|call| call.state == CallState::Open)
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
}

/// What a progress or result update shows of a call.
struct CallUpdate {
    status: ToolStatus,
    content: Vec<ToolContent>,
    /// Replace the call's earlier locations when given.
    locations: Option<Vec<Location>>,
    raw_output: Option<Value>,
}

/// A finished call's status and content, the files its output points at,
/// and, as its raw output, the text the model got, as replay sends a saved
/// result's. A call to a tool that does not exist has none: the SDK writes
/// the model's text for it.
fn finished_update(result: &ToolCompletion, cwd: &Path) -> CallUpdate {
    let finished = |status, content, raw_output: Option<&str>| CallUpdate {
        status,
        content,
        locations: result
            .output()
            .and_then(|output| tools::finished_locations(output, cwd)),
        raw_output: raw_output.map(|text| Value::String(text.to_owned())),
    };
    match result {
        ToolCompletion::Completed(output) => {
            let status = if output.is_failure() {
                ToolStatus::Failed
            } else {
                ToolStatus::Completed
            };
            finished(status, output_content(output, cwd), Some(output.content()))
        }
        ToolCompletion::Failure(failure) => finished(
            ToolStatus::Failed,
            vec![text_content(failure.message())],
            Some(failure.message()),
        ),
        // The model called a tool that does not exist; say so on the card.
        ToolCompletion::Unavailable => finished(
            ToolStatus::Failed,
            vec![text_content("tool is unavailable")],
            None,
        ),
        // ToolCompletion is non_exhaustive; unknown results cannot be shown
        // as success.
        _ => finished(ToolStatus::Failed, Vec::new(), None),
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
                }),
        );
    }
    content
}

fn compaction(id: CompactionId, status: CompactionStatus) -> WireEvent {
    WireEvent::Compaction { id, status }
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

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
