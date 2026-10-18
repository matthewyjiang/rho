use std::collections::{BTreeMap, HashMap};

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, Diff, Notice, NoticeSeverity, SessionId, SessionNotification,
    SessionUpdate, StopReason, ToolCall, ToolCallContent, ToolCallId, ToolCallLocation,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use rho_sdk::{
    tool::{OperationKind, ToolMetadata, ToolOutput},
    RunEvent, ToolCompletion,
};
use serde_json::Value;

use super::{
    compaction_card::{self, CompactionCard},
    HostUpdates,
};
use crate::{
    display_transcript::{DisplayRow, DisplayTranscript},
    presentation::NotificationTone,
};

const PROVIDER_STREAM_RESET_NOTICE: &str = "[provider response discarded; retrying]";
/// Title for a tool call whose name the mapper never saw proposed.
const UNNAMED_TOOL_TITLE: &str = "tool";

#[path = "events_replay.rs"]
mod replay;

/// Maps one prompt's SDK events onto ACP session updates.
///
/// Proposed tool arguments are held until [`RunEvent::ToolStarted`] so the first
/// ACP `ToolCall` can carry name, kind, locations, and raw input together.
///
/// Hosts only accept a `tool_call_update` for an id they saw in a `tool_call`
/// (Zed shows anything else as a failed "Tool call not found" card), so every
/// call opens with a full `tool_call`, even one the SDK finishes without
/// starting, such as a call to a tool that does not exist. When the run ends,
/// calls still open are closed as failed, since the SDK does not finish calls
/// that a cancel interrupts and ACP has no cancelled tool status.
///
/// Hosts that advertise compaction see each compaction the run performs as
/// a card; one still open when the run ends is closed the same way.
pub(super) struct EventMapper {
    updates: HostUpdates,
    proposed: HashMap<String, rho_sdk::model::ToolCall>,
    announced: BTreeMap<String, CallState>,
    compaction: Option<CompactionCard>,
}

/// Whether the host may still be waiting for a call's final status.
#[derive(Clone, Copy, PartialEq)]
enum CallState {
    Open,
    Finished,
}

/// How a run ended, for closing what it left open.
#[derive(Clone, Copy, Debug)]
pub(super) enum RunEnding<'a> {
    Cancelled,
    Failed(&'a str),
    /// It completed, or its event stream ended without a terminal event.
    WithoutResult,
}

impl EventMapper {
    pub(super) fn new(updates: HostUpdates) -> Self {
        Self {
            updates,
            proposed: HashMap::new(),
            announced: BTreeMap::new(),
            compaction: None,
        }
    }

    /// Maps one SDK event onto ACP notifications. Events that only move mapper
    /// state, or that ACP has no update for, map to nothing; a run's terminal
    /// event maps to the closing updates for calls still open.
    pub(super) fn map_event(
        &mut self,
        session_id: &SessionId,
        event: &rho_sdk::RunEvent,
    ) -> Vec<SessionNotification> {
        let update = match event {
            RunEvent::AssistantTextDelta { text } => {
                Some(SessionUpdate::AgentMessageChunk(text_chunk(text)))
            }
            RunEvent::ReasoningDelta { text } | RunEvent::ReasoningSummaryDelta { text } => {
                Some(SessionUpdate::AgentThoughtChunk(text_chunk(text)))
            }
            RunEvent::ToolProposed { call } => {
                if !call.id.is_empty() {
                    self.proposed.insert(call.id.clone(), call.clone());
                }
                None
            }
            RunEvent::ToolStarted {
                call_id,
                name,
                metadata,
            } => {
                let proposed = self
                    .proposed
                    .remove(call_id.as_str())
                    .map(|call| call.arguments);
                self.announced
                    .insert(call_id.as_str().to_owned(), CallState::Open);
                Some(SessionUpdate::ToolCall(started_tool_call(
                    call_id, name, metadata, proposed,
                )))
            }
            RunEvent::ToolUpdated { call_id, progress } => self.tool_update(
                call_id,
                CallState::Open,
                ToolCallStatus::InProgress,
                vec![ToolCallContent::from(progress.text())],
            ),
            RunEvent::ToolFinished { call_id, result } => {
                let (status, content) = finished_content(result);
                self.tool_update(call_id, CallState::Finished, status, content)
            }
            RunEvent::ProviderStreamReset { .. } => {
                self.proposed.clear();
                Some(SessionUpdate::AgentThoughtChunk(text_chunk(
                    PROVIDER_STREAM_RESET_NOTICE,
                )))
            }
            RunEvent::CompactionStarted { .. } if self.updates.compaction => {
                let card = CompactionCard::new();
                let update = card.in_progress();
                self.compaction = Some(card);
                Some(update)
            }
            RunEvent::CompactionCompleted { outcome, .. } if self.updates.compaction => {
                // A first update with a terminal status still creates the card.
                let card = self.compaction.take().unwrap_or_else(CompactionCard::new);
                Some(card.completed(outcome))
            }
            RunEvent::Cancelled { .. } => {
                return self.close_unfinished(session_id, RunEnding::Cancelled);
            }
            RunEvent::Failed { message, .. } => {
                return self.close_unfinished(session_id, RunEnding::Failed(message));
            }
            RunEvent::Completed { .. } => {
                return self.close_unfinished(session_id, RunEnding::WithoutResult);
            }
            RunEvent::Started { .. }
            | RunEvent::StepStarted { .. }
            | RunEvent::ToolCallUpdated { .. }
            | RunEvent::UsageUpdated { .. }
            | RunEvent::ProviderContextUpdated { .. }
            | RunEvent::HostInputRequested { .. }
            | RunEvent::CompactionStarted { .. }
            | RunEvent::CompactionCompleted { .. }
            | RunEvent::SteeringApplied { .. }
            | RunEvent::SteeringDelivered { .. }
            | RunEvent::ProviderDiagnostic { .. }
            | RunEvent::ToolHostInputRequested { .. }
            | RunEvent::WebSearch { .. }
            | RunEvent::ProviderRequestRetry
            | RunEvent::ModelCallCompleted { .. }
            | RunEvent::HostedToolActivity { .. }
            | RunEvent::ProviderServiceTierFallback { .. } => None,
            // RunEvent is non_exhaustive; unknown future variants have no ACP update.
            _ => None,
        };
        update
            .map(|update| notify(session_id, update))
            .into_iter()
            .collect()
    }

