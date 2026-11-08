//! Turns one session's runtime events and saved transcript into the wire
//! events the daemon streams to attached clients ([`WireEvent`]).
//!
//! [`LiveEvents`] maps a loaded session's live runtime events as a turn
//! emits them; [`replay`] maps the display rows a client that attaches with
//! a reset receives first. Both open a tool call with the same card (title,
//! kind, raw input, locations) and close calls left unfinished as `failed`,
//! so a conversation reads the same whether a client watched it live or
//! received it replayed, apart from how its text was chunked. Only live
//! turns report usage, compactions, and discarded responses; saved rows
//! hold none, and the daemon tells titles itself.
//!
//! [`outcome`] says how a turn ended, for the daemon's clients and
//! `rho run` alike.
//!
//! Ported from the ACP `EventMapper`; the ACP adapter maps these wire events
//! instead, and drops its own mapper, in #1670.

use std::{collections::HashMap, path::Path};

use rho_protocol::{
    CardTone, ContentBlock, Location, PlanEntry, PlanStatus, SessionEvent as WireEvent, ToolCallId,
    ToolCallUpdate, ToolContent, ToolStatus,
};
use rho_sdk::{model, tool::ToolMetadata};
use rho_tools::tool_card::ToolHeader;
use serde_json::Value;

use crate::{
    app::interactive_presenter::call_header,
    display_transcript::{DisplayRow, DisplayTranscript},
    presentation::NotificationTone,
    tools::{
        mcp::exported_name::McpToolNames,
        todo::{TodoState, TodoStatus},
    },
};

mod live;
pub(crate) mod outcome;
mod replay;
#[cfg(test)]
mod test_support;
mod tools;

pub(crate) use live::LiveEvents;

/// Hands out wire ids for tool calls. Providers may reuse a call id across
/// turns, and a reused wire id would update the earlier card, so each call
/// opened under a provider id after its first is numbered (`id#2`, ...). A
/// call without an id is numbered by how many such calls came before.
#[derive(Clone, Debug, Default)]
struct CallIds {
    /// How many calls each provider id has opened.
    opened: HashMap<String, usize>,
}

impl CallIds {
    /// The wire id of the next call opened under provider id `provider`.
    fn open(&mut self, provider: &str) -> ToolCallId {
        let count = self.opened.entry(provider.to_owned()).or_default();
        *count += 1;
        match (provider.is_empty(), *count) {
            (false, 1) => ToolCallId::new(provider),
            (false, n) => ToolCallId::new(format!("{provider}#{n}")),
            (true, n) => ToolCallId::new(format!("unnamed-call-{n}")),
        }
    }

    /// Counts every id at least as often as `other` does, so no id either
    /// of them handed out is handed out again.
    fn cover(&mut self, other: &Self) {
        for (provider, &count) in &other.opened {
            let own = self.opened.entry(provider.clone()).or_default();
            *own = (*own).max(count);
        }
    }
}

/// Title for a tool call whose name the mapper never saw.
const UNNAMED_TOOL_TITLE: &str = "tool";

/// A tool call as it opens, live or replayed: its title, kind, arguments
/// and the files it names, `in_progress`. `metadata` is what the call's tool
/// reported at start; replay asks the session's tool for it, since saved
/// calls keep none.
fn opened_call(
    id: ToolCallId,
    name: &str,
    arguments: Option<Value>,
    metadata: &ToolMetadata,
    cwd: &Path,
    mcp_tools: &McpToolNames,
) -> ToolCallUpdate {
    let locations = tools::started_locations(name, arguments.as_ref(), metadata, cwd);
    ToolCallUpdate {
        status: Some(ToolStatus::InProgress),
        locations: non_empty(locations),
        ..named_call(id, name, arguments, metadata, cwd, mcp_tools)
    }
}

/// A call's identity without a status: its title, kind and arguments.
fn named_call(
    id: ToolCallId,
    name: &str,
    arguments: Option<Value>,
    metadata: &ToolMetadata,
    cwd: &Path,
    mcp_tools: &McpToolNames,
) -> ToolCallUpdate {
    ToolCallUpdate {
        title: Some(tool_title(
            name,
            arguments.as_ref(),
            metadata,
            cwd,
            mcp_tools,
        )),
        kind: Some(tools::kind(name, arguments.as_ref(), metadata)),
        raw_input: arguments,
        ..status_update(id, /*status*/ None)
    }
}

