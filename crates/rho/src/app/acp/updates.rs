//! A session's daemon events (`wire::SessionEvent`) as ACP session updates.
//!
//! The wire event mirrors ACP (`rho-protocol` `event.rs`), so most events
//! map field for field. [`Updates::map`] is pure apart from what the stream
//! itself decides: which tool calls the host already saw, so the first
//! report of a call opens its card and later ones patch it, and the IDs it
//! mints for messages and cards the daemon gives none.
//!
//! Events that are not transcript updates map to nothing here; the session
//! pump handles them (`sessions/pump.rs`): `TurnEnded` ends a prompt,
//! `State` tracks the attachment's job and Herdr, `Input*` open or close host
//! prompts (`inputs.rs`), and `OptionsChanged` is diffed against what the
//! host last saw ([`options_changed`]).

use std::collections::HashSet;

use agent_client_protocol::v1::schema::{
    AvailableCommandsUpdate, CompactionId, CompactionStatus, CompactionUpdate, ConfigOptionUpdate,
    ContentBlock, ContentChunk, Cost, CurrentModeUpdate, Diff, EmbeddedResource,
    EmbeddedResourceResource, ImageContent, MessageId, Notice, NoticeSeverity, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, ResourceLink, SessionId, SessionInfoUpdate,
    SessionNotification, SessionUpdate, TextResourceContents, ToolCall, ToolCallContent,
    ToolCallId, ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    UsageUpdate,
};
use rho_protocol::{self as wire, SessionEvent};
use serde_json::Value;

use super::{host_features::HostFeatures, options, permission};
use crate::permission::PermissionMode;

/// v1 diffs cannot tell a deleted file from an emptied one, so a deletion
/// says so in `_meta`; ACP v2's structured changes read it.
pub(super) const DELETED_FILE_META_KEY: &str = "rho/fileDeleted";

/// Hosts with their own transcript use this marker to avoid duplicating it.
const REPLAY_META_KEY: &str = "isReplay";

/// Shown when the provider's response was dropped and is being retried.
const RESPONSE_DISCARDED_NOTICE: &str = "[provider response discarded; retrying]";

/// A tool call the daemon reported without a title, until it names one.
const UNNAMED_TOOL_TITLE: &str = "tool";

/// One session's event mapping, kept for as long as its stream.
pub(super) struct Updates {
    features: HostFeatures,
    /// Tool calls the host already holds a card for.
    seen_tools: HashSet<String>,
    /// Prefix of the IDs this mapping mints, unique per process, so hosts
    /// that keep their transcript across reconnects never merge two.
    nonce: String,
    minted: u64,
}

impl Updates {
    pub(super) fn new(features: HostFeatures) -> Self {
        Self::with_nonce(features, uuid::Uuid::new_v4().to_string())
    }

    fn with_nonce(features: HostFeatures, nonce: String) -> Self {
        Self {
            features,
            seen_tools: HashSet::new(),
            nonce,
            minted: 0,
        }
    }

