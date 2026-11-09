//! What a daemon hands the binary it re-executes as the same process
//! (#1713): its lock and its listening socket, as open descriptors the exec
//! keeps.
//!
//! Holding both through the exec is what makes the restart invisible: no
//! other daemon can take the lock in the gap, and a client that connects
//! meanwhile waits in the socket's backlog until the new binary accepts it,
//! instead of finding nothing listening.

use std::{
    fmt,
    fs::File,
    io,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixListener,
    },
    str::FromStr,
};

use super::{
    lock::{self, DaemonLock},
    paths::DaemonPaths,
};

/// The descriptors a re-executed daemon inherits, as
/// `rho daemon run --exec-handoff <lock>:<socket>` passes them, in the
/// form its [`fmt::Display`] and [`FromStr`] agree on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Handoff {
    pub(super) lock: RawFd,
    pub(super) socket: RawFd,
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

/// The listening socket a re-executed daemon inherited, checked to be this
/// home's; [`super::server::rebind`] serves it.
pub(crate) struct InheritedSocket(pub(super) UnixListener);

/// Takes over the lock and the socket `handoff` names, for the daemon
/// re-executed with it, before anything else this process starts could
/// inherit them.
pub(crate) fn take(
    paths: &DaemonPaths,
    handoff: Handoff,
) -> io::Result<(DaemonLock, InheritedSocket)> {
    let lock = lock::adopt(paths, File::from(adopt(handoff.lock)?))?;
    let listener = UnixListener::from(adopt(handoff.socket)?);
    let address = listener.local_addr()?;
    if address.as_pathname() != Some(paths.socket.as_path()) {
        return Err(io::Error::other(format!(
            "inherited descriptor {} is not the daemon socket {}",
            handoff.socket,
            paths.socket.display()
        )));
    }
    Ok((lock, InheritedSocket(listener)))
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
pub(super) fn set_close_on_exec(fd: BorrowedFd<'_>, close: bool) -> io::Result<()> {
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
