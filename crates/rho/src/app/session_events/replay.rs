//! Rebuild a client's view of a session from its saved display rows,
//! without running tools.

use std::{collections::HashMap, path::Path};

use rho_protocol::{MessageId, SessionEvent as WireEvent, ToolCallId, ToolCallUpdate, ToolStatus};
use rho_sdk::model::{self, Message, SemanticMessage, ToolResult};
use serde_json::Value;

use super::{
    cards, opened_call, plan_entries, result_content, status_update, user_content, CallIds,
    UNNAMED_TOOL_TITLE,
};
use crate::{
    app::session_runtime::SessionTools,
    display_transcript::DisplayTranscript,
    session::tree::{DisplayRowOrigin, ProjectedMessage},
    tools::mcp::exported_name::McpToolNames,
};

/// The wire events that replay `rows`, a saved session's display rows
/// (`SavedHistory::display_rows`), in order, ending with the session's
/// current plan when it has one.
///
/// Each saved message replays whole: one event per message, never chunks.
/// Message ids derive from the row a message was saved in and tool call ids
/// from the provider's saved ids, numbered by `ids`, which has counted the
/// rows before `rows`; so the same rows replay with the same ids every time.
/// A call the history left without a result, as a killed process does,
/// closes `failed`, since history is not running work.
pub(super) fn replay(
    rows: &[ProjectedMessage],
    workspace: &Path,
    tools: SessionTools<'_>,
    ids: &mut CallIds,
) -> Vec<WireEvent> {
    let mcp_tools = tools.mcp_report().tool_names();
    let mut replay = Replay {
        workspace,
        tools,
        mcp_tools: &mcp_tools,
        calls: HashMap::new(),
        opened: 0,
        ids,
        message_ids: MessageIds::default(),
        events: Vec::new(),
    };
    for row in rows {
        replay.message_ids.enter(row.origin.as_ref());
        replay.message(&row.message);
    }
    let Replay {
        calls, mut events, ..
    } = replay;
    let mut unfinished: Vec<_> = calls
        .into_values()
        .filter(|call| call.status == ToolStatus::InProgress)
        .collect();
    unfinished.sort_by_key(|call| call.opened_at);
    events.extend(unfinished.into_iter().map(|call| call.closed()));
    let plan = plan_entries(&tools.todo_state());
    if !plan.is_empty() {
        events.push(WireEvent::Plan { entries: plan });
    }
    events
}

/// The id replay gives the first message of the display row saved at
/// `origin`: a live user message saved there takes it too, so a client sees
/// one message whether it watched the prompt live or got it replayed.
pub(super) fn saved_message_id(origin: &DisplayRowOrigin) -> MessageId {
    let mut ids = MessageIds::default();
    ids.enter(Some(origin));
    ids.next()
}

struct Replay<'a> {
    /// Session working directory; titles show paths relative to it.
    workspace: &'a Path,
    /// The session's tools. Saved calls keep no metadata, so each tool
    /// reports what it would at start, as it does live.
    tools: SessionTools<'a>,
    /// `tools`' MCP names, computed once: they identify MCP calls, whose
    /// capped names cannot be decoded.
    mcp_tools: &'a McpToolNames,
    /// The latest call opened under each saved id, for its results.
    calls: HashMap<String, ReplayedCall>,
    /// How many calls replay opened.
    opened: usize,
    /// The session's call ids, through the rows replayed so far.
    ids: &'a mut CallIds,
    message_ids: MessageIds,
    events: Vec<WireEvent>,
}

/// A call replay opened, and what it shows so far.
struct ReplayedCall {
    id: ToolCallId,
    status: ToolStatus,
    content: Vec<rho_protocol::ToolContent>,
    /// How many calls replay opened before it, for closing calls in order.
    opened_at: usize,
}

impl ReplayedCall {
    fn closed(self) -> WireEvent {
        WireEvent::tool_call(status_update(self.id, Some(ToolStatus::Failed)))
    }
}