    /// The ACP updates `event` shows as.
    pub(super) fn map(&mut self, event: &SessionEvent) -> Vec<SessionUpdate> {
        match event {
            // A steer shows as the user message it applied as.
            SessionEvent::UserMessage {
                message,
                content,
                steer: _,
            } => content
                .iter()
                .filter_map(content_block)
                .map(|block| SessionUpdate::UserMessageChunk(chunk(block, message)))
                .collect(),
            SessionEvent::AgentMessageChunk { message, text } => {
                vec![SessionUpdate::AgentMessageChunk(chunk(
                    ContentBlock::from(text.as_str()),
                    message,
                ))]
            }
            SessionEvent::ThoughtChunk { message, text } => {
                vec![SessionUpdate::AgentThoughtChunk(chunk(
                    ContentBlock::from(text.as_str()),
                    message,
                ))]
            }
            SessionEvent::ToolCall(update) => self.tool_call(update).into_iter().collect(),
            SessionEvent::Plan { entries } => vec![SessionUpdate::Plan(Plan::new(
                entries.iter().map(plan_entry).collect(),
            ))],
            SessionEvent::Usage {
                used_tokens,
                window_tokens,
                cost_usd_micros,
            } => vec![usage(*used_tokens, *window_tokens, *cost_usd_micros)],
            SessionEvent::TitleChanged { title } => vec![SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().title(title.clone()),
            )],
            SessionEvent::Compaction { id, status } if self.features.compaction => {
                Vec::from_iter(compaction_update(id, status))
            }
            // A host without cards sees compaction only in `usage_update`.
            SessionEvent::Compaction { .. } => Vec::new(),
            // The retried response starts a message of its own.
            SessionEvent::ResponseDiscarded => {
                let id = self.mint("rho-msg");
                vec![SessionUpdate::AgentThoughtChunk(
                    ContentChunk::new(ContentBlock::from(RESPONSE_DISCARDED_NOTICE))
                        .message_id(MessageId::new(id)),
                )]
            }
            SessionEvent::Card {
                title,
                markdown,
                plain,
                tone,
                // The TUI's drawing of it; ACP clients draw their own.
                display: _,
            } => vec![self.card(title, markdown, plain, *tone)],
            SessionEvent::Notice { text } => vec![self.notice(text)],
            SessionEvent::State { .. }
            | SessionEvent::InputRequested { .. }
            | SessionEvent::InputResolved { .. }
            | SessionEvent::InputHeld { .. }
            | SessionEvent::TurnEnded { .. }
            | SessionEvent::OptionsChanged { .. }
            // The pump reports them, where the protocol has a place for them.
            | SessionEvent::Background { .. }
            // ACP has no update for what the turn is doing.
            | SessionEvent::ProviderRequest
            | SessionEvent::ProviderRetry { .. }
            // The steer shows once it applies, as a user message.
            | SessionEvent::SteeringDelivered { .. } => Vec::new(),
            // `Unknown`, or an event a newer daemon sends.
            _ => {
                tracing::debug!("skipping a session event this rho does not know");
                Vec::new()
            }
        }
    }

    /// Information for the user that is not part of the conversation: an ACP
    /// `notice` when the host shows them, else an agent message.
    fn notice(&mut self, text: &str) -> SessionUpdate {
        if self.features.notices {
            return SessionUpdate::Notice(Notice::new(NoticeSeverity::Info, text));
        }
        let id = self.mint("rho-msg");
        SessionUpdate::AgentMessageChunk(
            ContentChunk::new(ContentBlock::from(text)).message_id(MessageId::new(id)),
        )
    }

    /// The first report of a call opens its card; later ones patch only the
    /// fields they carry.
    /// Nothing for a patch that carries only a `display`, which ACP has no
    /// field for: a call's card follows its streamed arguments.
    fn tool_call(&mut self, update: &wire::ToolCallUpdate) -> Option<SessionUpdate> {
        let id = ToolCallId::new(update.id.as_str());
        let content = update
            .content
            .as_ref()
            .map(|content| content.iter().filter_map(tool_content).collect::<Vec<_>>());
        let locations = update
            .locations
            .as_ref()
            .map(|locations| locations.iter().map(location).collect::<Vec<_>>());
        if self.seen_tools.insert(update.id.as_str().to_owned()) {
            let title = update.title.as_deref().unwrap_or(UNNAMED_TOOL_TITLE);
            let mut call = ToolCall::new(id, title)
                .kind(update.kind.map_or(ToolKind::Other, tool_kind))
                .status(update.status.map_or(ToolCallStatus::Pending, tool_status))
                .raw_input(update.raw_input.clone());
            if let Some(content) = content {
                call = call.content(content);
            }
            if let Some(locations) = locations {
                call = call.locations(locations);
            }
            return Some(SessionUpdate::ToolCall(call));
        }
        if update.title.is_none()
            && update.kind.is_none()
            && update.status.is_none()
            && update.raw_input.is_none()
            && content.is_none()
            && locations.is_none()
        {
            return None;
        }
        let fields = ToolCallUpdateFields::new()
            .title(update.title.clone())
            .kind(update.kind.map(tool_kind))
            .status(update.status.map(tool_status))
            .raw_input(update.raw_input.clone())
            .content(content)
            .locations(locations);
        Some(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            id, fields,
        )))
    }

    /// A background result's card, as a finished tool call. Cards keep it out
    /// of the agent's message text, and hosts already render them beside the
    /// tool calls that started the work. Rows carry no identity of their own,
    /// so each card gets a fresh ID, which keeps hosts from merging two.
    fn card(
        &mut self,
        title: &str,
        markdown: &str,
        plain: &str,
        tone: wire::CardTone,
    ) -> SessionUpdate {
        let status = match tone {
            wire::CardTone::Error => ToolCallStatus::Failed,
            wire::CardTone::Neutral
            | wire::CardTone::Accent
            | wire::CardTone::Success
            | wire::CardTone::Warning => ToolCallStatus::Completed,
            // `Unknown`, or a tone a newer daemon sends.
            _ => ToolCallStatus::Completed,
        };
        let id = self.mint("rho-notice");
        let mut call = ToolCall::new(ToolCallId::new(id), title)
            .kind(ToolKind::Other)
            .status(status);
        // Content is markdown for hosts that render it. Hosts that show a
        // tool's output as is, such as T3 Code, read `rawOutput` first, so it
        // carries the plain form without code fences.
        if !plain.trim().is_empty() {
            call = call
                .content(vec![ToolCallContent::from(markdown)])
                .raw_output(Value::String(plain.to_owned()));
        }
        SessionUpdate::ToolCall(call)
    }

    fn mint(&mut self, kind: &str) -> String {
        self.minted += 1;
        format!("{kind}-{}-{}", self.nonce, self.minted)
    }
}

