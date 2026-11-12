//! Restarting in place (#1713): the daemon re-executes its binary as the
//! same process and hands it its lock and its listening socket, as open
//! descriptors the exec keeps.
//!
//! Holding both through the exec is what makes the restart invisible: no
//! other daemon can take the lock in the gap, and a client that connects
//! meanwhile waits in the socket's backlog until the new binary accepts it,
//! instead of finding nothing listening.

use std::{
    ffi::OsString,
    fmt,
    fs::{self, File},
    io,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::{fs::MetadataExt, net::UnixListener as StdUnixListener, process::CommandExt},
    },
    path::Path,
    process::Command,
    str::FromStr,
};

use tokio::net::UnixListener;

use super::endpoint::Listener;
use crate::daemon::{paths::DaemonPaths, server::Restarting};

/// The descriptors a re-executed daemon inherits, as the handoff token
/// `<lock>:<socket>` names them, in the form its [`fmt::Display`] and
/// [`FromStr`] agree on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Handoff {
    lock: RawFd,
    socket: RawFd,
}

impl fmt::Display for Handoff {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.lock, self.socket)
    }
}

impl FromStr for Handoff {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parse = |fd: &str| {
            fd.parse::<RawFd>()
                .ok()
                .filter(|fd| *fd > 2)
                .ok_or_else(|| format!("{fd:?} is not an inherited descriptor"))
        };
        let (lock, socket) = value
            .split_once(':')
            .ok_or_else(|| format!("expected <lock>:<socket>, got {value:?}"))?;
        Ok(Self {
            lock: parse(lock)?,
            socket: parse(socket)?,
        })
    }
}

/// Re-executes `executable` as this process with the arguments `args` makes
/// of the handoff token that passes it the daemon lock and the listening
/// socket. Returns only if that failed, once it released the lock and
/// removed the socket, so the next client starts a new daemon.
pub(in crate::daemon) fn relaunch(
    restarting: Restarting,
    executable: &Path,
    args: impl FnOnce(Option<String>) -> Vec<OsString>,
) -> io::Result<()> {
    let lock = restarting.lock.file().as_fd();
    let socket = restarting.listener.socket.as_fd();
    let handoff = Handoff {
        lock: lock.as_raw_fd(),
        socket: socket.as_raw_fd(),
    };
    let kept = set_close_on_exec(socket, false).and_then(|()| set_close_on_exec(lock, false));
    let error = match kept {
        Ok(()) => {
            reap_exited_children();
            tracing::info!(executable = %executable.display(), "re-executing the rho daemon");
            Command::new(executable)
                .args(args(Some(handoff.to_string())))
                .exec()
        }
        Err(error) => error,
    };
    tracing::error!(
        executable = %executable.display(),
        %error,
        "could not re-execute the rho daemon; exiting"
    );
    restarting.release();
    Err(error)
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

/// Takes over the lock file and the listening socket the handoff `token`
/// names, for the daemon re-executed with it, before anything else this
/// process starts could inherit them. The lock file is this home's, and the
/// socket is the one of `paths`; connections that waited in its backlog
/// meanwhile are accepted first.
pub(in crate::daemon) fn take_handoff(
    paths: &DaemonPaths,
    token: &str,
) -> io::Result<(File, Listener)> {
    let handoff: Handoff = token
        .parse()
        .map_err(|error: String| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let lock = File::from(adopt(handoff.lock)?);
    let inherited = lock.metadata()?;
    let on_disk = fs::symlink_metadata(&paths.lock)?;
    if (inherited.dev(), inherited.ino()) != (on_disk.dev(), on_disk.ino()) {
        return Err(io::Error::other(format!(
            "the inherited lock is not the daemon lock {}",
            paths.lock.display()
        )));
    }
    let listener = StdUnixListener::from(adopt(handoff.socket)?);
    let address = listener.local_addr()?;
    if address.as_pathname() != Some(paths.socket.as_path()) {
        return Err(io::Error::other(format!(
            "inherited descriptor {} is not the daemon socket {}",
            handoff.socket,
            paths.socket.display()
        )));
    }
    listener.set_nonblocking(true)?;
    Ok((lock, Listener::serving(UnixListener::from_std(listener)?)))
}

/// Takes ownership of inherited descriptor `fd`, once it is open, and marks
/// it close-on-exec again, so the processes the daemon's sessions start do
/// not inherit the lock or the socket.
fn adopt(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: `fcntl(F_GETFD)` takes no pointers; on a descriptor that is
    // not open it fails with EBADF and touches nothing.
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is open, and nothing else in this process owns it: the
    // predecessor left it open across the exec for this process alone, and
    // `rho daemon run` adopts each handoff once, at startup.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    set_close_on_exec(owned.as_fd(), true)?;
    Ok(owned)
}

/// Sets or clears `FD_CLOEXEC` on `fd`. The handoff clears it right before
/// the exec, so the exec keeps the descriptor open.
fn set_close_on_exec(fd: BorrowedFd<'_>, close: bool) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: `fcntl` with these commands takes no pointers.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    let flags = if close {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    // SAFETY: as above.
    if unsafe { libc::fcntl(raw, libc::F_SETFD, flags) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
