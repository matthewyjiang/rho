//! Session events: what an attached client sees of a session (#1657).
//!
//! The daemon sends each one in a [`super::SESSION_EVENT`] notification. They
//! mirror ACP's `session/update`, so an adapter maps them field for field.
//! Enums that a protocol minor may grow read unknown values as `Unknown`.

use std::{fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{InputRequest, InputRequestId, JobState, Resolution};

/// Declares a transparent string id with `new`, `as_str`, and `Display`,
/// like [`super::SessionId`].
macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_id! {
    /// Identifies one message in a session's event stream: chunks with the
    /// same id extend one message.
    MessageId
}

string_id! {
    /// Identifies one tool call; every [`ToolCallUpdate`] with the same id
    /// updates that call.
    ToolCallId
}

string_id! {
    /// Identifies one context compaction; every [`SessionEvent::Compaction`]
    /// with the same id reports on it.
    CompactionId
}

string_id! {
    /// One loaded session's event stream. A new daemon, or a reload, starts
    /// a new stream.
    StreamId
}

/// Position in a stream: the number of live events sent before and
/// including this one, from 1. `Seq(0)` is not a position: a reset's
/// transcript events carry it (see [`super::SessionEventParams::seq`]), and
/// a cursor at 0 always resets.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct Seq(pub u64);

/// Where a client is in a session's stream; send it back to resume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cursor {
    pub stream: StreamId,
    pub seq: Seq,
}

/// One thing that happened in a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionEvent {
    /// The session's job changed state: a turn started (`running`), waits
    /// for input, or ended. An ended turn's [`Self::TurnEnded`] comes first.
    State { state: JobState },
    /// A user message, whole.
    UserMessage {
        message: MessageId,
        content: Vec<ContentBlock>,
    },
    /// More of the agent's reply.
    AgentMessageChunk { message: MessageId, text: String },
    /// More of the agent's reasoning.
    ThoughtChunk { message: MessageId, text: String },
    /// Opens or updates a tool call; absent fields are unchanged. Boxed:
    /// it is the largest event by far.
    ToolCall(Box<ToolCallUpdate>),
    /// The agent's plan, replacing any earlier one.
    Plan { entries: Vec<PlanEntry> },
    /// How full the model's context is, and what the session has cost.
    Usage {
        used_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window_tokens: Option<u64>,
        /// The session's whole spend so far in millionths of a US dollar,
        /// so it stays exact, turns before a daemon restart included.
        /// Absent while no request has a price, as when every request ran
        /// on a local model (#1670).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd_micros: Option<u64>,
    },
    /// The session has a new title, as when the daemon named it after its
    /// first turn (#1670) or a client renamed it
    /// ([`super::SESSION_RENAME`]).
    TitleChanged { title: String },
    /// A context compaction began or ended. Each one starts
    /// [`CompactionStatus::Started`] and ends with exactly one other
    /// status, under the same `id` (#1670).
    Compaction {
        id: CompactionId,
        status: CompactionStatus,
    },
    /// The provider discarded the response it was streaming, and the run
    /// asks for it again. What was sent of it cannot be taken back; the
    /// retried response starts new messages (#1670).
    ResponseDiscarded,
    /// A titled card the session shows outside the conversation, such as a
    /// background result.
    Card {
        title: String,
        /// The body, for clients that render markdown.
        markdown: String,
        /// The same body as plain text, for clients that show text as is.
        plain: String,
        tone: CardTone,
    },
    /// Something to tell the user that is not part of the conversation.
    Notice { text: String },
    /// The turn waits for a person to answer `request` with
    /// [`super::SESSION_ANSWER`] (#1660).
    InputRequested { request: Box<InputRequest> },
    /// Input request `id` closed: a client answered it, the turn withdrew
    /// it, or it timed out. Clients that show it stop.
    InputResolved {
        id: InputRequestId,
        resolution: Resolution,
    },
    /// Input request `id`'s timeout stopped: a client held it with
    /// [`super::SESSION_HOLD`], so it waits for an answer.
    InputHeld { id: InputRequestId },
    /// The turn ended, and how (#1669).
    TurnEnded { outcome: super::TurnOutcome },
    /// The session's options changed, as after [`super::SESSION_CONFIGURE`]
    /// or a plan approval's mode switch: all of them, replacing the last.
    OptionsChanged { options: super::SessionOptions },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl SessionEvent {
    /// A [`Self::ToolCall`] carrying `update`.
    pub fn tool_call(update: ToolCallUpdate) -> Self {
        Self::ToolCall(Box::new(update))
    }
}

/// Opens a tool call, or updates the one with the same `id`. Absent fields
/// are unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallUpdate {
    pub id: ToolCallId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ToolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolStatus>,
    /// The tool's input, as the model wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<serde_json::Value>,
    /// Replaces the call's content when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ToolContent>>,
    /// Files the call touches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<Location>>,
}

/// What a tool does, so a client can pick an icon or a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Where a tool call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One piece of what a tool call shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolContent {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        /// Base64.
        data: String,
    },
    /// A file change; `old_text` is absent for a new file.
    Diff {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_text: Option<String>,
        new_text: String,
    },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One piece of a prompt or a user message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        /// Base64.
        data: String,
    },
    /// A text resource embedded whole, such as a file the user attached.
    Resource {
        uri: String,
        text: String,
    },
    /// A resource the user points at without its contents.
    ResourceLink {
        uri: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// A block a newer peer sends that this crate does not know; nothing
    /// sends it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Where a compaction is. Every status but `started` ends it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionStatus {
    Started,
    Completed,
    /// The turn was cancelled before the compaction finished.
    Cancelled,
    /// It failed, or the run ended before it reported a result.
    Failed {
        error: String,
    },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How a [`SessionEvent::Card`] reads, so a client can color it or show a
/// card that reports an error as failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CardTone {
    Neutral,
    Accent,
    Success,
    Warning,
    Error,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A file a tool call touches, and the line when one applies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Location {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// One step of the agent's plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
}

/// Where a plan step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;