/// How an update reaches the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Delivery {
    /// As it happens.
    Live,
    /// As history a loaded session replays, marked so hosts with their own
    /// transcript skip it.
    Replay,
}

/// `update` for `session`, delivered as `delivery` says.
pub(super) fn notification(
    session: &SessionId,
    update: SessionUpdate,
    delivery: Delivery,
) -> SessionNotification {
    let notification = SessionNotification::new(session.clone(), update);
    match delivery {
        Delivery::Live => notification,
        Delivery::Replay => notification.meta(serde_json::Map::from_iter([(
            REPLAY_META_KEY.to_owned(),
            Value::Bool(true),
        )])),
    }
}

/// What the host must hear when the session's options move from `old` to
/// `new`: the whole config when any option changed, the mode when it did
/// (hosts learn of an agent-initiated switch only from this), and the
/// command menu when it changed. Nothing when nothing did.
pub(super) fn options_changed(
    old: &wire::SessionOptions,
    new: &wire::SessionOptions,
    features: HostFeatures,
) -> Vec<SessionUpdate> {
    let mut updates = Vec::new();
    let config = options::config_options(new, features.toggles);
    if config != options::config_options(old, features.toggles) {
        updates.push(SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(
            config,
        )));
    }
    if new.permission_mode.current != old.permission_mode.current {
        if let Ok(mode) = PermissionMode::try_from(new.permission_mode.current) {
            updates.push(SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(
                permission::mode_id(mode),
            )));
        }
    }
    if new.commands != old.commands {
        updates.push(commands_update(&new.commands));
    }
    updates
}

/// The ACP `compaction_update` card for compaction `id` reaching `status`,
/// for hosts that advertise `clientCapabilities.session.compaction`;
/// nothing for a status a newer daemon sends.
///
/// A card opens `in_progress` and ends with exactly one terminal update.
/// Its ID is the daemon's compaction ID, random per compaction: hosts such
/// as T3 Code keep their transcript across reconnects, so an ID that
/// restarted per process would patch an earlier card in place. Cards carry
/// no `summary`: ACP reserves it for retained summary content, not status
/// text such as the before and after counts, which hosts get from
/// `usage_update`.
fn compaction_update(
    id: &wire::CompactionId,
    status: &wire::CompactionStatus,
) -> Option<SessionUpdate> {
    let id = CompactionId::new(id.as_str());
    let update = match status {
        wire::CompactionStatus::Started { .. } => {
            CompactionUpdate::new(id, CompactionStatus::InProgress)
        }
        wire::CompactionStatus::Completed { .. } => {
            CompactionUpdate::new(id, CompactionStatus::Completed)
        }
        wire::CompactionStatus::Cancelled => CompactionUpdate::new(id, CompactionStatus::Cancelled),
        wire::CompactionStatus::Failed { error } => {
            CompactionUpdate::new(id, CompactionStatus::Failed).error(error.clone())
        }
        // `Unknown`, or a status a newer daemon sends.
        _ => return None,
    };
    Some(SessionUpdate::CompactionUpdate(update))
}

/// The session's whole command menu.
pub(super) fn commands_update(commands: &[wire::SessionCommand]) -> SessionUpdate {
    SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(
        options::available_commands(commands),
    ))
}

/// A text or other block of message `message`; every chunk Rho sends names
/// its message.
fn chunk(block: ContentBlock, message: &wire::MessageId) -> ContentChunk {
    ContentChunk::new(block).message_id(MessageId::new(message.as_str()))
}

