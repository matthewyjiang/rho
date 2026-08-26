use std::collections::HashMap;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, ImageContent, SessionId, SessionNotification, SessionUpdate,
    StopReason, ToolCall, ToolCallContent, ToolCallId, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use rho_sdk::{
    model::Message,
    tool::{OperationKind, ToolMetadata, ToolOutput},
    RunEvent, ToolCompletion,
};
use serde_json::Value;

const PROVIDER_STREAM_RESET_NOTICE: &str = "[provider response discarded; retrying]";

/// Maps one prompt's SDK events onto ACP session updates.
///
/// Proposed tool arguments are held until [`RunEvent::ToolStarted`] so the first
/// ACP `ToolCall` can carry kind, locations, and raw input together.
pub(super) struct EventMapper {
    proposed: HashMap<String, Value>,
}

impl EventMapper {
    pub(super) fn new() -> Self {
        Self {
            proposed: HashMap::new(),
        }
    }

    /// Maps one SDK event onto at most one ACP notification. Events that only
    /// move mapper state, or that ACP has no update for, map to `None`.
    #[allow(deprecated)]
    pub(super) fn map_event(
        &mut self,
        session_id: &SessionId,
        event: &rho_sdk::RunEvent,
    ) -> Option<SessionNotification> {
        match event {
            RunEvent::AssistantTextDelta { text } => Some(notify(
                session_id,
                SessionUpdate::AgentMessageChunk(text_chunk(text)),
            )),
            RunEvent::ReasoningDelta { text } | RunEvent::ReasoningSummaryDelta { text } => {
                Some(notify(
                    session_id,
                    SessionUpdate::AgentThoughtChunk(text_chunk(text)),
                ))
            }
            RunEvent::ToolProposed { call } => {
                if !call.id.is_empty() {
                    self.proposed
                        .insert(call.id.clone(), call.arguments.clone());
                }
                None
            }
            RunEvent::ToolStarted {
                call_id,
                name,
                metadata,
            } => {
                let proposed = self.proposed.remove(call_id.as_str());
                Some(notify(
                    session_id,
                    SessionUpdate::ToolCall(started_tool_call(call_id, name, metadata, proposed)),
                ))
            }
            RunEvent::ToolUpdated { call_id, progress } => Some(notify(
                session_id,
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    ToolCallId::new(call_id.as_str()),
                    ToolCallUpdateFields::new()
                        .status(ToolCallStatus::InProgress)
                        .content(vec![ToolCallContent::from(progress.text())]),
                )),
            )),
            RunEvent::ToolFinished { call_id, result } => {
                self.proposed.remove(call_id.as_str());
                let (status, content) = finished_content(result);
                Some(notify(
                    session_id,
                    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                        ToolCallId::new(call_id.as_str()),
                        ToolCallUpdateFields::new().status(status).content(content),
                    )),
                ))
            }
            RunEvent::ProviderStreamReset { .. } => {
                self.proposed.clear();
                Some(notify(
                    session_id,
                    SessionUpdate::AgentThoughtChunk(text_chunk(PROVIDER_STREAM_RESET_NOTICE)),
                ))
            }
            RunEvent::Completed { .. } | RunEvent::Cancelled { .. } | RunEvent::Failed { .. } => {
                self.proposed.clear();
                None
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
            | RunEvent::ProviderDiagnostic { .. }
            | RunEvent::ToolHostInputRequested { .. }
            | RunEvent::WebSearch { .. }
            | RunEvent::ProviderRequestRetry
            | RunEvent::ModelCallCompleted { .. }
            | RunEvent::HostedToolActivity { .. }
            | RunEvent::ProviderServiceTierFallback { .. } => None,
            // RunEvent is non_exhaustive; unknown future variants have no ACP update.
            _ => None,
        }
    }

    pub(super) fn map_stop(outcome: &rho_sdk::RunOutcome) -> StopReason {
        map_sdk_stop_reason(outcome.stop_reason())
    }

    pub(super) fn replay_history(
        session_id: &SessionId,
        messages: &[rho_sdk::model::Message],
    ) -> Vec<SessionNotification> {
        messages
            .iter()
            .flat_map(|message| replay_message(session_id, message))
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
        ToolCompletion::Success(output) => (ToolCallStatus::Completed, success_content(output)),
        ToolCompletion::Failure(failure) => (
            ToolCallStatus::Failed,
            vec![ToolCallContent::from(failure.message())],
        ),
        ToolCompletion::Unavailable => (ToolCallStatus::Failed, Vec::new()),
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

fn replay_message(session_id: &SessionId, message: &Message) -> Vec<SessionNotification> {
    match message {
        Message::User(blocks) => replay_blocks(session_id, ReplayRole::User, blocks),
        Message::Assistant(blocks) => replay_blocks(session_id, ReplayRole::Agent, blocks),
        Message::EnrichedAssistant(message) => {
            let mut notifications = replay_blocks(session_id, ReplayRole::Agent, &message.content);
            if let Some(summary) = message
                .reasoning_summary
                .as_deref()
                .filter(|summary| !summary.is_empty())
            {
                notifications.push(notify(
                    session_id,
                    SessionUpdate::AgentThoughtChunk(text_chunk(summary)),
                ));
            }
            notifications
        }
        Message::AbortedAssistant(message) => {
            replay_blocks(session_id, ReplayRole::Agent, &message.content)
        }
        Message::System(_) | Message::ToolResult(_) => Vec::new(),
    }
}

#[derive(Clone, Copy)]
enum ReplayRole {
    User,
    Agent,
}

fn replay_blocks(
    session_id: &SessionId,
    role: ReplayRole,
    blocks: &[rho_sdk::model::ContentBlock],
) -> Vec<SessionNotification> {
    blocks
        .iter()
        .filter_map(|block| replay_block(block).map(|content| chunk_for(role, content)))
        .map(|update| notify(session_id, update))
        .collect()
}

fn replay_block(block: &rho_sdk::model::ContentBlock) -> Option<ContentBlock> {
    match block {
        rho_sdk::model::ContentBlock::Text(text) if !text.is_empty() => {
            Some(ContentBlock::from(text.clone()))
        }
        rho_sdk::model::ContentBlock::Image(image) => Some(ContentBlock::Image(ImageContent::new(
            image.data.clone(),
            image.mime_type.clone(),
        ))),
        rho_sdk::model::ContentBlock::Text(_) | rho_sdk::model::ContentBlock::ToolCall(_) => None,
    }
}

fn chunk_for(role: ReplayRole, content: ContentBlock) -> SessionUpdate {
    let chunk = ContentChunk::new(content);
    match role {
        ReplayRole::User => SessionUpdate::UserMessageChunk(chunk),
        ReplayRole::Agent => SessionUpdate::AgentMessageChunk(chunk),
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
