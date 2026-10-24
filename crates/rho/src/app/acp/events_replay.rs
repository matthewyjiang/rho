//! Rebuild a client's transcript from persisted display history, without
//! executing tools. ACP tool updates always follow a full call for that id.

use std::{collections::BTreeMap, path::Path};

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, ImageContent, MessageId, SessionId, SessionNotification,
    SessionUpdate, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields,
};
use rho_sdk::{
    model::{self, Message, SemanticMessage},
    tool::ToolMetadata,
};
use serde_json::Value;

use super::{notify, text_chunk, tool_title, EventMapper, UNNAMED_TOOL_TITLE};
use crate::{
    display_transcript::DisplayTranscript,
    session::tree::{DisplayRowOrigin, ProjectedMessage},
    tools::mcp::exported_name::McpToolNames,
};

/// Hosts with their own transcript use this marker to avoid duplicating it.
const REPLAY_META_KEY: &str = "isReplay";

pub(super) fn history(
    session_id: &SessionId,
    messages: &[ProjectedMessage],
    cwd: &Path,
    mcp_tools: &McpToolNames,
) -> Vec<SessionNotification> {
    let mut replay = Replay {
        cwd,
        mcp_tools,
        tools: BTreeMap::new(),
        message_ids: MessageIds::default(),
    };
    let mut updates = Vec::new();
    for projected in messages {
        replay.message_ids.enter(projected.origin.as_ref());
        replay.message(session_id, &projected.message, &mut updates);
    }
    // A killed process can leave a saved call without a result. History is not
    // running work; ACP has no cancelled status, so do not leave cards spinning.
    updates.extend(
        replay
            .tools
            .values()
            .filter_map(interrupted_update)
            .map(|update| notify(session_id, update)),
    );
    let meta = serde_json::Map::from_iter([(REPLAY_META_KEY.to_owned(), Value::Bool(true))]);
    updates
        .into_iter()
        .map(|update| update.meta(meta.clone()))
        .collect()
}

/// The ID replay gives the first message of the display row saved at
/// `origin`. A live user message saved there is accepted with it too, so a
/// host matching replayed messages to the ones it was sent finds them.
pub(super) fn saved_message_id(origin: &DisplayRowOrigin) -> MessageId {
    let mut ids = MessageIds::default();
    ids.enter(Some(origin));
    ids.next()
}

struct Replay<'a> {
    /// Session working directory; titles show paths relative to it.
    cwd: &'a Path,
    /// Identifies MCP calls, whose capped names cannot be decoded.
    mcp_tools: &'a McpToolNames,
    /// Latest occurrence by stored/provider ID; ACP identities are session-unique.
    tools: BTreeMap<String, ToolCall>,
    message_ids: MessageIds,
}

impl Replay<'_> {
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
                        SessionUpdate::AgentThoughtChunk(text_chunk(
                            summary,
                            self.message_ids.next(),
                        )),
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
        // Text after a tool call is a new message, as it is live.
        let mut message_id = None;
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
                    let title = tool_title(
                        &call.name,
                        Some(&call.arguments),
                        &ToolMetadata::default(),
                        self.cwd,
                        self.mcp_tools,
                    );
                    let tool = ToolCall::new(fresh_tool_call_id(), title)
                        .name(call.name.clone())
                        .status(ToolCallStatus::Pending)
                        .raw_input(call.arguments.clone());
                    if let Some(update) = self
                        .tools
                        .insert(call.id.clone(), tool.clone())
                        .as_ref()
                        .and_then(interrupted_update)
                    {
                        notifications.push(notify(session_id, update));
                    }
                    notifications.push(notify(session_id, SessionUpdate::ToolCall(tool)));
                    message_id = None;
                    continue;
                }
                model::ContentBlock::Text(_) => continue,
            };
            let chunk = ContentChunk::new(content).message_id(
                message_id
                    .get_or_insert_with(|| self.message_ids.next())
                    .clone(),
            );
            let update = match role {
                ReplayRole::User => SessionUpdate::UserMessageChunk(chunk),
                ReplayRole::Agent => SessionUpdate::AgentMessageChunk(chunk),
            };
            notifications.push(notify(session_id, update));
        }
    }

    /// Results and supplemental images replace ACP content as a whole, so
    /// retain earlier output. An orphan result still opens a complete card.
    fn tool_update(&mut self, mut call: ToolCall) -> SessionUpdate {
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
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                previous.tool_call_id.clone(),
                fields,
            ))
        } else {
            call.tool_call_id = fresh_tool_call_id();
            self.tools.insert(id, call.clone());
            SessionUpdate::ToolCall(call)
        }
    }
}

/// IDs for replayed messages that stay the same each time a session loads.
///
/// A message is named after the display row it was saved in, so an ID is
/// unique to one insertion, even of text saved twice. One saved row can
/// replay as several ACP messages (text after a tool call, a reasoning
/// summary), which are numbered in order.
#[derive(Default)]
struct MessageIds {
    saved: String,
    part: u32,
}

impl MessageIds {
    /// Moves on to the row saved at `origin`. A row the load added has none
    /// and names no message of its own, so it continues the previous row.
    fn enter(&mut self, origin: Option<&DisplayRowOrigin>) {
        if let Some(DisplayRowOrigin { node, row }) = origin {
            self.saved = format!("{node}-{row}");
            self.part = 0;
        }
    }

    /// The ID of the next ACP message the current saved message replays as.
    fn next(&mut self) -> MessageId {
        let id = MessageId::new(format!("rho-replay-msg-{}-{}", self.saved, self.part));
        self.part += 1;
        id
    }
}

/// Provider IDs can repeat across turns. Fresh replay identities also avoid
/// collisions with future live calls when the restored conversation continues.
fn fresh_tool_call_id() -> ToolCallId {
    ToolCallId::new(format!("rho-replay-{}", uuid::Uuid::new_v4()))
}

fn interrupted_update(call: &ToolCall) -> Option<SessionUpdate> {
    matches!(
        call.status,
        ToolCallStatus::Pending | ToolCallStatus::InProgress
    )
    .then(|| {
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            call.tool_call_id.clone(),
            ToolCallUpdateFields::new().status(ToolCallStatus::Failed),
        ))
    })
}

#[derive(Clone, Copy)]
enum ReplayRole {
    User,
    Agent,
}

#[cfg(test)]
#[path = "events_replay_tests.rs"]
mod tests;