impl Replay<'_> {
    fn message(&mut self, message: &Message) {
        match message.semantic() {
            SemanticMessage::ToolResult(result) => self.result(result),
            SemanticMessage::User(blocks) | SemanticMessage::BoundaryInput(blocks) => {
                let content = user_content(blocks);
                if !content.is_empty() {
                    let message = self.message_ids.next();
                    self.events
                        .push(WireEvent::UserMessage { message, content });
                }
            }
            SemanticMessage::Assistant(blocks) => self.agent(blocks),
            SemanticMessage::EnrichedAssistant(assistant) => {
                // Reasoning streams before the text and calls it led to, so
                // it replays first too.
                if let Some(summary) = assistant
                    .reasoning_summary
                    .as_deref()
                    .filter(|summary| !summary.is_empty())
                {
                    let message = self.message_ids.next();
                    self.events.push(WireEvent::ThoughtChunk {
                        message,
                        text: summary.to_owned(),
                    });
                }
                self.agent(&assistant.content);
            }
            SemanticMessage::AbortedAssistant(assistant) => self.agent(&assistant.content),
            SemanticMessage::System(text) => {
                if let Some(transcript) = DisplayTranscript::from_display(text) {
                    self.events.extend(cards(&transcript));
                }
            }
            // Display rows never hold model-only entries.
            SemanticMessage::CompactionSummary(_)
            | SemanticMessage::ModelContext(_)
            | SemanticMessage::ReasoningShift(_) => {}
        }
    }

    /// An assistant message: its text, as one message per run of text
    /// between tool calls (live text after a call is a new message too), and
    /// its calls. The wire's agent messages carry text only, so images an
    /// assistant message holds are not replayed.
    fn agent(&mut self, blocks: &[model::ContentBlock]) {
        let mut text = String::new();
        for block in blocks {
            match block {
                model::ContentBlock::Text(part) => text.push_str(part),
                model::ContentBlock::Image(_) => {}
                model::ContentBlock::ToolCall(call) => {
                    self.agent_text(std::mem::take(&mut text));
                    self.call(call);
                }
            }
        }
        self.agent_text(text);
    }

    fn agent_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let message = self.message_ids.next();
        self.events
            .push(WireEvent::AgentMessageChunk { message, text });
    }

    fn call(&mut self, call: &model::ToolCall) {
        let metadata = self
            .tools
            .tool_named(&call.name)
            .map(|tool| tool.start_metadata(&call.arguments))
            .unwrap_or_default();
        let id = self.ids.open(&call.id);
        let opened = opened_call(
            id.clone(),
            &call.name,
            Some(call.arguments.clone()),
            &metadata,
            self.workspace,
            self.mcp_tools,
        );
        self.open(&call.id, id, opened);
    }

    /// A result updates the latest call opened under its id; results replace
    /// a call's content as a whole, so it keeps earlier output. Its raw
    /// output is the text the model got, as live. An orphan result still
    /// opens a complete card.
    fn result(&mut self, result: &ToolResult) {
        let status = if result.is_failure() {
            ToolStatus::Failed
        } else {
            ToolStatus::Completed
        };
        let content = result_content(&result.content, &result.images);
        let raw_output = Some(Value::String(result.content.clone()));
        if let Some(call) = self.calls.get_mut(&result.id) {
            call.status = status;
            call.content.extend(content);
            self.events.push(WireEvent::tool_call(ToolCallUpdate {
                content: Some(call.content.clone()),
                raw_output,
                ..status_update(call.id.clone(), Some(status))
            }));
            return;
        }
        let id = self.ids.open(&result.id);
        let orphan = ToolCallUpdate {
            title: Some(UNNAMED_TOOL_TITLE.to_owned()),
            kind: Some(rho_protocol::ToolKind::Other),
            content: Some(content),
            raw_output,
            ..status_update(id.clone(), Some(status))
        };
        self.open(&result.id, id, orphan);
    }

    /// Sends `opened`, the call `id` opened under `saved`, first closing a
    /// call still open under the same saved id, which it supersedes.
    fn open(&mut self, saved: &str, id: ToolCallId, opened: ToolCallUpdate) {
        let call = ReplayedCall {
            id,
            status: opened.status.unwrap_or(ToolStatus::InProgress),
            content: opened.content.clone().unwrap_or_default(),
            opened_at: self.opened,
        };
        self.opened += 1;
        if let Some(superseded) = self.calls.insert(saved.to_owned(), call) {
            if superseded.status == ToolStatus::InProgress {
                self.events.push(superseded.closed());
            }
        }
        self.events.push(WireEvent::tool_call(opened));
    }
}

/// Ids for replayed messages that stay the same each time a session loads.
///
/// A message is named after the display row it was saved in, so an id is
/// unique to one insertion, even of text saved twice. One saved row can
/// replay as several messages (text after a tool call, a reasoning
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

    /// The id of the next message the current saved row replays as.
    fn next(&mut self) -> MessageId {
        let id = MessageId::new(format!("saved-{}-{}", self.saved, self.part));
        self.part += 1;
        id
    }
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;
