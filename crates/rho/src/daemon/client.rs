//! Connecting to a running daemon.

use std::{io, path::PathBuf};

use rho_protocol::{Client, ClientError, ClientInfo, HelloResponse, Notifications};
use tokio::net::UnixStream;

use super::paths::DaemonPaths;

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

/// The client identity the `rho` CLI sends in its handshake.
pub(crate) fn cli_client_info() -> ClientInfo {
    ClientInfo {
        name: "rho-cli".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
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
    client: ClientInfo,
) -> Result<Connection, ConnectError> {
    let stream = match UnixStream::connect(&paths.socket).await {
        Ok(stream) => stream,
        // No socket, or a socket left by a daemon that died without cleanup.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(ConnectError::NotRunning)
        }
        Err(source) => {
            return Err(ConnectError::Io {
                socket: paths.socket.clone(),
                source,
            })
        }
    };
    let (client, notifications, hello) = Client::handshake(stream, client).await?;
    Ok(Connection {
        client,
        notifications,
        hello,
    })
}
