//! Protocol versioning and the handshake compatibility rule.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A protocol version. Major 0 is the unstable protocol: every minor may
/// change anything. From major 1 on, minors add optional fields, methods,
/// and error codes, and only majors may change anything.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

/// The protocol this crate speaks. It stays at major 0 until a frontend
/// outside this repository has run against it (#1776), so every change to
/// the wire raises the minor, and CI holds each change to that.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(0, 1);

impl ProtocolVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Two sides can talk iff their majors match and, in major 0, their
    /// minors do too: an unstable minor may change anything. From major 1
    /// on, each side ignores fields it does not know and checks
    /// `capabilities` for optional features, so minors never block a
    /// connection.
    pub const fn is_compatible_with(self, other: Self) -> bool {
        self.major == other.major && (self.major > 0 || self.minor == other.minor)
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
    /// The running daemon is older than the client. Replacing it with the
    /// client's version lets them talk; [`VersionMismatch::in_progress`]
    /// says what that interrupts.
    RestartDaemon,
    /// The client is older than the daemon. Update the client, or restart
    /// one that was updated while it ran.
    UpdateClient,
}

/// An incompatible handshake. The daemon sends this as the `data` of a
/// [`crate::ErrorCode::VersionMismatch`] error; its `Display` is the
/// actionable message for users.
///
/// Both sides must read this whatever their versions, so it only ever gains
/// optional fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct VersionMismatch {
    pub daemon: ProtocolVersion,
    pub client: ProtocolVersion,
    pub fix: VersionFix,
    /// How many things restarting the daemon would interrupt: jobs in
    /// progress (queued, running, or waiting for input), logins under way,
    /// and sessions with work of their own, such as a background process.
    /// The daemon sends it with [`VersionFix::RestartDaemon`], so a client
    /// can say what the restart costs before anyone runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_progress: Option<u64>,
}

impl VersionMismatch {
    /// The mismatch between a daemon and a client, or `None` when they are
    /// compatible. The older side is the one to change.
    pub fn between(daemon: ProtocolVersion, client: ProtocolVersion) -> Option<Self> {
        if daemon.is_compatible_with(client) {
            return None;
        }
        let fix = if daemon < client {
            VersionFix::RestartDaemon
        } else {
            VersionFix::UpdateClient
        };
        Some(Self {
            daemon,
            client,
            fix,
            in_progress: None,
        })
    }
}

impl fmt::Display for VersionMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            daemon,
            client,
            fix,
            in_progress,
        } = self;
        match fix {
            VersionFix::RestartDaemon => {
                write!(
                    formatter,
                    "rho daemon speaks protocol {daemon}, older than this client's {client}; \
                     run `rho daemon restart` to replace it, which interrupts "
                )?;
                match in_progress {
                    Some(0) => formatter.write_str("nothing"),
                    Some(1) => formatter.write_str("1 job in progress"),
                    Some(count) => write!(formatter, "{count} jobs in progress"),
                    None => formatter.write_str("the jobs it has in progress"),
                }
            }
            VersionFix::UpdateClient => write!(
                formatter,
                "rho daemon speaks protocol {daemon}, newer than this client's {client}; \
                 update this client, or restart it if it was updated while running"
            ),
        }
    }
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
