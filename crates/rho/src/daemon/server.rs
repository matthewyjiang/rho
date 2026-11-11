//! The daemon's socket listener and accept loop. What the daemon answers
//! comes from the [`Service`] its caller passes to [`BoundDaemon::serve`].
//! A restart keeps the listener and the lock for the binary it re-executes
//! ([`Restarting::reexec`]), which serves them again ([`rebind`]).

use std::{
    ffi::OsString,
    fs, io,
    os::{
        fd::{AsFd, AsRawFd},
        unix::{fs::PermissionsExt, process::CommandExt},
    },
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use rho_protocol::{DaemonInfo, FrameError, Service, PROTOCOL_VERSION};
use tokio::{
    net::{UnixListener, UnixStream},
    task::JoinSet,
};

use super::{
    executable::Executable,
    handoff::{self, Handoff, InheritedSocket},
    lock::{self, DaemonLock, DaemonRecord},
    paths::{current_uid, DaemonPaths},
    shutdown::{Ending, Shutdown},
};

/// A daemon that owns its socket and is ready to serve.
pub(crate) struct BoundDaemon {
    listener: UnixListener,
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

/// A daemon that stopped serving to restart: it still holds the listening
/// socket, whose backlog keeps the clients that connect meanwhile waiting.
pub(crate) struct Restarting {
    lock: DaemonLock,
    listener: UnixListener,
    paths: DaemonPaths,
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
    fn release(self) {
        let Self {
            lock,
            listener,
            paths,
        } = self;
        drop(listener);
        remove_daemon_files(&paths);
        drop(lock);
    }

    /// Re-executes `executable` as this process with the arguments `args`
    /// makes of the [`Handoff`] that passes it the daemon lock and the
    /// listening socket. Returns only if that failed, once it released the
    /// lock and removed the socket, so the next client starts a new daemon.
    pub(crate) fn reexec(
        self,
        executable: &Executable,
        args: impl FnOnce(Handoff) -> Vec<OsString>,
    ) -> io::Error {
        let handoff = Handoff {
            lock: self.lock.as_fd().as_raw_fd(),
            socket: self.listener.as_raw_fd(),
        };
        let kept = handoff::set_close_on_exec(self.listener.as_fd(), false)
            .and_then(|()| handoff::set_close_on_exec(self.lock.as_fd(), false));
        let error = match kept {
            Ok(()) => {
                reap_exited_children();
                tracing::info!(executable = %executable.path().display(), "re-executing the rho daemon");
                Command::new(executable.path()).args(args(handoff)).exec()
            }
            Err(error) => error,
        };
        tracing::error!(
            executable = %executable.path().display(),
            %error,
            "could not re-execute the rho daemon; exiting"
        );
        self.release();
        error
    }
}

/// Reaps the children that already exited, which the exec would leave as
/// zombies: the new binary never waits for what this one started.
fn reap_exited_children() {
    loop {
        // SAFETY: a null status pointer is allowed; `WNOHANG` returns at
        // once with 0 while children still run, and -1 once none is left.
        let reaped = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
        if reaped <= 0 {
            return;
        }
    }
}

/// Binds the daemon socket and publishes the info file.
///
/// Taking the [`DaemonLock`] proves no other daemon serves this rho home, and
/// the socket and info file belong to this home alone, so the ones a crashed
/// daemon left behind are removed before binding. The runtime directory must
/// already be prepared.
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
    publish(paths, listener, lock, "rho daemon listening")
}

/// Serves the socket the daemon this process re-executed from handed it,
/// with its lock, and publishes this binary's info file. Connections that
/// waited in the socket's backlog meanwhile are accepted first.
pub(crate) fn rebind(
    paths: &DaemonPaths,
    lock: DaemonLock,
    InheritedSocket(listener): InheritedSocket,
) -> io::Result<BoundDaemon> {
    listener.set_nonblocking(true)?;
    let listener = UnixListener::from_std(listener)?;
    publish(paths, listener, lock, "rho daemon re-executed in place")
}

/// Writes the info file for the daemon `listener` serves.
fn publish(
    paths: &DaemonPaths,
    listener: UnixListener,
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
            listener,
            paths,
            started_at: _,
            lock,
        } = self;
        let info = Arc::new(DaemonInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
        });
        let uid = current_uid();
        let mut connections = JoinSet::new();
        let result = loop {
            tokio::select! {
                // First, so a connection made once shutdown was requested is
                // never accepted only to be closed: it waits in the backlog
                // for a restarted daemon, or finds the socket gone.
                biased;
                () = shutdown.requested() => break Ok(()),
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
                                service,
                                shutdown.requested(),
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
        // instead. A restarting one keeps it, so that client waits in the
        // backlog for the binary it re-executes.
        let listener = match (&result, shutdown.ending()) {
            (Ok(()), Ending::Restart) => {
                tracing::info!("rho daemon restarting");
                Some(listener)
            }
            (_, Ending::Stop | Ending::Restart) => {
                tracing::info!("rho daemon shutting down");
                drop(listener);
                None
            }
        };
        // Connections stop reading once shutdown is requested, so this waits
        // only for the requests they have already read.
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

/// Removes the socket and the info file, which no daemon serves any more.
fn remove_daemon_files(paths: &DaemonPaths) {
    for path in [&paths.socket, &paths.info_file] {
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), %error, "could not remove daemon file");
            }
        }
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
