//! Answers the `initialize` requests the ACP SDK's protocol router would
//! misroute, until one `initialize` succeeds.
//!
//! With a v1 agent configured, `AgentProtocolRouter` (agent-client-protocol
//! 5.6.1) hands every first frame that doesn't select v2 to that agent,
//! and the connection stays v1 from then on. That is a documented choice:
//! "a request below v1 and a missing or malformed first request is handed
//! unchanged to the v1 implementation". The v2 negotiation rules bind
//! dual-stack agents too (migration guide, §Negotiate once). Two requests
//! break them:
//!
//! - `protocolVersion: 0` selects v1. It has to be `-32022`, because no
//!   version rho supports is at or below 0.
//! - A missing or malformed `protocolVersion` is refused with `-32602`,
//!   but the connection is now v1, so a corrected `initialize(2)` gets
//!   v1. A failed `initialize` has to leave the connection uninitialized.
//!
//! Both get the spec's answer here, and the router never sees them.
//! Everything else, `null` ids included, is the router's.
//!
//! TODO(upstream): delete this module once the router answers these itself
//! (rust-sdk issue "AgentProtocolRouter locks dual-stack agents to v1 on
//! `protocolVersion: 0` or a malformed `initialize`").

use std::sync::{Mutex, PoisonError};

use agent_client_protocol::{
    schema::ProtocolVersion, v2::schema as v2, Error as AcpError, RawJsonRpcId, RawJsonRpcMessage,
};
use serde::{de::IgnoredAny, Deserialize};
use serde_json::Value;

/// The screen over one connection's lines.
pub(super) struct InitializeScreen {
    /// The versions the router serves, which a `-32022` lists.
    supported: &'static [ProtocolVersion],
    negotiation: Mutex<Negotiation>,
}

/// How far the connection's version negotiation got.
enum Negotiation {
    /// No `initialize` has succeeded. These requests went to the router and
    /// are still unanswered or failed.
    Pending(Vec<RawJsonRpcId>),
    /// An `initialize` succeeded and the router owns every later frame.
    Done,
}

impl InitializeScreen {
    pub(super) fn new(supported: &'static [ProtocolVersion]) -> Self {
        Self {
            supported,
            negotiation: Mutex::new(Negotiation::Pending(Vec::new())),
        }
    }

    /// The reply to an incoming line the router must not see, which the
    /// line is then dropped for. Records the `initialize` requests the
    /// router does get, so [`Self::outgoing`] sees one succeed.
    pub(super) fn incoming(&self, line: &str) -> Option<String> {
        let mut negotiation = self.lock();
        let Negotiation::Pending(pending) = &mut *negotiation else {
            return None;
        };
        match screen(line, self.supported) {
            Screened::Forward => None,
            Screened::Initialize(id) => {
                pending.push(id);
                None
            }
            Screened::Answer(id, error) => Some(
                serde_json::to_string(&RawJsonRpcMessage::response(id, Err(error)))
                    .expect("a JSON-RPC error response always serializes"),
            ),
        }
    }

    /// Ends negotiation once outgoing `line` answers a pending `initialize`
    /// with a result.
    pub(super) fn outgoing(&self, line: &str) {
        let mut negotiation = self.lock();
        if let Negotiation::Pending(pending) = &*negotiation {
            if initialized(line, pending) {
                *negotiation = Negotiation::Done;
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Negotiation> {
        self.negotiation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// What to do with an incoming line before an `initialize` succeeded.
enum Screened {
    /// Not an `initialize` this screen judges: the router gets it.
    Forward,
    /// An `initialize` with a version the router selects correctly.
    Initialize(RawJsonRpcId),
    /// An `initialize` answered here with this error.
    Answer(RawJsonRpcId, AcpError),
}

/// Judges `line`, parsing it only as far as an `initialize` request's id
/// and `protocolVersion`.
fn screen(line: &str, supported: &[ProtocolVersion]) -> Screened {
    #[derive(Deserialize)]
    struct Request {
        id: RawJsonRpcId,
        method: String,
        #[serde(default)]
        params: Option<Value>,
    }
    let Ok(request) = serde_json::from_str::<Request>(line) else {
        return Screened::Forward;
    };
    if request.method != "initialize" || request.id == RawJsonRpcId::Null {
        return Screened::Forward;
    }
    let version = request
        .params
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|params| params.get("protocolVersion"))
        .and_then(|version| ProtocolVersion::deserialize(version).ok());
    match version {
        Some(version) if supported.iter().any(|served| *served <= version) => {
            Screened::Initialize(request.id)
        }
        Some(version) => Screened::Answer(
            request.id,
            v2::Error::unsupported_protocol_version(version, supported).into(),
        ),
        None => Screened::Answer(
            request.id,
            AcpError::invalid_params()
                .data("initialize.protocolVersion must be an integer from 0 to 65535"),
        ),
    }
}

/// Whether outgoing `line` answers one of the `pending` initialize requests
/// with a result.
fn initialized(line: &str, pending: &[RawJsonRpcId]) -> bool {
    #[derive(Deserialize)]
    struct Reply {
        id: RawJsonRpcId,
        result: Option<IgnoredAny>,
    }
    serde_json::from_str::<Reply>(line)
        .is_ok_and(|reply| reply.result.is_some() && pending.contains(&reply.id))
}

#[cfg(test)]
#[path = "initialize_screen_tests.rs"]
mod tests;
