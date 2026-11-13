//! The daemon's listener and accept loop. What the daemon answers comes
//! from the [`Service`] its caller passes to [`BoundDaemon::serve`]. A
//! restart ends in [`Restarting::relaunch`], which runs the daemon's binary
//! again as the platform restarts a daemon; where that keeps the listener
//! and the lock, the new run serves them again ([`rebind`]).

use std::{
    ffi::OsString,
    fs, io,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use rho_protocol::{BuildId, DaemonInfo, FrameError, Service, PROTOCOL_VERSION};
use rho_sdk::CancellationToken;
use tokio::task::JoinSet;

use super::{
    executable::Executable,
    lock::{self, DaemonLock, DaemonRecord},
    paths::DaemonPaths,
    platform::{self, Listener},
    shutdown::{Ending, Shutdown},
};

/// A daemon that owns its socket and is ready to serve.
pub(crate) struct BoundDaemon {
    listener: Listener,
    paths: DaemonPaths,
    started_at: u64,
    // Handed to the caller in [`Served`] once serving ends.
    lock: DaemonLock,
}

/// How [`BoundDaemon::serve`] ended. It still holds the daemon lock, so the
/// caller can finish its own shutdown, such as saving the turns it hosts,
/// before another daemon may start and before `rho daemon stop` reports the
/// daemon gone.
#[must_use = "dropping Served releases the daemon lock"]
pub(crate) enum Served {
    /// Serving ended for a stop, or failed; the socket is gone.
    Stopped {
        result: io::Result<()>,
        lock: DaemonLock,
    },
    /// Serving ended for a restart. Boxed: it carries the daemon's paths.
    Restarting(Box<Restarting>),
}

/// A daemon that stopped serving to restart: it still holds the listener,
/// whose backlog keeps the clients that connect meanwhile waiting.
pub(crate) struct Restarting {
    pub(super) lock: DaemonLock,
    pub(super) listener: Listener,
    pub(super) paths: DaemonPaths,
}

impl Served {
    /// Releases the daemon lock, after removing the socket and info file a
    /// restart kept, and returns whether serving failed.
    pub(crate) fn release(self) -> io::Result<()> {
        match self {
            Self::Stopped { result, lock } => {
                drop(lock);
                result
            }
            Self::Restarting(restarting) => {
                restarting.release();
                Ok(())
            }
        }
    }
}

impl Restarting {
    /// Stops listening, removes the socket and the info file, and releases
    /// the lock, so the next client starts a new daemon.
    pub(super) fn release(self) {
        let Self {
            lock,
            listener,
            paths,
        } = self;
        drop(listener);
        remove_daemon_files(&paths);
        drop(lock);
    }

    /// Runs `executable` again in this daemon's place, with the arguments
    /// `args` makes of the handoff token, if the platform's restart passes
    /// the new run one. `Ok` once another process is that run, and this one
    /// should exit; where the restart replaces this process instead, it
    /// returns only an error. Either way an error comes once the lock is
    /// released and the socket removed, so the next client starts a new
    /// daemon.
    pub(crate) fn relaunch(
        self,
        executable: &Executable,
        args: impl FnOnce(Option<String>) -> Vec<OsString>,
    ) -> io::Result<()> {
        platform::relaunch(self, executable.path(), args)
    }
}

/// Takes over the lock and the listener the handoff `token` names, for a
/// daemon its predecessor restarted with one, before anything else this
/// process starts could inherit them. Where a restart keeps no listener,
/// there is none to take over, and the caller binds one as any daemon does.
pub(crate) fn take_handoff(
    paths: &DaemonPaths,
    token: &str,
) -> io::Result<(DaemonLock, Option<Inherited>)> {
    let (lock, listener) = platform::take_handoff(paths, token)?;
    Ok((DaemonLock::adopt(lock)?, listener.map(Inherited)))
}

/// The listener a restarted daemon inherited, checked to be this home's;
/// [`rebind`] serves it.
pub(crate) struct Inherited(Listener);

/// Binds the daemon socket and publishes the info file.
///
/// Taking the [`DaemonLock`] proves no other daemon serves this rho home, and
/// the socket and info file belong to this home alone, so the ones a crashed
/// daemon left behind are removed before binding. The runtime directory must
/// already be prepared.
pub(crate) fn bind(paths: &DaemonPaths, lock: DaemonLock) -> io::Result<BoundDaemon> {
    match fs::remove_file(&paths.info_file) {
        Ok(()) => {
            tracing::info!(path = %paths.info_file.display(), "removed stale daemon file");
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = Listener::bind(paths)?;
    publish(paths, listener, lock, "rho daemon listening")
}

/// Serves the listener the daemon this process restarted from handed it,
/// with its lock, and publishes this binary's info file. Connections that
/// waited in the listener's backlog meanwhile are accepted first.
pub(crate) fn rebind(
    paths: &DaemonPaths,
    lock: DaemonLock,
    Inherited(listener): Inherited,
) -> io::Result<BoundDaemon> {
    publish(paths, listener, lock, "rho daemon re-executed in place")
}

/// Writes the info file for the daemon `listener` serves.
fn publish(
    paths: &DaemonPaths,
    listener: Listener,
    lock: DaemonLock,
    message: &'static str,
) -> io::Result<BoundDaemon> {
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
        "{message}"
    );
    Ok(BoundDaemon {
        listener,
        paths: paths.clone(),
        started_at,
        lock,
    })
}

impl BoundDaemon {
    /// Unix seconds when the daemon bound its socket, as published in the
    /// info file.
    pub(crate) fn started_at(&self) -> u64 {
        self.started_at
    }

    /// Accepts connections and routes their requests to `service` until
    /// `shutdown` is requested, by the service (on `rho/daemon/stop` or
    /// `rho/daemon/restart`) or by the caller (for example on a signal).
    ///
    /// Shutdown is graceful: stop accepting, let every connection answer the
    /// requests it has already read, then remove the socket and info file.
    /// A restart keeps both, and the listener, in the returned [`Served`],
    /// which holds the lock until the caller releases it or re-executes.
    pub(crate) async fn serve<Svc: Service + 'static>(
        self,
        service: Arc<Svc>,
        shutdown: Shutdown,
    ) -> Served {
        let Self {
            mut listener,
            paths,
            started_at: _,
            lock,
        } = self;
        let info = Arc::new(DaemonInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            build: reported_build(),
        });
        let mut connections = JoinSet::new();
        // Ends the connections, once the listener is closed or held. A
        // client reconnects as soon as its connection ends, so one that
        // ended first could reach a listener this daemon is about to close.
        let closing = CancellationToken::new();
        let result = loop {
            tokio::select! {
                // First, so a connection made once shutdown was requested is
                // never accepted only to be closed: it waits in the backlog
                // for a restarted daemon, or finds the socket gone.
                biased;
                () = shutdown.requested() => break Ok(()),
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                accepted = listener.accept() => match accepted {
                    Ok(stream) => {
                        let service = Arc::clone(&service);
                        let info = Arc::clone(&info);
                        let closing = closing.clone();
                        connections.spawn(async move {
                            let served = rho_protocol::serve_connection(
                                stream,
                                &info,
                                service,
                                closing.cancelled(),
                            )
                            .await;
                            match served {
                                Ok(()) => {}
                                // A budget a client can hit is logged where
                                // it is seen.
                                Err(error @ FrameError::Stalled { .. }) => {
                                    tracing::warn!(%error, "dropped a client that stopped reading");
                                }
                                Err(error) => {
                                    tracing::debug!(%error, "connection closed with an error");
                                }
                            }
                        });
                    }
                    Err(error) if is_transient_accept_error(&error) => {
                        tracing::debug!(%error, "accept failed");
                    }
                    Err(error) => {
                        tracing::error!(%error, "accept failed; shutting down");
                        shutdown.request(Ending::Stop, |_| {});
                        break Err(error);
                    }
                },
            }
        };
        // A stopping daemon closes its socket now, so a client that
        // connects meanwhile finds nothing listening and waits for the lock
        // instead. A restarting one holds it as the platform's restart
        // needs: where the new run inherits the socket, that client waits in
        // its backlog.
        let listener = match (&result, shutdown.ending()) {
            (Ok(()), Ending::Restart) => {
                tracing::info!("rho daemon restarting");
                listener.hold_for_restart();
                Some(listener)
            }
            (_, Ending::Stop | Ending::Restart) => {
                tracing::info!("rho daemon shutting down");
                drop(listener);
                None
            }
        };
        // Connections stop reading now, so this waits only for the requests
        // they have already read.
        closing.cancel();
        while connections.join_next().await.is_some() {}
        tracing::info!("rho daemon stopped serving");
        match listener {
            Some(listener) => Served::Restarting(Box::new(Restarting {
                lock,
                listener,
                paths,
            })),
            None => {
                remove_daemon_files(&paths);
                Served::Stopped { result, lock }
            }
        }
    }
}

/// The build this process reports as a daemon in every hello's answer: the
/// one its protocol crate was built with. A debug build reports
/// `RHO_TUI_TEST_BUILD_ID` instead when it is set, so a test can run a
/// daemon of another build; an auto-started daemon gets it from the client
/// that starts it, as it gets the other fixture variables.
fn reported_build() -> BuildId {
    #[cfg(debug_assertions)]
    if let Ok(build) = std::env::var("RHO_TUI_TEST_BUILD_ID") {
        return BuildId::new(build);
    }
    BuildId::of_this_crate()
}

/// Removes the socket and the info file, which no daemon serves any more.
fn remove_daemon_files(paths: &DaemonPaths) {
    let removed = [
        (&paths.socket, platform::remove_endpoint(paths)),
        (&paths.info_file, fs::remove_file(&paths.info_file)),
    ];
    for (path, removed) in removed {
        if let Err(error) = removed {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), %error, "could not remove daemon file");
            }
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