    /// Final updates for the compaction and calls the host saw start but
    /// never saw finish: calls close `failed`, the compaction with `ending`.
    /// The run's terminal event closes them; the pump calls this again when
    /// the event stream ends, for a run that ended without one.
    pub(super) fn close_unfinished(
        &mut self,
        session_id: &SessionId,
        ending: RunEnding<'_>,
    ) -> Vec<SessionNotification> {
        self.proposed.clear();
        let compaction = self.compaction.take().map(|card| {
            let update = match ending {
                RunEnding::Cancelled => card.cancelled(),
                RunEnding::Failed(message) => card.failed(message),
                RunEnding::WithoutResult => card.failed(compaction_card::UNFINISHED),
            };
            notify(session_id, update)
        });
        let calls = self
            .announced
            .iter_mut()
            .filter(|(_, state)| **state == CallState::Open)
            .map(|(call_id, state)| {
                *state = CallState::Finished;
                notify(
                    session_id,
                    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                        ToolCallId::new(call_id.as_str()),
                        ToolCallUpdateFields::new().status(ToolCallStatus::Failed),
                    )),
                )
            });
        compaction.into_iter().chain(calls).collect()
    }

    /// A progress or result update for a call, sent as a full `tool_call`
    /// when the host has not seen the call yet. Progress that arrives after
    /// a call finished is dropped, so it cannot reopen the host's card.
    fn tool_update(
        &mut self,
        call_id: &rho_sdk::ToolCallId,
        next: CallState,
        status: ToolCallStatus,
        content: Vec<ToolCallContent>,
    ) -> Option<SessionUpdate> {
        let id = call_id.as_str();
        if let Some(state) = self.announced.get_mut(id) {
            if *state == CallState::Finished && next == CallState::Open {
                return None;
            }
            *state = next;
            return Some(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new(id),
                ToolCallUpdateFields::new().status(status).content(content),
            )));
        }
        self.announced.insert(id.to_owned(), next);
        let proposed = self.proposed.remove(id);
        let title = proposed
            .as_ref()
            .map_or(UNNAMED_TOOL_TITLE, |call| call.name.as_str());
        let mut tool = ToolCall::new(ToolCallId::new(id), title)
            .kind(ToolKind::Other)
            .status(status)
            .content(content);
        if let Some(call) = proposed {
            tool = tool.name(call.name).raw_input(call.arguments);
        }
        Some(SessionUpdate::ToolCall(tool))
    }

    pub(super) fn map_stop(outcome: &rho_sdk::RunOutcome) -> StopReason {
        map_sdk_stop_reason(outcome.stop_reason())
    }

    pub(super) fn replay_history(
        session_id: &SessionId,
        messages: &[rho_sdk::model::Message],
    ) -> Vec<SessionNotification> {
        replay::history(session_id, messages)
    }

    /// A live agent message outside any run, such as a notice at load.
    pub(super) fn agent_notice(session_id: &SessionId, text: &str) -> SessionNotification {
        notify(
            session_id,
            SessionUpdate::AgentMessageChunk(text_chunk(text)),
        )
    }

    /// Information for the user that is not part of the conversation: an ACP
    /// `notice` when the host shows them, else an agent message.
    pub(super) fn info_notice(
        session_id: &SessionId,
        updates: HostUpdates,
        text: &str,
    ) -> SessionNotification {
        if updates.notices {
            notify(
                session_id,
                SessionUpdate::Notice(Notice::new(NoticeSeverity::Info, text)),
            )
        } else {
            Self::agent_notice(session_id, text)
        }
    }

    /// Background results the model was given, one finished tool call card
    /// per display row. Cards keep them out of the agent's message text, and
    /// hosts already render them beside the tool calls that started the work.
    pub(super) fn display_transcript(
        session_id: &SessionId,
        transcript: &DisplayTranscript,
    ) -> Vec<SessionNotification> {
        transcript
            .0
            .iter()
            .map(|row| notify(session_id, SessionUpdate::ToolCall(display_row_call(row))))
            .collect()
    }
}

