//! The daemon as a process: how it is detached from the client that starts
//! it, which of that client's variables describe the machine, and how it is
//! asked to stop without a protocol request.

use std::{
    ffi::OsStr,
    io,
    os::{
        fd::AsFd,
        unix::{fs::MetadataExt, process::CommandExt},
    },
    process::Command,
};

use tokio::signal::unix::{signal, Signal, SignalKind};

use crate::daemon::paths::DaemonPaths;

/// Where the daemon's files are, who it runs as, and its locale. `RHO_HOME`
/// is not here: the spawner pins it to the resolved home.
pub(in crate::daemon) const MACHINE_VARIABLES: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    // The daemon's own executable lookups; a session's use its client's.
    "PATH",
    "TMPDIR",
    // What the client's loader found this executable's libraries with.
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    // The socket's directory, which the client resolved its path from.
    "XDG_RUNTIME_DIR",
    // Model catalog caches.
    "XDG_CACHE_HOME",
    // The global ignore file of the `git` the write gate asks, and of
    // workspace walks.
    "XDG_CONFIG_HOME",
    "GIT_CONFIG_GLOBAL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
];

/// Whether the environment variable `name` is the one `listed` names.
/// Variable names are case sensitive here.
pub(in crate::daemon) fn is_variable(name: &str, listed: &str) -> bool {
    name == listed
}

/// Detaches the daemon `command` starts from the caller's terminal.
pub(in crate::daemon) fn detach(command: &mut Command, _paths: &DaemonPaths) {
    command
        // Its own process group, so Ctrl+C in the caller's terminal does
        // not reach the daemon.
        .process_group(0)
        // Not the caller's directory, which the daemon would otherwise
        // hold for its whole life (blocking an unmount, say). It opens
        // each session in that session's own workspace.
        .current_dir("/");
}

/// Sends SIGTERM to the daemon running as `pid`, which triggers the same
/// graceful shutdown as a stop request.
pub(in crate::daemon) fn request_stop(_paths: &DaemonPaths, pid: u32) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    // SAFETY: `kill` takes no pointers and has no memory-safety preconditions.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The requests to stop that reach a daemon outside the protocol: SIGINT
/// and SIGTERM. Listen before the daemon is observable; a signal that
/// arrives between `listen` and `recv` is still delivered to `recv`.
pub(crate) struct StopRequests {
    interrupt: Signal,
    terminate: Signal,
}

impl StopRequests {
    pub(crate) fn listen(_paths: &DaemonPaths) -> io::Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    /// The name of the next request to stop.
    pub(crate) async fn recv(&mut self) -> &'static str {
        tokio::select! {
            _ = self.interrupt.recv() => "SIGINT",
            _ = self.terminate.recv() => "SIGTERM",
        }
    }
}

/// Whether stderr is the journal stream systemd describes in `JOURNAL_STREAM`
/// (`<device>:<inode>`). Per systemd.exec(5), the variable alone is not
/// enough: a child of a systemd service inherits it even when its stderr was
/// redirected elsewhere.
pub(crate) fn stderr_is_journal_stream(stream: &OsStr) -> bool {
    let Some((device, inode)) = stream.to_str().and_then(|value| value.split_once(':')) else {
        return false;
    };
    let (Ok(device), Ok(inode)) = (device.parse::<u64>(), inode.parse::<u64>()) else {
        return false;
    };
    io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .and_then(|fd| std::fs::File::from(fd).metadata())
        .is_ok_and(|stderr| stderr.dev() == device && stderr.ino() == inode)
}
