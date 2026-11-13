//! ACP steering: deliver a parent message into the turn that is running.
//!
//! An agent opts in with `InitializeResponse._meta.steering.supported`
//! (the `_session/steering` extension, as claude-agent-acp serves it). Rho
//! asks with `idleBehavior: "promptRequired"`, so the agent either injects
//! the message into the turn in flight or declines because no turn runs.
//! Anything but `injected`, including an error, falls back to Rho's
//! next-turn queue, so a message is never silently lost.

use agent_client_protocol::{
    v1::schema::{Meta, SessionId},
    Error, UntypedMessage,
};
use serde_json::{json, Value};

pub(super) const METHOD: &str = "_session/steering";

/// Whether `initialize`'s `_meta` advertises steering.
pub(super) fn supported(meta: Option<&Meta>) -> bool {
    meta.and_then(|meta| meta.get("steering"))
        .and_then(|steering| steering.get("supported"))
        .and_then(Value::as_bool)
        == Some(true)
}

pub(super) fn request(session_id: &SessionId, text: &str) -> Result<UntypedMessage, Error> {
    UntypedMessage::new(
        METHOD,
        json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": text}],
            "_meta": {"steering": {"idleBehavior": "promptRequired"}},
        }),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Delivery {
    /// Part of the running turn.
    Injected,
    /// Not delivered; send it as the next turn.
    NextTurn,
}

/// `promptRequired`, the legacy `startedNewTurn`, and unknown outcomes all
/// mean the running turn did not take the message.
pub(super) fn delivery(reply: &Value) -> Delivery {
    match reply.get("outcome").and_then(Value::as_str) {
        Some("injected") => Delivery::Injected,
        _ => Delivery::NextTurn,
    }
}
