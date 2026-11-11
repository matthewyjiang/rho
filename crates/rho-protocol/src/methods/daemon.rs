//! The connection handshake and the daemon's own status, stop, and restart.

use std::{collections::BTreeSet, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CredentialStoreStatus, ProtocolVersion};

/// Handshake. Must be the first request on every connection.
pub const HELLO: &str = "rho/hello";
/// Report the daemon's identity, uptime, and load.
pub const DAEMON_STATUS: &str = "rho/daemon/status";
/// Acknowledge, then shut the daemon down gracefully.
pub const DAEMON_STOP: &str = "rho/daemon/stop";
/// Acknowledge, then re-execute the daemon in place, interrupting the turns
/// under way.
pub const DAEMON_RESTART: &str = "rho/daemon/restart";

/// Who is connecting. Logged by the daemon; never used for authorization,
/// which goes by [`ClientRole`] and [`ClientCapability`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClientInfo {
    /// Frontend name, for example `rho-cli` or `rho-ui`.
    pub name: String,
    /// Frontend version.
    pub version: String,
}

/// What a connection is for, which bounds the capabilities it may declare.
/// The daemon reads it, so it has no `Unknown`: a role this daemon does not
/// know is [`crate::ErrorCode::InvalidParams`].
///
/// A role is declared, not proven: the socket is private to the daemon's
/// user, and any process of that user may declare any role. It keeps a
/// well-behaved client to its part, so an agent's session-control client
/// cannot answer the approvals of the sessions it drives; it is not a
/// sandbox for a process that can open the socket itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientRole {
    /// A person's frontend, such as the terminal UI, `rho attach`, an ACP
    /// host's `rho acp`, or a `rho` command. It may declare any capability.
    Frontend,
    /// An agent's session control: `rho mcp serve` and the `sessions` tool,
    /// which start, prompt, and watch sessions for a model. It may not
    /// declare [`ClientCapability::Answer`]: a turn that asks waits for a
    /// person.
    Agent,
}

/// Something a connection declares it does, which the daemon then lets it
/// do. A method that needs a capability the connection did not declare is
/// refused with [`crate::ErrorCode::CapabilityRequired`]. The daemon reads
/// it, so it has no `Unknown`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ClientCapability {
    /// Answer a session's open requests, its approvals and questions, and
    /// hold their timeouts: [`super::SESSION_ANSWER`] and
    /// [`super::SESSION_HOLD`].
    Answer,
}

/// What a client says of itself in [`HELLO`]: who it is, what it is for,
/// and what it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientHello {
    pub client: ClientInfo,
    pub role: ClientRole,
    pub capabilities: BTreeSet<ClientCapability>,
}

impl ClientHello {
    /// A person's frontend that answers their sessions' requests.
    pub fn frontend(client: ClientInfo) -> Self {
        Self {
            client,
            role: ClientRole::Frontend,
            capabilities: BTreeSet::from([ClientCapability::Answer]),
        }
    }

    /// A person's command that answers no session's requests, such as
    /// `rho daemon status`.
    pub fn command(client: ClientInfo) -> Self {
        Self {
            client,
            role: ClientRole::Frontend,
            capabilities: BTreeSet::new(),
        }
    }

    /// An agent's session control.
    pub fn agent(client: ClientInfo) -> Self {
        Self {
            client,
            role: ClientRole::Agent,
            capabilities: BTreeSet::new(),
        }
    }
}

/// Params of [`HELLO`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelloParams {
    /// The protocol the client speaks.
    pub protocol: ProtocolVersion,
    pub client: ClientInfo,
    /// What the connection is for. It holds for the connection's life.
    pub role: ClientRole,
    /// What the connection does, each of which its role must allow; one it
    /// does not is [`crate::ErrorCode::InvalidParams`], and the hello is
    /// refused.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub capabilities: BTreeSet<ClientCapability>,
}

/// The running daemon process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonInfo {
    /// The daemon's rho version.
    pub version: String,
    pub pid: u32,
}

/// Result of [`HELLO`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelloResponse {
    /// The protocol the daemon speaks, which is compatible with the
    /// client's.
    pub protocol: ProtocolVersion,
    pub daemon: DaemonInfo,
}

/// Params of [`DAEMON_STATUS`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusParams {}

/// Result of [`DAEMON_STATUS`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonStatus {
    pub pid: u32,
    /// The daemon's rho version.
    pub version: String,
    pub protocol: ProtocolVersion,
    /// Unix time, in seconds, when the daemon started.
    pub started_at: u64,
    pub uptime_secs: u64,
    /// Jobs with a turn in progress: [`crate::JobState::Running`] or
    /// [`crate::JobState::WaitingForInput`].
    pub jobs: u64,
    /// The socket the daemon listens on.
    pub socket: PathBuf,
    /// The credential store the daemon keeps provider credentials in, what
    /// chose it, and whether the daemon can reach it.
    pub credential_store: CredentialStoreStatus,
    /// Set once the daemon's executable was replaced: the daemon re-executes
    /// the new one in place as soon as nothing would be cut short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<PendingUpgrade>,
}

/// A replaced executable the daemon re-executes once it is idle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PendingUpgrade {
    /// The rho version the replacement reports.
    pub version: String,
    /// How many things the upgrade waits for: jobs in progress (queued,
    /// running, or waiting for input), logins under way, and sessions with
    /// work of their own, such as a background process.
    pub waiting_for: u64,
}

/// Params of [`DAEMON_STOP`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StopParams {}

/// Result of [`DAEMON_STOP`]: the daemon has begun shutting down. It finishes
/// in-flight responses, closes connections, interrupts running turns, and
/// exits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StopResponse {}

/// Params of [`DAEMON_RESTART`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestartParams {}

/// Result of [`DAEMON_RESTART`]: the daemon has begun restarting. It detaches
/// every attached client as [`crate::DetachReason::Restarting`], finishes
/// in-flight responses, closes connections, records the turns it interrupts
/// as interrupted, and re-executes its executable as the same process. A
/// client that connects meanwhile waits until the restarted daemon answers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestartResponse {}
