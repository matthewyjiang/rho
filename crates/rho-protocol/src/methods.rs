//! Method names and their params and result types.
//!
//! Names are `rho/<area>/<verb>`, except the connection-level `rho/hello`.
//! Params and results ignore unknown fields so a protocol minor can add
//! optional fields without breaking older peers.

use std::path::PathBuf;

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::ProtocolVersion;

/// Handshake. Must be the first request on every connection.
pub const HELLO: &str = "rho/hello";
/// Report the daemon's identity, uptime, and load.
pub const DAEMON_STATUS: &str = "rho/daemon/status";
/// Acknowledge, then shut the daemon down gracefully.
pub const DAEMON_STOP: &str = "rho/daemon/stop";

/// A request method with typed params and result.
///
/// Implemented by zero-sized markers so [`crate::Client::request`] can check
/// a call's types at compile time. Implementors pair `NAME` with the params
/// and result types the daemon uses for that method.
pub trait Method {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned;
    type Result: Serialize + DeserializeOwned;
}

/// Marker for [`HELLO`].
pub struct Hello;

impl Method for Hello {
    const NAME: &'static str = HELLO;
    type Params = HelloParams;
    type Result = HelloResponse;
}

/// Marker for [`DAEMON_STATUS`].
pub struct Status;

impl Method for Status {
    const NAME: &'static str = DAEMON_STATUS;
    type Params = StatusParams;
    type Result = DaemonStatus;
}

/// Marker for [`DAEMON_STOP`].
pub struct Stop;

impl Method for Stop {
    const NAME: &'static str = DAEMON_STOP;
    type Params = StopParams;
    type Result = StopResponse;
}

/// Who is connecting. Logged by the daemon; never used for authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Frontend name, for example `rho-cli` or `rho-ui`.
    pub name: String,
    /// Frontend version.
    pub version: String,
}

/// Params of [`HELLO`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloParams {
    /// The protocol the client speaks.
    pub protocol: ProtocolVersion,
    pub client: ClientInfo,
}

/// The running daemon process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// The daemon's rho version.
    pub version: String,
    pub pid: u32,
}

/// Result of [`HELLO`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResponse {
    /// The protocol the daemon speaks. Its major equals the client's.
    pub protocol: ProtocolVersion,
    pub daemon: DaemonInfo,
    /// Optional features this daemon supports. Protocol minors add names
    /// here; protocol 1.0 defines none.
    pub capabilities: Vec<String>,
}

/// Params of [`DAEMON_STATUS`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusParams {}

/// Result of [`DAEMON_STATUS`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub pid: u32,
    /// The daemon's rho version.
    pub version: String,
    pub protocol: ProtocolVersion,
    /// Unix time, in seconds, when the daemon started.
    pub started_at: u64,
    pub uptime_secs: u64,
    /// Agent jobs in progress.
    pub jobs: u64,
    /// The socket the daemon listens on.
    pub socket: PathBuf,
}

/// Params of [`DAEMON_STOP`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopParams {}

/// Result of [`DAEMON_STOP`]: the daemon has begun shutting down. It finishes
/// in-flight responses, closes connections, and exits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopResponse {}
