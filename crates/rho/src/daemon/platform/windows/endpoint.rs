//! The daemon's endpoint: the named pipe `\\.\pipe\rho-daemon-<home id>`,
//! private by its security descriptor (see `security`).

use std::{io, time::Duration};

use rho_protocol::CloseOnShutdown;
use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer};

use super::{
    files::DaemonPathError,
    pipe::{self, Opened, PipeServer},
};
use crate::daemon::paths::{DaemonPaths, SocketFound};

/// Interval between connection attempts while a daemon restarts. Each
/// attempt is one `CreateFile` on a local pipe.
const RESTART_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A client's connection to the daemon. A pipe cannot close one direction
/// alone, so a client that shuts down its writes stops reading too, and the
/// daemon sees it leave.
pub(in crate::daemon) type Stream = CloseOnShutdown<NamedPipeClient>;

/// The daemon's listening pipe.
pub(in crate::daemon) struct Listener {
    /// `None` once the daemon stopped listening to restart.
    pipe: Option<PipeServer>,
}

impl Listener {
    /// Creates the pipe of `paths`. A crashed daemon leaves none behind:
    /// its pipe went with its process. Fails if the pipe exists, which the
    /// daemon lock rules out for a daemon of this home, so that pipe is
    /// someone else's.
    pub(in crate::daemon) fn bind(paths: &DaemonPaths) -> io::Result<Self> {
        Ok(Self {
            pipe: Some(PipeServer::create(paths.socket.as_os_str())?),
        })
    }

    /// The next connection. Only the user who owns the daemon can open the
    /// pipe, so every connection is theirs. Cancel safe.
    pub(in crate::daemon) async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        match &mut self.pipe {
            Some(pipe) => pipe.accept().await,
            None => std::future::pending().await,
        }
    }

    /// Called once serving ended for a restart. Closes the pipe: a restart
    /// here starts another process, which creates the pipe again, and a pipe
    /// has no backlog to hold a client for it. An instance left waiting
    /// would take a client's connection that nothing ever serves.
    pub(in crate::daemon) fn hold_for_restart(&mut self) {
        self.pipe = None;
    }
}

/// Nothing to remove: the pipe is gone once the daemon closed its listener
/// and its connections.
pub(in crate::daemon) fn remove_endpoint(_paths: &DaemonPaths) -> io::Result<()> {
    Ok(())
}

/// Connects to the daemon's pipe, once it proved to be this user's. `None`
/// when no pipe of that name exists.
pub(in crate::daemon) async fn connect(paths: &DaemonPaths) -> io::Result<Option<Stream>> {
    Ok(pipe::open(paths.socket.as_os_str())
        .await?
        .map(CloseOnShutdown::new))
}

/// Connects to a daemon that is restarting. It closes its pipe and another
/// process creates it again (see `restart`), so this waits until one
/// listens, however long the old one takes to drain; the caller bounds the
/// wait.
pub(in crate::daemon) async fn connect_restarting(
    paths: &DaemonPaths,
) -> io::Result<Option<Stream>> {
    let pipe = paths.socket.as_os_str();
    loop {
        match pipe::try_open(pipe)? {
            Opened::Client(stream) => return Ok(Some(CloseOnShutdown::new(stream))),
            Opened::Foreign => return Err(pipe::foreign(pipe)),
            // Gone, or only the connections the old daemon is still
            // answering keep its name: the new one is not listening yet.
            Opened::Missing | Opened::Busy => {}
        }
        tokio::time::sleep(RESTART_POLL_INTERVAL).await;
    }
}

/// Verifies, creating nothing, that the daemon's pipe is this user's. It
/// connects to look, which the daemon sees as a client that left at once.
pub(in crate::daemon) fn inspect_endpoint(
    paths: &DaemonPaths,
) -> Result<SocketFound, DaemonPathError> {
    let unreadable = |message: String| DaemonPathError::Unreadable {
        path: paths.socket.clone(),
        message,
    };
    match pipe::probe(paths.socket.as_os_str()) {
        Ok(Opened::Client(_)) => Ok(SocketFound::Private),
        Ok(Opened::Missing) => Ok(SocketFound::Missing),
        Ok(Opened::Busy) => Err(unreadable(
            "every instance of the pipe is taken; retry".to_owned(),
        )),
        Ok(Opened::Foreign) => Err(DaemonPathError::ForeignPipe {
            path: paths.socket.clone(),
        }),
        Err(error) => Err(unreadable(error.to_string())),
    }
}
