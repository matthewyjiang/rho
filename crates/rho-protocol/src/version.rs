//! Protocol versioning and the handshake compatibility rule.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A protocol version. Minors add optional fields, methods, and error codes;
/// majors may change anything.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

/// The protocol this crate speaks.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(1, 0);

impl ProtocolVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Two sides can talk iff their majors match. Within a major, each side
    /// ignores fields it does not know and checks `capabilities` for
    /// optional features, so minors never block a connection.
    pub const fn is_compatible_with(self, other: Self) -> bool {
        self.major == other.major
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

/// Which side of an incompatible pair has to change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VersionFix {
    /// The running daemon is older than the client. Stopping it lets the next
    /// connection start the daemon from the newer installed binary.
    RestartDaemon,
    /// The client is older than the daemon. Update the client.
    UpdateClient,
}

/// An incompatible handshake. The daemon sends this as the `data` of a
/// [`crate::ErrorCode::VersionMismatch`] error; its `Display` is the
/// actionable message for users.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct VersionMismatch {
    pub daemon: ProtocolVersion,
    pub client: ProtocolVersion,
    pub fix: VersionFix,
}

impl VersionMismatch {
    /// The mismatch between a daemon and a client, or `None` when they are
    /// compatible.
    pub fn between(daemon: ProtocolVersion, client: ProtocolVersion) -> Option<Self> {
        if daemon.is_compatible_with(client) {
            return None;
        }
        let fix = if daemon.major < client.major {
            VersionFix::RestartDaemon
        } else {
            VersionFix::UpdateClient
        };
        Some(Self {
            daemon,
            client,
            fix,
        })
    }
}

impl fmt::Display for VersionMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            daemon,
            client,
            fix,
        } = self;
        write!(
            formatter,
            "rho daemon speaks protocol {daemon} but this client speaks {client}; "
        )?;
        match fix {
            VersionFix::RestartDaemon => formatter.write_str("run `rho daemon stop` and retry"),
            VersionFix::UpdateClient => formatter.write_str("update rho"),
        }
    }
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
