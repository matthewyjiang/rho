//! Builders shared by the live and replay tests.

use std::collections::HashMap;

use rho_protocol::{
    MessageId, SessionEvent as WireEvent, ToolCallId, ToolCallUpdate, ToolContent, ToolKind,
    ToolStatus,
};
use rho_sdk::model;
use serde_json::json;

use crate::session::tree::{DisplayRowOrigin, NodeId};

/// Where display row `row` of the test session was saved.
pub(super) fn origin(row: usize) -> DisplayRowOrigin {
    DisplayRowOrigin {
        node: NodeId::from_string("node").unwrap(),
        row,
    }
}

/// A shell call of `command`; its card is titled by the command.
pub(super) fn shell_call(id: &str, command: &str) -> model::ToolCall {
    model::ToolCall::new(id.into(), "bash".into(), json!({ "command": command }))
}

/// The card [`shell_call`] opens with.
pub(super) fn opened(id: &str, command: &str) -> WireEvent {
    WireEvent::tool_call(ToolCallUpdate {
        id: ToolCallId::new(id),
        title: Some(command.to_owned()),
        kind: Some(ToolKind::Other),
        status: Some(ToolStatus::InProgress),
        raw_input: Some(json!({ "command": command })),
        content: None,
        locations: None,
        display: None,
    })
}

/// Call `id` moving to `status`, showing `content`.
pub(super) fn updated(id: &str, status: ToolStatus, content: Vec<ToolContent>) -> WireEvent {
    WireEvent::tool_call(ToolCallUpdate {
        status: Some(status),
        content: Some(content),
        ..closed_update(id)
    })
}

/// Call `id` closed `failed` without a result.
pub(super) fn closed(id: &str) -> WireEvent {
    WireEvent::tool_call(closed_update(id))
}

fn closed_update(id: &str) -> ToolCallUpdate {
    ToolCallUpdate {
        id: ToolCallId::new(id),
        title: None,
        kind: None,
        status: Some(ToolStatus::Failed),
        raw_input: None,
        content: None,
        locations: None,
        display: None,
    }
}

pub(super) fn text(text: &str) -> ToolContent {
    ToolContent::Text { text: text.into() }
}

pub(super) fn agent(message: &str, text: &str) -> WireEvent {
    WireEvent::AgentMessageChunk {
        message: MessageId::new(message),
        text: text.into(),
    }
}

/// `events` with message ids renamed `m0`, `m1`, ... by first appearance,
/// since live ids are random. Which chunks share a message is kept.
pub(super) fn canonical_messages(events: Vec<WireEvent>) -> Vec<WireEvent> {
    let mut names: HashMap<String, MessageId> = HashMap::new();
    let mut rename = |id: &mut MessageId| {
        let next = MessageId::new(format!("m{}", names.len()));
        *id = names.entry(id.as_str().to_owned()).or_insert(next).clone();
    };
    events
        .into_iter()
        .map(|mut event| {
            match &mut event {
                WireEvent::UserMessage { message, .. }
                | WireEvent::AgentMessageChunk { message, .. }
                | WireEvent::ThoughtChunk { message, .. } => rename(message),
                _ => {}
            }
            event
        })
        .collect()
}

/// `events` with each tool call's `display` cleared, for comparing the
/// other fields; `tool_calls_carry_the_tui_cards` covers the cards.
pub(super) fn without_display(events: Vec<WireEvent>) -> Vec<WireEvent> {
    events
        .into_iter()
        .map(|mut event| {
            if let WireEvent::ToolCall(update) = &mut event {
                update.display = None;
            }
            event
        })
        .collect()
}
