//! The daemon as a process: how it is detached from the client that starts
//! it, which of that client's variables describe the machine, and how it is
//! asked to stop without a protocol request.

use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command},
};

use tokio::signal::windows::{ctrl_break, ctrl_c, CtrlBreak, CtrlC};
use windows_sys::Win32::{
    Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT},
    System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW},
};

use super::pipe::{self, Opened, PipeServer};
use crate::daemon::paths::DaemonPaths;

/// Where the daemon's files are, who it runs as, and its locale. `RHO_HOME`
/// is not here: the spawner pins it to the resolved home.
pub(in crate::daemon) const MACHINE_VARIABLES: &[&str] = &[
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    // What a Unix-style shell on Windows calls the profile.
    "HOME",
    "USERNAME",
    "USERDOMAIN",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "ALLUSERSPROFILE",
    "PUBLIC",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    // Windows itself: sockets and the system libraries do not load without
    // them.
    "SystemRoot",
    "SystemDrive",
    "windir",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
    // The daemon's own executable lookups; a session's use its client's.
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "PSModulePath",
    "TEMP",
    "TMP",
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
/// Windows ignores case in variable names: `Path` is `PATH`.
pub(in crate::daemon) fn is_variable(name: &str, listed: &str) -> bool {
    name.eq_ignore_ascii_case(listed)
}

/// Starts the daemon `command` runs, detached from the caller's terminal.
///
/// A process started on Windows inherits every handle of its parent that is
/// marked inheritable, whatever the child's own standard streams are. The
/// caller's are, when a shell or another program handed it pipes, and a
/// daemon holding their write ends would keep whoever reads them from ever
/// seeing them close. So this marks the caller's own standard streams not
/// inheritable, for good: a child that should share them still gets them,
/// because `std` passes it a duplicate.
pub(in crate::daemon) fn spawn_detached(
    command: &mut Command,
    paths: &DaemonPaths,
) -> io::Result<Child> {
    for stream in [
        io::stdin().as_raw_handle(),
        io::stdout().as_raw_handle(),
        io::stderr().as_raw_handle(),
    ] {
        // SAFETY: the call takes no pointers and fails on a handle that is
        // not open, as when the process has no such stream. A stream it
        // could not change stays as inheritable as it was.
        unsafe { SetHandleInformation(stream as HANDLE, HANDLE_FLAG_INHERIT, 0) };
    }
    command
        // Its own process group, so Ctrl+C in the caller's console does not
        // reach the daemon, and a console of its own with no window, which
        // the console programs its sessions run share instead of each
        // opening one.
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        // Not the caller's directory, which the daemon would otherwise
        // hold for its whole life (it could not be renamed or removed). It
        // opens each session in that session's own workspace.
        .current_dir(&paths.home);
    command.spawn()
}

/// The pipe a daemon is asked to stop through: any connection to it is the
/// request. It is named after the daemon's own pipe, and as private.
fn stop_pipe(paths: &DaemonPaths) -> OsString {
    let mut name = paths.socket.clone().into_os_string();
    name.push("-stop");
    name
}

/// Asks the daemon of `paths` to stop by connecting to its stop pipe, which
/// triggers the same graceful shutdown as a stop request. Windows has no
/// signal to send a process, so `pid` is not what finds it.
pub(in crate::daemon) fn request_stop(paths: &DaemonPaths, _pid: u32) -> io::Result<()> {
    let pipe = stop_pipe(paths);
    match pipe::probe(&pipe)? {
        Opened::Client(_) => Ok(()),
        Opened::Missing => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "the rho daemon is not listening for a stop on {}",
                pipe.display()
            ),
        )),
        // Someone else is asking right now, which stops it as well.
        Opened::Busy => Ok(()),
        Opened::Foreign => Err(pipe::foreign(&pipe)),
    }
}

/// The requests to stop that reach a daemon outside the protocol: a
/// connection to its stop pipe, and Ctrl+C or Ctrl+Break in the console of
/// one run by hand. Listen before the daemon is observable; a request that
/// arrives between `listen` and `recv` is still delivered to `recv`.
pub(crate) struct StopRequests {
    /// `None` once the pipe failed; the console events still stop the
    /// daemon.
    pipe: Option<PipeServer>,
    ctrl_c: CtrlC,
    ctrl_break: CtrlBreak,
}

impl StopRequests {
    pub(crate) fn listen(paths: &DaemonPaths) -> io::Result<Self> {
        Ok(Self {
            pipe: Some(PipeServer::create(&stop_pipe(paths))?),
            ctrl_c: ctrl_c()?,
            ctrl_break: ctrl_break()?,
        })
    }

    /// The name of the next request to stop.
    pub(crate) async fn recv(&mut self) -> &'static str {
        loop {
            let asked = async {
                match &mut self.pipe {
                    Some(pipe) => pipe.accept().await.map(drop),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                asked = asked => match asked {
                    Ok(()) => return "stop request",
                    Err(error) => {
                        tracing::warn!(%error, "the stop pipe failed; only a protocol request or the console stops this daemon now");
                        self.pipe = None;
                    }
                },
                _ = self.ctrl_c.recv() => return "Ctrl+C",
                _ = self.ctrl_break.recv() => return "Ctrl+Break",
            }
        }
    }
}

/// Windows has no systemd journal, so stderr is never its stream.
pub(crate) fn stderr_is_journal_stream(_stream: &OsStr) -> bool {
    false
}