fn content_block(block: &wire::ContentBlock) -> Option<ContentBlock> {
    Some(match block {
        wire::ContentBlock::Text { text } => ContentBlock::from(text.as_str()),
        wire::ContentBlock::Image { mime_type, data } => {
            ContentBlock::Image(ImageContent::new(data.as_str(), mime_type.as_str()))
        }
        wire::ContentBlock::Resource { uri, text } => ContentBlock::Resource(
            EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(
                TextResourceContents::new(text.as_str(), uri.as_str()),
            )),
        ),
        wire::ContentBlock::ResourceLink {
            uri,
            name,
            description,
        } => ContentBlock::ResourceLink(
            ResourceLink::new(name.as_str(), uri.as_str()).description(description.clone()),
        ),
        // Only a prompt carries context; a user message never holds it.
        wire::ContentBlock::Context { .. } => return None,
        // `Unknown`, or a block a newer daemon sends.
        _ => return None,
    })
}

fn tool_kind(kind: wire::ToolKind) -> ToolKind {
    match kind {
        wire::ToolKind::Read => ToolKind::Read,
        wire::ToolKind::Edit => ToolKind::Edit,
        wire::ToolKind::Delete => ToolKind::Delete,
        wire::ToolKind::Move => ToolKind::Move,
        wire::ToolKind::Search => ToolKind::Search,
        wire::ToolKind::Execute => ToolKind::Execute,
        wire::ToolKind::Think => ToolKind::Think,
        wire::ToolKind::Fetch => ToolKind::Fetch,
        wire::ToolKind::SwitchMode => ToolKind::SwitchMode,
        wire::ToolKind::Other => ToolKind::Other,
        // `Unknown`, or a kind a newer daemon sends.
        _ => ToolKind::Other,
    }
}

fn tool_status(status: wire::ToolStatus) -> ToolCallStatus {
    match status {
        wire::ToolStatus::Pending => ToolCallStatus::Pending,
        wire::ToolStatus::InProgress => ToolCallStatus::InProgress,
        wire::ToolStatus::Completed => ToolCallStatus::Completed,
        wire::ToolStatus::Failed => ToolCallStatus::Failed,
        // `Unknown`, or a status a newer daemon sends: still running.
        _ => ToolCallStatus::InProgress,
    }
}

fn tool_content(content: &wire::ToolContent) -> Option<ToolCallContent> {
    Some(match content {
        wire::ToolContent::Text { text } => ToolCallContent::from(text.as_str()),
        wire::ToolContent::Image { mime_type, data } => ToolCallContent::from(ContentBlock::Image(
            ImageContent::new(data.as_str(), mime_type.as_str()),
        )),
        wire::ToolContent::Diff {
            path,
            old_text,
            new_text,
            deleted,
        } => {
            let diff = Diff::new(path.clone(), new_text.as_str()).old_text(old_text.clone());
            ToolCallContent::Diff(if *deleted {
                diff.meta(serde_json::Map::from_iter([(
                    DELETED_FILE_META_KEY.to_owned(),
                    Value::Bool(true),
                )]))
            } else {
                diff
            })
        }
        // `Unknown`, or content a newer daemon sends.
        _ => return None,
    })
}

/// ACP counts `line` from 1 on v1 and v2 alike; the daemon's rows count from 0.
fn location(location: &wire::Location) -> ToolCallLocation {
    ToolCallLocation::new(location.path.clone())
        .line(location.line.map(|row| row.saturating_add(1)))
}

fn plan_entry(entry: &wire::PlanEntry) -> PlanEntry {
    let status = match entry.status {
        wire::PlanStatus::Pending => PlanEntryStatus::Pending,
        wire::PlanStatus::InProgress => PlanEntryStatus::InProgress,
        wire::PlanStatus::Completed => PlanEntryStatus::Completed,
        // `Unknown`, or a status a newer daemon sends.
        _ => PlanEntryStatus::Pending,
    };
    // The todo tool has no priorities; Medium is the neutral ACP value.
    PlanEntry::new(entry.content.clone(), PlanEntryPriority::Medium, status)
}

/// Context fill and cumulative cost. ACP's `size` is required; 0 means the
/// window is unknown, which clients render as no maximum. Cost is omitted,
/// never sent as 0, while no request has a price.
fn usage(used: u64, window: Option<u64>, cost_usd_micros: Option<u64>) -> SessionUpdate {
    let cost = cost_usd_micros.map(|micros| Cost::new(micros as f64 / 1_000_000.0, "USD"));
    SessionUpdate::UsageUpdate(UsageUpdate::new(used, window.unwrap_or(0)).cost(cost))
}

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;
