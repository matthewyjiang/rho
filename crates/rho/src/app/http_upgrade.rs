//! Upgrading an HTTP server (`rho mcp serve --http`, `rho daemon serve --http`)
//! in place, as the daemon upgrades (`daemon::Executable`): once `rho update`
//! replaced the server's executable with one that proves itself rho, the
//! server stops accepting connections, drains work under way, and re-executes
//! the replacement as the same process. It hands the new binary its listening
//! socket (`--listener-fd`), so a client that connects meanwhile waits in the
//! backlog instead of finding nothing there, and a service manager sees the
//! same process.
//!
//! A replacement older than the running server, as after a rollback, may
//! predate `--listener-fd`, so it is not handed the socket: the server closes
//! it as it re-executes, and the replacement listens anew.
//!
//! Unix only: elsewhere the server keeps running the binary it started with
//! until it is restarted.

use std::{
    ffi::OsString,
    future::Future,
    io,
    net::{SocketAddr, TcpListener},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::sync::{watch, Notify};

use crate::daemon::Executable;

/// The hidden flag that hands a re-executed server its listening socket.
const LISTENER_FD_FLAG: &str = "--listener-fd";

/// How often the server checks whether its executable was replaced.
//
// Matches the daemon's interval (`UPGRADE_CHECK_INTERVAL`): one `stat(2)` of
// the executable (~microseconds), bounding how long a server runs a replaced
// binary before it drains.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// How an HTTP server upgrades in place: its log label, the executable it
/// watches, and the arguments it re-executes the replacement with.
pub(crate) struct Upgrade {
    label: &'static str,
    executable: Executable,
    args: Vec<OsString>,
}

impl Upgrade {
    /// Upgrades of `executable`, re-executed with `args`, the arguments the
    /// server started with after its program. None off Unix, where a server
    /// keeps the binary it started with.
    pub(crate) fn new(
        label: &'static str,
        executable: Executable,
        args: impl Iterator<Item = OsString>,
    ) -> Option<Self> {
        cfg!(unix).then(|| Self {
            label,
            executable,
            args: without_listener_fd(args),
        })
    }

    /// Resolves with the replacement's version once the executable was
    /// replaced by a binary that proves itself rho. Keep one such future
    /// for the server's life: it runs each replacement once.
    pub(crate) async fn replaced(&self) -> String {
        let executable = &self.executable;
        let mut checks = tokio::time::interval(CHECK_INTERVAL);
        checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The replacement last probed to an answer, so each is run once; one
        // that could not run at all is tried again.
        let mut probed = None;
        loop {
            checks.tick().await;
            let Some(replacement) = executable.replacement() else {
                probed = None;
                continue;
            };
            if probed == Some(replacement.identity) {
                continue;
            }
            match executable.probe().await {
                Ok(version) => return version,
                Err(error) if error.is_transient() => {
                    tracing::debug!(%error, "could not probe the server's replaced executable; trying again");
                }
                Err(error) => {
                    eprintln!(
                        "{}: {} was replaced by a file that is not rho ({error}); not upgrading",
                        self.label,
                        executable.path().display()
                    );
                    probed = Some(replacement.identity);
                }
            }
        }
    }

    /// Re-executes the replacement, `version`, as this process, handing it
    /// `listener` unless it is older than this server. Returns only with why
    /// that failed.
    pub(crate) fn relaunch(self, listener: TcpListener, version: &str) -> anyhow::Error {
        let running = env!("CARGO_PKG_VERSION");
        let older = crate::update::version_is_newer(running, version);
        if older {
            eprintln!(
                "{}: {version} is older than {running} and may not take over the listening socket; it listens anew",
                self.label
            );
        }
        #[cfg(unix)]
        {
            let handed = (!older).then_some(&listener);
            let error = unix::relaunch(handed, self.executable.path(), self.args);
            anyhow::Error::new(error).context(format!(
                "could not re-execute {} to upgrade",
                self.executable.path().display()
            ))
        }
        #[cfg(not(unix))]
        {
            let _ = (listener, self.args);
            anyhow::anyhow!("the server upgrades in place only on Unix")
        }
    }
}

/// The listening socket a re-executing server handed this one as `fd`,
/// with `--listener-fd`. Call before anything else this process starts
/// could inherit it.
pub(crate) fn inherit_listener(fd: i32) -> io::Result<TcpListener> {
    #[cfg(unix)]
    {
        let listener = TcpListener::from(crate::daemon::adopt(fd)?);
        listener.local_addr()?;
        listener.set_nonblocking(true)?;
        Ok(listener)
    }
    #[cfg(not(unix))]
    Err(io::Error::other(format!(
        "a server hands on its listening socket only on Unix, not {LISTENER_FD_FLAG} {fd}"
    )))
}

/// Validates that an inherited `socket` listens on `address` (allowing port
/// `0` as a wildcard for the requested port) and converts it to a Tokio
/// `TcpListener`.
pub(crate) fn adopted_listener(
    socket: TcpListener,
    address: SocketAddr,
) -> anyhow::Result<tokio::net::TcpListener> {
    let local = socket.local_addr()?;
    if local.ip() != address.ip() || (address.port() != 0 && local.port() != address.port()) {
        anyhow::bail!("the inherited listening socket is on {local}, not on --http {address}");
    }
    Ok(tokio::net::TcpListener::from_std(socket)?)
}

/// Draining an HTTP server for an upgrade: the calls under way, and the signal
/// that tells each connection to close after the call it serves.
pub(crate) struct Drain {
    calls: InFlight,
    begin: watch::Sender<bool>,
}

impl Drain {
    pub(crate) fn new() -> Self {
        Self {
            calls: InFlight::default(),
            begin: watch::Sender::new(false),
        }
    }

    /// The calls under way, for a connection to count its calls in.
    pub(crate) fn calls(&self) -> InFlight {
        self.calls.clone()
    }

    /// Resolves once draining began, so a connection closes after the call
    /// it serves and the client's next call lands on the replacement.
    pub(crate) fn began(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut begin = self.begin.subscribe();
        async move { drop(begin.wait_for(|began| *began).await) }
    }

    /// Tells every connection to close after the call it serves, then waits
    /// for the calls under way, at most `timeout`. Returns how many are still
    /// under way: zero once they all ended.
    pub(crate) async fn settle(&self, timeout: Duration) -> usize {
        self.begin.send_replace(true);
        self.calls.drained(timeout).await;
        self.calls.count()
    }

    /// Waits for the calls under way, at most `timeout`, then re-executes
    /// `upgrade`'s replacement, `version`, handing it `listener`, which the
    /// caller stopped accepting on. Returns only with why that failed.
    pub(crate) async fn finish(
        &self,
        upgrade: Upgrade,
        version: &str,
        listener: tokio::net::TcpListener,
        timeout: Duration,
    ) -> anyhow::Error {
        // Begun before announcing it, so a call that ends once a client sees
        // the announcement already finds its connection told to close.
        self.begin.send_replace(true);
        eprintln!(
            "{}: upgrading to {version} in place once the calls under way end ({} now)",
            upgrade.label,
            self.calls.count()
        );
        let left = self.settle(timeout).await;
        if left > 0 {
            eprintln!(
                "{}: {left} calls still under way after {timeout:?}; cutting them short to upgrade",
                upgrade.label,
            );
        }
        match listener.into_std() {
            Ok(listener) => upgrade.relaunch(listener, version),
            Err(error) => {
                anyhow::Error::new(error).context("could not hand on the listening socket")
            }
        }
    }
}

/// The calls under way, which an upgrade waits for.
#[derive(Clone, Default)]
pub(crate) struct InFlight {
    count: Arc<AtomicUsize>,
    /// Wakes a drain each time a call ends.
    ended: Arc<Notify>,
}

/// One call under way, until dropped.
pub(crate) struct Call(InFlight);

impl InFlight {
    /// Counts a call under way until the returned guard drops.
    pub(crate) fn enter(&self) -> Call {
        self.count.fetch_add(1, Ordering::SeqCst);
        Call(self.clone())
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Resolves once no call is under way, or after `timeout`.
    async fn drained(&self, timeout: Duration) {
        let _ = tokio::time::timeout(timeout, async {
            loop {
                let ended = self.ended.notified();
                if self.count() == 0 {
                    return;
                }
                ended.await;
            }
        })
        .await;
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::SeqCst);
        self.0.ended.notify_waiters();
    }
}

/// `args` without the `--listener-fd` an earlier re-execution added.
fn without_listener_fd(args: impl Iterator<Item = OsString>) -> Vec<OsString> {
    let mut kept = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == LISTENER_FD_FLAG {
            args.next();
        } else {
            kept.push(arg);
        }
    }
    kept
}