/// An update that changes at most a call's status; every other field is
/// absent, so it stays as the client has it.
fn status_update(id: ToolCallId, status: Option<ToolStatus>) -> ToolCallUpdate {
    ToolCallUpdate {
        id,
        title: None,
        kind: None,
        status,
        raw_input: None,
        content: None,
        locations: None,
    }
}

/// What a call does, as the interactive card header says it. The title
/// names the call, never a live phase:
/// - a shell call is its bare command: the call's `command_line` metadata,
///   else the command read from its arguments;
/// - a status-first header keeps its identity and drops the phase;
/// - an MCP call leads with its server.
///
/// A replayed call whose tool the session no longer has gets no metadata,
/// so a tool that describes itself through `command_summary` is titled by
/// name alone.
fn tool_title(
    name: &str,
    arguments: Option<&Value>,
    metadata: &ToolMetadata,
    cwd: &Path,
    mcp_tools: &McpToolNames,
) -> String {
    if let Some(command) = metadata.command_line_text().filter(|c| !c.is_empty()) {
        return command.to_owned();
    }
    let title = match call_header(name, arguments, metadata, cwd, mcp_tools) {
        ToolHeader::Shell {
            command: Some(command),
            ..
        } => command,
        // A shell call with no command yet; `$` alone would read as one.
        ToolHeader::Shell { command: None, .. } => name.to_owned(),
        ToolHeader::StatusFirst { identity, .. } => identity,
        ToolHeader::Call {
            verb,
            primary: Some(primary),
        } if !primary.is_empty() => format!("{verb}({primary})"),
        ToolHeader::Call { verb, .. } => verb,
    };
    match mcp_tools.identify(name) {
        Some(identity) => format!("{}/{title}", identity.server),
        None => title,
    }
}

/// A user message's content. Empty text says nothing, and a user message
/// never holds tool calls.
fn user_content(blocks: &[model::ContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .filter_map(|block| match block {
            model::ContentBlock::Text(text) if !text.is_empty() => {
                Some(ContentBlock::Text { text: text.clone() })
            }
            model::ContentBlock::Image(image) => Some(ContentBlock::Image {
                mime_type: image.mime_type.clone(),
                data: image.data.clone(),
            }),
            model::ContentBlock::Text(_) | model::ContentBlock::ToolCall(_) => None,
        })
        .collect()
}

/// A tool result's content: its text, unless empty, then its images. Live
/// and saved results both start with this, so a replayed call shows what
/// the live one did.
fn result_content(text: &str, images: &[model::ImageContent]) -> Vec<ToolContent> {
    let text = (!text.is_empty()).then(|| text_content(text));
    text.into_iter()
        .chain(images.iter().map(|image| ToolContent::Image {
            mime_type: image.mime_type.clone(),
            data: image.data.clone(),
        }))
        .collect()
}

fn text_content(text: impl Into<String>) -> ToolContent {
    ToolContent::Text { text: text.into() }
}

/// One card per row of a background result's transcript: the rows a turn
/// shows where it applied the result, and saves for replay.
fn cards(transcript: &DisplayTranscript) -> impl Iterator<Item = WireEvent> + '_ {
    transcript.0.iter().map(|row| {
        let text = row.card_text();
        WireEvent::Card {
            title: text.title.to_owned(),
            markdown: text.markdown,
            plain: text.plain,
            tone: card_tone(row),
        }
    })
}

/// How a row's card reads: a notification card's tone; a notice is neutral.
fn card_tone(row: &DisplayRow) -> CardTone {
    match row {
        DisplayRow::Notification(card) => match card.tone {
            NotificationTone::Neutral => CardTone::Neutral,
            NotificationTone::Accent => CardTone::Accent,
            NotificationTone::Success => CardTone::Success,
            NotificationTone::Warning => CardTone::Warning,
            NotificationTone::Error => CardTone::Error,
        },
        DisplayRow::Notice(_) => CardTone::Neutral,
    }
}

/// The session's todo checklist as a plan; empty while there is none.
fn plan_entries(todo: &TodoState) -> Vec<PlanEntry> {
    let Some(list) = todo.list() else {
        return Vec::new();
    };
    list.todos
        .iter()
        .map(|item| PlanEntry {
            content: item.content.clone(),
            status: match item.status {
                TodoStatus::Pending => PlanStatus::Pending,
                TodoStatus::InProgress => PlanStatus::InProgress,
                TodoStatus::Completed => PlanStatus::Completed,
            },
        })
        .collect()
}

/// `locations`, or `None` when there are none, so the wire leaves the
/// field out.
fn non_empty(locations: Vec<Location>) -> Option<Vec<Location>> {
    Some(locations).filter(|locations| !locations.is_empty())
}
