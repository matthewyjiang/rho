//! Maps a loaded session's live runtime events onto wire events.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use rho_protocol::{
    ContentBlock, MessageId, PlanEntry, SessionEvent as WireEvent, ToolCallId, ToolCallUpdate,
    ToolContent, ToolKind, ToolStatus,
};
use rho_sdk::{
    model,
    tool::{ToolMetadata, ToolOutput},
    RunEvent, ToolCompletion, UserInput,
};

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

/// Told when the provider discarded a response it had begun streaming and
/// the run retries it: clients cannot retract the text they were sent.
const STREAM_RESET_NOTICE: &str = "provider response discarded; retrying";
/// The title of the call that shows a compaction.
const COMPACTION_TITLE: &str = "compact context";
/// A compaction the run ended before it reported a result.
const UNFINISHED_COMPACTION: &str = "compaction ended without a result";

/// Turns one session's live runtime events into wire events. One per loaded
/// session; keeps the message ids and open tool calls it has announced.
///
/// Proposed tool arguments are held until [`RunEvent::ToolStarted`], so a
/// call opens with its title, kind, arguments and locations together. Every
/// call opens with a full card, even one the run finishes without starting,
/// such as a call to a tool that does not exist. When the run's events end
/// ([`SessionEvent::RunClosed`]), calls still open close `failed`: the SDK
/// does not finish calls a cancel interrupts. A compaction shows as a call
/// of its own and closes the same way.
///
/// Agent message and thought chunks carry a message id. Consecutive chunks
/// of one kind share it; a chunk of the other kind, a new model step, or any
/// other event in between, such as a tool call, starts a new message.
///
/// The session's checklist goes out as a plan whenever a finished tool
/// changed it, and each model step reports its context fill as usage.
///
/// It also replays the session's saved rows ([`Self::replay`]), so a call
/// id a provider reuses is numbered the same way live and replayed, and the
/// turn in progress never reuses a wire id the saved transcript holds.
pub(crate) struct LiveEvents {
    /// The session's working directory; titles show paths relative to it.
    workspace: PathBuf,
    /// Identifies MCP calls, whose capped names cannot be decoded.
    mcp_tools: McpToolNames,
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
    /// The compaction call still open.
    compaction: Option<ToolCallId>,
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
            // The daemon reports the job's state itself: a turn's start, its
            // waits for input, and its end.
            SessionEvent::RunStarted | SessionEvent::RunFinished { .. } => Vec::new(),
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
                vec![WireEvent::Usage {
                    used_tokens: context_estimate.tokens(),
                    window_tokens: context_estimate.reported_context_window(),
                }]
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
                    .chain([WireEvent::ToolCall(opened)])
                    .collect()
            }
            RunEvent::ToolUpdated { call_id, progress } => self
                .tool_update(
                    call_id,
                    CallState::Open,
                    ToolStatus::InProgress,
                    vec![text_content(progress.message())],
                    /*locations*/ None,
                )
                .into_iter()
                .collect(),
            RunEvent::ToolFinished { call_id, result } => {
                let (status, content) = finished_content(result, &self.workspace);
                let locations = result
                    .output()
                    .and_then(|output| tools::finished_locations(output, &self.workspace));
                let finished =
                    self.tool_update(call_id, CallState::Finished, status, content, locations);
                // The call may have replaced the checklist, whether it ran
                // directly or inside code mode.
                finished.into_iter().chain(self.plan_change()).collect()
            }
            RunEvent::ProviderStreamReset { .. } => {
                // The retried response proposes its calls again.
                self.proposed.clear();
                vec![WireEvent::Notice {
                    text: STREAM_RESET_NOTICE.to_owned(),
                }]
            }
            RunEvent::CompactionStarted { .. } => {
                let id = fresh_compaction_id();
                self.compaction = Some(id.clone());
                vec![compaction_call(
                    id,
                    ToolStatus::InProgress,
                    /*content*/ None,
                )]
            }
            RunEvent::CompactionCompleted { outcome, .. } => {
                let id = self.compaction.take().unwrap_or_else(fresh_compaction_id);
                let summary = text_content(compaction_summary(outcome));
                vec![compaction_call(id, ToolStatus::Completed, Some(summary))]
            }
            // [`SessionEvent::RunClosed`] closes what the run left open.
            RunEvent::Started { .. }
            | RunEvent::Completed { .. }
            | RunEvent::Cancelled { .. }
            | RunEvent::Failed { .. } => Vec::new(),
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
            let reason = match ending {
                RunEnd::Failed(message) => message.as_str(),
                RunEnd::Ended => UNFINISHED_COMPACTION,
            };
            compaction_call(id, ToolStatus::Failed, Some(text_content(reason)))
        });
        let announced = std::mem::take(&mut self.announced);
        let calls = announced
            .into_values()
            .filter(|call| call.state == CallState::Open)
            .map(|call| WireEvent::ToolCall(status_update(call.id, Some(ToolStatus::Failed))));
        compaction.into_iter().chain(calls).collect()
    }

    /// A progress or result update for a call, sent as a full card when
    /// clients have not seen the call yet. Progress that arrives after a
    /// call finished is dropped, so it cannot reopen the call. `locations`
    /// replace the call's earlier ones when given.
    fn tool_update(
        &mut self,
        call_id: &rho_sdk::ToolCallId,
        next: CallState,
        status: ToolStatus,
        content: Vec<ToolContent>,
        locations: Option<Vec<rho_protocol::Location>>,
    ) -> Option<WireEvent> {
        let id = call_id.as_str();
        let update = move |call: ToolCallUpdate| ToolCallUpdate {
            status: Some(status),
            content: Some(content),
            locations,
            ..call
        };
        if let Some(call) = self.announced.get_mut(id) {
            if call.state == CallState::Finished && next == CallState::Open {
                return None;
            }
            call.state = next;
            return Some(WireEvent::ToolCall(update(status_update(
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
        Some(WireEvent::ToolCall(update(card)))
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
            .map(|call| WireEvent::ToolCall(status_update(call.id, Some(ToolStatus::Failed))));
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

/// A finished call's status and content.
fn finished_content(result: &ToolCompletion, cwd: &Path) -> (ToolStatus, Vec<ToolContent>) {
    match result {
        ToolCompletion::Completed(output) if output.is_failure() => {
            (ToolStatus::Failed, output_content(output, cwd))
        }
        ToolCompletion::Completed(output) => (ToolStatus::Completed, output_content(output, cwd)),
        ToolCompletion::Failure(failure) => {
            (ToolStatus::Failed, vec![text_content(failure.message())])
        }
        // The model called a tool that does not exist; say so on the card.
        ToolCompletion::Unavailable => (
            ToolStatus::Failed,
            vec![text_content("tool is unavailable")],
        ),
        // ToolCompletion is non_exhaustive; unknown results cannot be shown
        // as success.
        _ => (ToolStatus::Failed, Vec::new()),
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

/// The call that shows a compaction, with its title each time, so a first
/// update with a terminal status still opens a complete card.
fn compaction_call(id: ToolCallId, status: ToolStatus, content: Option<ToolContent>) -> WireEvent {
    WireEvent::ToolCall(ToolCallUpdate {
        title: Some(COMPACTION_TITLE.to_owned()),
        kind: Some(ToolKind::Other),
        content: content.map(|content| vec![content]),
        ..status_update(id, Some(status))
    })
}

/// What a finished compaction did, in the words `/compact` replies with.
fn compaction_summary(outcome: &rho_sdk::CompactionOutcome) -> String {
    if crate::compaction::outcome_reduced_context(outcome) {
        format!(
            "context compacted: {} → {} messages, {} → {} tokens",
            outcome.previous_messages(),
            outcome.current_messages(),
            outcome.previous_tokens(),
            outcome.current_tokens(),
        )
    } else {
        "context not compacted: compaction ran but did not reduce context".into()
    }
}

/// A live message's id. Replayed messages derive theirs from saved rows.
fn fresh_message_id() -> MessageId {
    MessageId::new(format!("msg-{}", uuid::Uuid::new_v4()))
}

/// Each compaction gets a fresh random id: clients may keep a session's
/// events across daemon restarts, and an id that restarts per process
/// would update an earlier card.
fn fresh_compaction_id() -> ToolCallId {
    ToolCallId::new(format!("compaction-{}", uuid::Uuid::new_v4()))
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