fn map_sdk_stop_reason(reason: rho_sdk::StopReason) -> StopReason {
    match reason {
        rho_sdk::StopReason::EndTurn => StopReason::EndTurn,
        rho_sdk::StopReason::MaxSteps => StopReason::MaxTurnRequests,
        // StopReason is non_exhaustive; a new SDK reason still ends the ACP turn.
        _ => StopReason::EndTurn,
    }
}

fn notify(session_id: &SessionId, update: SessionUpdate) -> SessionNotification {
    SessionNotification::new(session_id.clone(), update)
}

fn text_chunk(text: impl Into<String>) -> ContentChunk {
    ContentChunk::new(ContentBlock::from(text.into()))
}

fn started_tool_call(
    call_id: &rho_sdk::ToolCallId,
    name: &str,
    metadata: &ToolMetadata,
    proposed: Option<Value>,
) -> ToolCall {
    let title = tool_title(name, metadata);
    let mut tool = ToolCall::new(ToolCallId::new(call_id.as_str()), title)
        .name(name.to_owned())
        .kind(tool_kind(metadata))
        .status(ToolCallStatus::InProgress)
        .locations(tool_locations(metadata.affected_paths()));
    if let Some(arguments) = proposed {
        tool = tool.raw_input(arguments);
    }
    tool
}

fn tool_title(name: &str, metadata: &ToolMetadata) -> String {
    match metadata.command_summary_text() {
        Some(summary) if !summary.is_empty() => format!("{name}: {summary}"),
        _ => name.to_string(),
    }
}

fn tool_kind(metadata: &ToolMetadata) -> ToolKind {
    match metadata.operation_kind() {
        Some(OperationKind::Read) => ToolKind::Read,
        Some(OperationKind::Write) => ToolKind::Edit,
        Some(OperationKind::Execute) => ToolKind::Execute,
        Some(OperationKind::Network) => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

fn tool_locations(paths: &[std::path::PathBuf]) -> Vec<ToolCallLocation> {
    paths.iter().cloned().map(ToolCallLocation::new).collect()
}

fn finished_content(result: &ToolCompletion) -> (ToolCallStatus, Vec<ToolCallContent>) {
    match result {
        ToolCompletion::CompletedFailure(output) => {
            (ToolCallStatus::Failed, success_content(output))
        }
        ToolCompletion::Success(output) => (ToolCallStatus::Completed, success_content(output)),
        ToolCompletion::Failure(failure) => (
            ToolCallStatus::Failed,
            vec![ToolCallContent::from(failure.message())],
        ),
        // The model called a tool that does not exist; say so on the card.
        ToolCompletion::Unavailable => (
            ToolCallStatus::Failed,
            vec![ToolCallContent::from("tool is unavailable")],
        ),
        // ToolCompletion is non_exhaustive; unknown results cannot be shown as success.
        _ => (ToolCallStatus::Failed, Vec::new()),
    }
}

fn success_content(output: &ToolOutput) -> Vec<ToolCallContent> {
    let mut content = Vec::new();
    if !output.content().is_empty() {
        content.push(ToolCallContent::from(output.content()));
    }
    let metadata = output.presentation();
    if metadata.file_changes().is_empty() {
        // A patch is not the complete file text ACP Diff.new_text requires.
        if let Some(diff) = metadata.unified_diff() {
            content.push(ToolCallContent::from(diff));
        }
    } else {
        content.extend(metadata.file_changes().iter().map(|change| {
            ToolCallContent::Diff(
                Diff::new(change.path(), change.new_text())
                    .old_text(change.old_text().map(str::to_owned)),
            )
        }));
    }
    content
}

fn display_row_call(row: &DisplayRow) -> ToolCall {
    let status = match row {
        DisplayRow::Notification(card) => match card.tone {
            NotificationTone::Error => ToolCallStatus::Failed,
            NotificationTone::Neutral
            | NotificationTone::Accent
            | NotificationTone::Success
            | NotificationTone::Warning => ToolCallStatus::Completed,
        },
        DisplayRow::Notice(_) => ToolCallStatus::Completed,
    };
    let text = row.card_text();
    // Rows carry no stable identity of their own; a fresh id per card keeps
    // hosts from merging two deliveries about the same run.
    let id = format!("rho-notice-{}", uuid::Uuid::new_v4());
    let mut call = ToolCall::new(ToolCallId::new(id), text.title)
        .kind(ToolKind::Other)
        .status(status);
    // Content is markdown for hosts that render it. Hosts that show a tool's
    // output as is, such as T3 Code, read `rawOutput` first, so it carries
    // the plain form without code fences.
    if !text.plain.trim().is_empty() {
        call = call
            .content(vec![ToolCallContent::from(text.markdown)])
            .raw_output(Value::String(text.plain));
    }
    call
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
