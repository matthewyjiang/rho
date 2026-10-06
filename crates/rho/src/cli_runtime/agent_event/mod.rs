//! Protocol-level events translated into protocol-neutral run artifacts.

pub(crate) mod render;
mod tool_cards;

use agent_client_protocol::schema::v1::{SessionUpdate, StopReason};

/// Protocol-level vocabulary for external agents, rendered into artifact-level
/// `StreamEffect`s. ACP delivers updates over stdio; an in-process runtime such
/// as Claude may construct the same variants later. Non-ACP extras can become
/// sibling variants when needed.
#[derive(Debug)]
pub(crate) enum AgentEvent {
    /// One ACP session update, verbatim. Boxed because `SessionUpdate` dwarfs
    /// the other variants; build it with `AgentEvent::from(update)`.
    Update(Box<SessionUpdate>),
    /// A prompt turn ended; the driver owns terminal classification.
    TurnEnded(StopReason),
    /// Fail-soft diagnostic for drift, decoding, or unsupported events.
    Notice(String),
}

impl From<SessionUpdate> for AgentEvent {
    fn from(update: SessionUpdate) -> Self {
        Self::Update(Box::new(update))
    }
}
