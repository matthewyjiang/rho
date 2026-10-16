use std::collections::{BTreeMap, HashMap};

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionId, SessionNotification, SessionUpdate, StopReason,
    ToolCall, ToolCallContent, ToolCallId, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind,
};
use rho_sdk::{
    tool::{OperationKind, ToolMetadata, ToolOutput},
    RunEvent, ToolCompletion,
};
use serde_json::Value;

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
pub(super) struct EventMapper {
    proposed: HashMap<String, rho_sdk::model::ToolCall>,
    announced: BTreeMap<String, CallState>,
}

/// Whether the host may still be waiting for a call's final status.
#[derive(Clone, Copy, PartialEq)]
enum CallState {
    Open,
    Finished,
}

impl EventMapper {
    pub(super) fn new() -> Self {
        Self {
            proposed: HashMap::new(),
            announced: BTreeMap::new(),
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
            RunEvent::Completed { .. } | RunEvent::Cancelled { .. } | RunEvent::Failed { .. } => {
                self.proposed.clear();
                return self.close_open_calls(session_id);
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

    /// Final `failed` updates for calls the host saw start but never saw
    /// finish. The run's terminal event closes them; the pump calls this again
    /// when the event stream ends, for a run that ended without one.
    pub(super) fn close_open_calls(&mut self, session_id: &SessionId) -> Vec<SessionNotification> {
        self.announced
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
            })
            .collect()
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
    if let Some(diff) = output.presentation().unified_diff() {
        // unified_diff is patch text, not the file's new contents. ACP Diff.new_text
        // is the post-edit file, so the patch is sent as ordinary tool content
        // whether or not an affected path is present.
        content.push(ToolCallContent::from(diff));
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
