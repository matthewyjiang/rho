//! Rebuild a client's transcript from persisted display history, without
//! executing tools. ACP tool updates always follow a full call for that id.

use std::collections::BTreeMap;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, ImageContent, SessionId, SessionNotification, SessionUpdate,
    ToolCall, ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use rho_sdk::model::{self, Message, SemanticMessage};
use serde_json::Value;

use super::{notify, text_chunk, EventMapper, UNNAMED_TOOL_TITLE};
use crate::display_transcript::DisplayTranscript;

/// Hosts with their own transcript use this marker to avoid duplicating it.
const REPLAY_META_KEY: &str = "isReplay";

pub(super) fn history(session_id: &SessionId, messages: &[Message]) -> Vec<SessionNotification> {
    let mut replay = Replay::default();
    let mut updates = Vec::new();
    for message in messages {
        replay.message(session_id, message, &mut updates);
    }
    // A killed process can leave a saved call without a result. History is not
    // running work; ACP has no cancelled status, so do not leave cards spinning.
    for call in replay.tools.values() {
        if matches!(
            call.status,
            ToolCallStatus::Pending | ToolCallStatus::InProgress
        ) {
            updates.push(notify(
                session_id,
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    call.tool_call_id.clone(),
                    ToolCallUpdateFields::new().status(ToolCallStatus::Failed),
                )),
            ));
        }
    }
    let meta = serde_json::Map::from_iter([(REPLAY_META_KEY.to_owned(), Value::Bool(true))]);
    updates
        .into_iter()
        .map(|update| update.meta(meta.clone()))
        .collect()
}

#[derive(Default)]
struct Replay {
    tools: BTreeMap<String, ToolCall>,
}

impl Replay {
    fn message(
        &mut self,
        session_id: &SessionId,
        message: &Message,
        notifications: &mut Vec<SessionNotification>,
    ) {
        match message.semantic() {
            SemanticMessage::ToolResult(result) => {
                let mut call =
                    ToolCall::new(ToolCallId::new(result.id.clone()), UNNAMED_TOOL_TITLE)
                        .status(if result.ok {
                            ToolCallStatus::Completed
                        } else {
                            ToolCallStatus::Failed
                        })
                        .raw_output(Value::String(result.content.clone()));
                if !result.content.is_empty() {
                    call.content
                        .push(ToolCallContent::from(result.content.clone()));
                }
                notifications.push(notify(session_id, self.tool_update(call)));
            }
            SemanticMessage::ToolImageSupplement(images) => {
                let status = self
                    .tools
                    .get(images.tool_call_id())
                    .map_or(ToolCallStatus::Completed, |call| call.status);
                let call =
                    ToolCall::new(ToolCallId::new(images.tool_call_id()), images.tool_name())
                        .name(images.tool_name().to_owned())
                        .status(status)
                        .content(
                            images
                                .images()
                                .map(|image| {
                                    ToolCallContent::from(ContentBlock::Image(ImageContent::new(
                                        image.data.clone(),
                                        image.mime_type.clone(),
                                    )))
                                })
                                .collect(),
                        );
                notifications.push(notify(session_id, self.tool_update(call)));
            }
            SemanticMessage::User(blocks) => {
                self.blocks(session_id, ReplayRole::User, blocks, notifications)
            }
            SemanticMessage::Assistant(blocks) => {
                self.blocks(session_id, ReplayRole::Agent, blocks, notifications)
            }
            SemanticMessage::EnrichedAssistant(message) => {
                self.blocks(
                    session_id,
                    ReplayRole::Agent,
                    &message.content,
                    notifications,
                );
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
            }
            SemanticMessage::AbortedAssistant(message) => self.blocks(
                session_id,
                ReplayRole::Agent,
                &message.content,
                notifications,
            ),
            SemanticMessage::System(text) => {
                if let Some(transcript) = DisplayTranscript::from_display(text) {
                    notifications.extend(EventMapper::display_transcript(session_id, &transcript));
                }
            }
        }
    }

    fn blocks(
        &mut self,
        session_id: &SessionId,
        role: ReplayRole,
        blocks: &[model::ContentBlock],
        notifications: &mut Vec<SessionNotification>,
    ) {
        for block in blocks {
            let content = match block {
                model::ContentBlock::Text(text) if !text.is_empty() => {
                    ContentBlock::from(text.clone())
                }
                model::ContentBlock::Image(image) => ContentBlock::Image(ImageContent::new(
                    image.data.clone(),
                    image.mime_type.clone(),
                )),
                model::ContentBlock::ToolCall(call) => {
                    let tool = ToolCall::new(ToolCallId::new(call.id.clone()), call.name.clone())
                        .name(call.name.clone())
                        .status(ToolCallStatus::Pending)
                        .raw_input(call.arguments.clone());
                    self.tools.insert(call.id.clone(), tool.clone());
                    notifications.push(notify(session_id, SessionUpdate::ToolCall(tool)));
                    continue;
                }
                model::ContentBlock::Text(_) => continue,
            };
            let chunk = ContentChunk::new(content);
            let update = match role {
                ReplayRole::User => SessionUpdate::UserMessageChunk(chunk),
                ReplayRole::Agent => SessionUpdate::AgentMessageChunk(chunk),
            };
            notifications.push(notify(session_id, update));
        }
    }

    /// Results and supplemental images replace ACP content as a whole, so
    /// retain earlier output. An orphan result still opens a complete card.
    fn tool_update(&mut self, call: ToolCall) -> SessionUpdate {
        let id = call.tool_call_id.0.to_string();
        if let Some(previous) = self.tools.get_mut(&id) {
            previous.status = call.status;
            previous.content.extend(call.content);
            let mut fields = ToolCallUpdateFields::new()
                .status(previous.status)
                .content(previous.content.clone());
            if let Some(output) = call.raw_output {
                fields = fields.raw_output(output);
            }
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(call.tool_call_id, fields))
        } else {
            self.tools.insert(id, call.clone());
            SessionUpdate::ToolCall(call)
        }
    }
}

#[derive(Clone, Copy)]
enum ReplayRole {
    User,
    Agent,
}

#[cfg(test)]
#[path = "events_replay_tests.rs"]
mod tests;
