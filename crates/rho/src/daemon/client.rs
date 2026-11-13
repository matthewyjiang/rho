//! Connecting to a running daemon.

use std::{io, path::PathBuf, time::Duration};

use super::{
    paths::DaemonPaths,
    platform::{self, Stream},
};
use rho_protocol::{Client, ClientError, ClientHello, ClientInfo, HelloResponse, Notifications};

/// How long a client waits, once the daemon began restarting in place, for
/// the restarted daemon to answer: `rho daemon restart`, and a client
/// attaching again after the daemon detached it as restarting.
//
// Tripwire. The restart first waits up to `DRAIN_TIMEOUT` (4 s, in
// `daemon_host::sessions`) from the request for the turns under way to save,
// as a stop does, then re-executes. Measured with the debug binary, all of
// `rho daemon restart` with no sessions, 150 runs each: 22 ms median (25 ms
// max) idle, and 78 ms median (306 ms max) with all 12 cores saturated.
// 10 s covers the drain and a cold page cache for the new binary.
pub(crate) const DAEMON_RESTART_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectError {
    /// Nothing is listening on the socket.
    #[error("rho daemon is not running")]
    NotRunning,
    #[error("could not connect to the rho daemon at {socket}: {source}")]
    Io { socket: PathBuf, source: io::Error },
    #[error(transparent)]
    Handshake(#[from] ClientError),
}

/// What a `rho` process connects to the daemon as, which decides what its
/// connection may do (#1780).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectAs {
    /// A person's frontend, which answers their sessions' approvals and
    /// questions: the terminal UI, `rho attach`, and `rho acp`.
    Frontend,
    /// A person's command that answers no session's requests, such as
    /// `rho daemon status`, `rho login`, and `rho run --detach`.
    Command,
    /// An agent's session control, `rho mcp serve` and the `sessions` tool,
    /// which the daemon refuses an answer or a hold from.
    Agent,
}

impl ConnectAs {
    /// The hello the `rho` CLI sends for this kind of connection.
    pub(crate) fn hello(self) -> ClientHello {
        let client = ClientInfo {
            name: "rho-cli".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        match self {
            Self::Frontend => ClientHello::frontend(client),
            Self::Command => ClientHello::command(client),
            Self::Agent => ClientHello::agent(client),
        }
    }
}

/// A connection to the daemon: requests go through its client, and the
/// sessions it attaches to send their events to its notifications.
pub(crate) struct Connection {
    pub(crate) client: Client,
    pub(crate) notifications: Notifications,
    pub(crate) hello: HelloResponse,
}

/// Connects to the running daemon and completes the handshake. Never starts
/// a daemon.
pub(crate) async fn connect(
    paths: &DaemonPaths,
    hello: ClientHello,
) -> Result<Connection, ConnectError> {
    handshake(paths, platform::connect(paths).await, hello).await
}

/// Connects to a daemon that began restarting in place, once the restarted
/// daemon answers, and completes the handshake. Bound it with
/// [`DAEMON_RESTART_TIMEOUT`]: where a restart closes the endpoint, this
/// waits until one listens again.
pub(crate) async fn connect_restarting(
    paths: &DaemonPaths,
    hello: ClientHello,
) -> Result<Connection, ConnectError> {
    handshake(paths, platform::connect_restarting(paths).await, hello).await
}

async fn handshake(
    paths: &DaemonPaths,
    connected: io::Result<Option<Stream>>,
    hello: ClientHello,
) -> Result<Connection, ConnectError> {
    let stream = match connected {
        Ok(Some(stream)) => stream,
        Ok(None) => return Err(ConnectError::NotRunning),
        Err(source) => {
            return Err(ConnectError::Io {
                socket: paths.socket.clone(),
                source,
            })
        }
    };
    let (client, notifications, hello) = Client::handshake(stream, hello).await?;
    Ok(Connection {
        client,
        notifications,
        hello,
    })
}
