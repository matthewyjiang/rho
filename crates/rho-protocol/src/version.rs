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
/// outside this repository has run against it (#1776). Until the beta the
/// wire changes without raising the minor: nothing is released, and the
/// version is set once then.
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
    /// client's version lets them talk; [`VersionMismatch::restart`] says
    /// what that costs.
    RestartDaemon,
    /// The client is older than the daemon. Update the client, or restart
    /// one that was updated while it ran.
    UpdateClient,
}

/// What restarting the daemon would cost, as of the moment the daemon
/// counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct RestartCost {
    /// How many things the restart would interrupt: jobs in progress
    /// (queued, running, or waiting for input), logins under way, and
    /// sessions with work of their own, such as a background process.
    pub in_progress: u64,
    /// How many other clients are connected, which the restart disconnects.
    /// They speak the daemon's protocol, so each needs updating or
    /// restarting before it can use a newer daemon.
    pub clients: u64,
}

impl RestartCost {
    /// Whether the restart would interrupt nothing and disconnect no one.
    pub const fn is_free(self) -> bool {
        self.in_progress == 0 && self.clients == 0
    }
}

/// What the restart interrupts and whom it disconnects, as a clause that
/// follows "which interrupts".
impl fmt::Display for RestartCost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            in_progress,
            clients,
        } = self;
        match in_progress {
            0 => formatter.write_str("nothing"),
            count => write!(
                formatter,
                "{count} in progress (jobs, logins, or background work)"
            ),
        }?;
        match clients {
            0 => Ok(()),
            1 => formatter.write_str(" and disconnects 1 other client"),
            count => write!(formatter, " and disconnects {count} other clients"),
        }
    }
}

/// An accepted handshake from a client of another build than the daemon's
/// ([`crate::BuildId`]). The two speak one protocol version, but their wires
/// may differ, so a request may fail. The daemon sends it as
/// [`crate::HelloResponse::skew`]; its `Display` is the notice for users.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct BuildSkew {
    /// What restarting the daemon, which replaces it with the client's
    /// build, would cost as the daemon accepted the client. That client is
    /// not one of the clients it counts.
    pub restart: RestartCost,
}

impl fmt::Display for BuildSkew {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "rho daemon runs a different build than this client, so requests may fail; \
             restart this client if it is the older one, or run `rho daemon restart` to \
             replace the daemon, which interrupts {}",
            self.restart
        )
    }
}

/// An incompatible handshake. The daemon sends this as the `data` of a
/// [`crate::ErrorCode::VersionMismatch`] error; its `Display` is the
/// actionable message for users.
///
/// Both sides must read this whatever their versions, so it only ever gains
/// optional fields, also while the protocol is unstable; CI holds it to
/// that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct VersionMismatch {
    pub daemon: ProtocolVersion,
    pub client: ProtocolVersion,
    pub fix: VersionFix,
    /// What restarting the daemon would cost. The daemon sends it with
    /// [`VersionFix::RestartDaemon`], so a client can say so before anyone
    /// restarts it, and can tell a daemon nobody is using.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<RestartCost>,
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
            restart: None,
        })
    }
}

impl fmt::Display for VersionMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            daemon,
            client,
            fix,
            restart,
        } = self;
        match fix {
            VersionFix::RestartDaemon => {
                write!(
                    formatter,
                    "rho daemon speaks protocol {daemon}, older than this client's {client}; \
                     run `rho daemon restart` to replace it, which interrupts "
                )?;
                match restart {
                    Some(restart) => write!(formatter, "{restart}"),
                    None => formatter.write_str("its work in progress"),
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
