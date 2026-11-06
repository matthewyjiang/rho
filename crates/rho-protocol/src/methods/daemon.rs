//! The connection handshake and the daemon's own status and stop.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ProtocolVersion;

/// Handshake. Must be the first request on every connection.
pub const HELLO: &str = "rho/hello";
/// Report the daemon's identity, uptime, and load.
pub const DAEMON_STATUS: &str = "rho/daemon/status";
/// Acknowledge, then shut the daemon down gracefully.
pub const DAEMON_STOP: &str = "rho/daemon/stop";

/// Who is connecting. Logged by the daemon; never used for authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClientInfo {
    /// Frontend name, for example `rho-cli` or `rho-ui`.
    pub name: String,
    /// Frontend version.
    pub version: String,
}

/// Params of [`HELLO`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelloParams {
    /// The protocol the client speaks.
    pub protocol: ProtocolVersion,
    pub client: ClientInfo,
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
    /// The protocol the daemon speaks. Its major equals the client's.
    pub protocol: ProtocolVersion,
    pub daemon: DaemonInfo,
    /// Optional features this daemon supports. Protocol minors add names
    /// here; protocol 1.0 defines none.
    pub capabilities: Vec<String>,
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
}

/// Params of [`DAEMON_STOP`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StopParams {}

/// Result of [`DAEMON_STOP`]: the daemon has begun shutting down. It finishes
/// in-flight responses, closes connections, interrupts running turns, and
/// exits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StopResponse {}
