//! The daemon's socket listener and accept loop. What the daemon answers
//! comes from the [`Service`] its caller passes to [`BoundDaemon::serve`].

use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use rho_protocol::{DaemonInfo, ServerInfo, Service, PROTOCOL_VERSION};
use rho_sdk::CancellationToken;
use tokio::{
    net::{UnixListener, UnixStream},
    task::JoinSet,
};

use super::{
    lock::{self, DaemonLock, DaemonRecord},
    paths::{current_uid, DaemonPaths},
};

/// A daemon that owns its socket and is ready to serve.
pub(crate) struct BoundDaemon {
    listener: UnixListener,
    paths: DaemonPaths,
    started_at: u64,
    // Held until `serve` returns, after the socket and info file are removed.
    _lock: DaemonLock,
}

/// Binds the daemon socket and publishes the info file.
///
/// Taking the [`DaemonLock`] proves no other daemon is alive, so the socket
/// and info file a crashed daemon left behind are removed before binding. The
/// runtime directory must already be prepared.
pub(crate) fn bind(paths: &DaemonPaths, lock: DaemonLock) -> io::Result<BoundDaemon> {
    for stale in [&paths.info_file, &paths.socket] {
        match fs::remove_file(stale) {
            Ok(()) => tracing::info!(path = %stale.display(), "removed stale daemon file"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let listener = UnixListener::bind(&paths.socket)?;
    // The runtime directory is 0700, so nobody else can reach the socket in
    // the moment before this chmod; 0600 keeps it private if the directory's
    // mode is ever loosened.
    fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs());
    lock::write_record(
        paths,
        &DaemonRecord {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: PROTOCOL_VERSION,
            started_at,
            socket: paths.socket.clone(),
        },
    )?;
    tracing::info!(
        pid = std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
        socket = %paths.socket.display(),
        "rho daemon listening"
    );
    Ok(BoundDaemon {
        listener,
        paths: paths.clone(),
        started_at,
        _lock: lock,
    })
}

impl BoundDaemon {
    /// Unix seconds when the daemon bound its socket, as published in the
    /// info file.
    pub(crate) fn started_at(&self) -> u64 {
        self.started_at
    }

    /// Accepts connections and routes their requests to `service` until
    /// `shutdown` is cancelled, by the service (on `rho/daemon/stop`) or by
    /// the caller (for example on a signal).
    ///
    /// Shutdown is graceful: stop accepting, let every connection finish the
    /// response it is writing, then remove the socket and info file and
    /// release the lock.
    pub(crate) async fn serve<Svc: Service + 'static>(
        self,
        service: Arc<Svc>,
        shutdown: CancellationToken,
    ) -> io::Result<()> {
        let Self {
            listener,
            paths,
            started_at: _,
            _lock,
        } = self;
        let info = Arc::new(ServerInfo {
            daemon: DaemonInfo {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                pid: std::process::id(),
            },
            capabilities: Vec::new(),
        });
        let uid = current_uid();
        let mut connections = JoinSet::new();
        let result = loop {
            tokio::select! {
                () = shutdown.cancelled() => break Ok(()),
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        if !peer_is_owner(&stream, uid) {
                            continue;
                        }
                        let service = Arc::clone(&service);
                        let info = Arc::clone(&info);
                        let shutdown = shutdown.clone();
                        connections.spawn(async move {
                            let served = rho_protocol::serve_connection(
                                stream,
                                &info,
                                service.as_ref(),
                                shutdown.cancelled(),
                            )
                            .await;
                            if let Err(error) = served {
                                tracing::debug!(%error, "connection closed with an error");
                            }
                        });
                    }
                    Err(error) if is_transient_accept_error(&error) => {
                        tracing::debug!(%error, "accept failed");
                    }
                    Err(error) => {
                        tracing::error!(%error, "accept failed; shutting down");
                        shutdown.cancel();
                        break Err(error);
                    }
                },
            }
        };
        tracing::info!("rho daemon shutting down");
        drop(listener);
        // Connections observe the cancelled token between requests, so this
        // waits only for responses already being written.
        while connections.join_next().await.is_some() {}
        for path in [&paths.socket, &paths.info_file] {
            if let Err(error) = fs::remove_file(path) {
                if error.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), %error, "could not remove daemon file");
                }
            }
        }
        tracing::info!("rho daemon stopped");
        result
    }
}

/// Only the user who owns the daemon may connect. The socket's 0600 mode
/// already enforces this; the peer check keeps it true if the mode is ever
/// loosened.
fn peer_is_owner(stream: &UnixStream, uid: u32) -> bool {
    match stream.peer_cred() {
        Ok(credentials) if credentials.uid() == uid => true,
        Ok(credentials) => {
            tracing::warn!(
                peer_uid = credentials.uid(),
                "rejected connection from another user"
            );
            false
        }
        Err(error) => {
            tracing::warn!(%error, "rejected connection without peer credentials");
            false
        }
    }
}

/// Accept errors caused by one connection rather than by the listener.
fn is_transient_accept_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