#[cfg(unix)]
mod unix {
    use std::{
        ffi::OsString,
        io,
        net::TcpListener,
        os::{
            fd::{AsFd, AsRawFd},
            unix::process::CommandExt,
        },
        path::Path,
        process::Command,
    };

    use super::LISTENER_FD_FLAG;
    use crate::daemon::{reap_exited_children, set_close_on_exec};

    /// Re-executes `executable` as this process with `args`, handing it
    /// `handed`, the listening socket, if any; without one the exec closes
    /// it. Returns only if that failed.
    pub(super) fn relaunch(
        handed: Option<&TcpListener>,
        executable: &Path,
        args: Vec<OsString>,
    ) -> io::Error {
        let socket = handed.map(AsFd::as_fd);
        // From here until the exec, a child another thread starts inherits
        // the socket too, as with the daemon's handoff; the window is the
        // two calls below.
        if let Some(Err(error)) =
            socket.map(|socket| set_close_on_exec(socket, /*close*/ false))
        {
            return error;
        }
        reap_exited_children();
        let mut command = Command::new(executable);
        command.args(args);
        if let Some(socket) = socket {
            command
                .arg(LISTENER_FD_FLAG)
                .arg(socket.as_raw_fd().to_string());
        }
        let error = command.exec();
        if let Some(socket) = socket {
            let _ = set_close_on_exec(socket, /*close*/ true);
        }
        error
    }
}

#[cfg(test)]
#[path = "http_upgrade_tests.rs"]
mod tests;
