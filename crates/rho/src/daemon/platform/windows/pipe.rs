//! Named pipes private to the current user: the server that creates one,
//! and the client that opens one and checks whose it is.

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    time::{Duration, Instant},
};

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_DATA, ERROR_PIPE_BUSY},
    Storage::FileSystem::SECURITY_IDENTIFICATION,
};

use super::security::{owned_by_current_user, OwnerOnly};

/// How long a client waits for a pipe whose every instance is taken.
//
// Tripwire. A pipe has no backlog: a client can open it only while the
// server has an instance nobody connected to yet, and the server makes the
// next one as soon as a client takes the last (see `PipeServer::accept`), so
// the wait is one task wake-up and one `CreateNamedPipe`. Hitting the limit
// means the daemon stopped accepting without closing its pipe.
const PIPE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between attempts to open a busy pipe. Each attempt is one
/// `CreateFile` on a local pipe.
const PIPE_BUSY_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A pipe only the current user can open, and the instance of it the next
/// client connects to.
pub(super) struct PipeServer {
    name: OsString,
    security: OwnerOnly,
    next: NamedPipeServer,
}

impl PipeServer {
    /// Creates the pipe `name`. Fails if a pipe of that name exists, so the
    /// server never joins a pipe someone else created.
    pub(super) fn create(name: &OsStr) -> io::Result<Self> {
        let security = OwnerOnly::new()?;
        let next = instance(name, &security, Instance::First)?;
        Ok(Self {
            name: name.to_owned(),
            security,
            next,
        })
    }

    /// The next client's connection. Cancel safe: a client that connected
    /// stays connected to the waiting instance until a later call returns
    /// it.
    pub(super) async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        match self.next.connect().await {
            Ok(()) => {}
            // The client connected and closed before this call: still a
            // connection, which the caller reads as ended.
            Err(error) if error.raw_os_error() == Some(ERROR_NO_DATA as i32) => {}
            Err(error) => return Err(error),
        }
        let next = instance(&self.name, &self.security, Instance::Another)?;
        Ok(std::mem::replace(&mut self.next, next))
    }
}

/// Which instance of a pipe to create.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Instance {
    /// The one that creates the pipe, and fails if it exists.
    First,
    /// One more of a pipe this server created.
    Another,
}

fn instance(name: &OsStr, security: &OwnerOnly, which: Instance) -> io::Result<NamedPipeServer> {
    let mut attributes = security.attributes();
    // SAFETY: `attributes` is a valid `SECURITY_ATTRIBUTES` whose descriptor
    // `security` keeps alive through the call, which copies it.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(which == Instance::First)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(name, (&raw mut attributes).cast())
    }
}

/// What opening a pipe as a client found.
pub(super) enum Opened<Client> {
    Client(Client),
    /// No pipe of that name: nothing listens.
    Missing,
    /// The pipe exists and every instance is taken.
    Busy,
    /// The pipe belongs to another user.
    Foreign,
}

/// Sorts the result of opening a pipe, checking a pipe that opened is the
/// current user's before anything is written to it.
fn opened<Client: AsRawHandle>(result: io::Result<Client>) -> io::Result<Opened<Client>> {
    match result {
        Ok(client) => Ok(if owned_by_current_user(client.as_raw_handle())? {
            Opened::Client(client)
        } else {
            Opened::Foreign
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Opened::Missing),
        Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => Ok(Opened::Busy),
        Err(error) => Err(error),
    }
}

/// Opens the pipe `name` once, without a runtime, to look at it.
pub(super) fn probe(name: &OsStr) -> io::Result<Opened<File>> {
    opened(
        File::options()
            .read(true)
            .write(true)
            // A pipe's server may impersonate a client that opens it with
            // the default level, before the owner check here says whose
            // server it is. Identification lets it learn who connected and
            // nothing more, which is also what `ClientOptions` asks for.
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(name),
    )
}

/// Connects to the pipe `name`, waiting while every instance is taken. The
/// server learns who connected and cannot impersonate them: `ClientOptions`
/// opens with the identification level.
/// `None` when no pipe of that name exists.
pub(super) async fn open(name: &OsStr) -> io::Result<Option<NamedPipeClient>> {
    let started = Instant::now();
    loop {
        match opened(ClientOptions::new().open(name))? {
            Opened::Client(client) => return Ok(Some(client)),
            Opened::Missing => return Ok(None),
            Opened::Foreign => return Err(foreign(name)),
            Opened::Busy => {}
        }
        let elapsed = started.elapsed();
        if elapsed >= PIPE_BUSY_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "every instance of {} stayed taken for PIPE_BUSY_TIMEOUT ({PIPE_BUSY_TIMEOUT:?}; waited {elapsed:?})",
                    name.display()
                ),
            ));
        }
        tokio::time::sleep(PIPE_BUSY_POLL_INTERVAL).await;
    }
}

/// Why a pipe another user owns is not connected to.
pub(super) fn foreign(name: &OsStr) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "{} was created by another user, so it is not this user's rho daemon",
            name.display()
        ),
    )
}
