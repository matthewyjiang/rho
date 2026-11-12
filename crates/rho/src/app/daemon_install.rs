//! What installing rho does about the daemon that is running (#1666).
//!
//! `rho update` leaves the daemon on the new binary without cutting work
//! short. Where the installer wrote over the daemon's executable, the daemon
//! notices the new file itself and re-executes it once nothing would be cut
//! short (#1713), and the update waits to say which of the two happened.
//! Where the new version went to another path, or the daemon holds its file
//! against replacement, an idle daemon is stopped instead, so the next
//! client starts the new one; a busy daemon is left alone, with what to run
//! once it is done. `rho uninstall` stops the daemon before it removes
//! anything.
//!
//! Whether a running daemon's file can be replaced comes from the daemon's
//! platform boundary ([`daemon::REPLACEABLE_WHILE_RUNNING`]); nothing here
//! branches on the operating system.

use std::{io::Write, time::Duration};

use rho_protocol::{methods, ClientError, PendingUpgrade, StatusParams};

use super::daemon_cli;
use crate::daemon::{
    self, ConnectAs, ConnectError, DaemonPaths, Executable, LockHolder, DAEMON_RESTART_TIMEOUT,
};

/// How long `rho update` waits for the daemon to notice the executable it
/// just installed.
//
// Tripwire. The daemon looks at its executable every `UPGRADE_CHECK_INTERVAL`
// (1 s, in `daemon_host::upgrade`), runs the new one with `--version`, and
// restarts. Measured with the debug binary, all of `rho update` with a
// stand-in installer and an idle daemon one full interval from its next
// check: 1.02 s median (1.03 s max) over 20 runs idle, and 1.06 s median
// (1.08 s max) over 8 runs with all 12 cores saturated. A daemon that says
// nothing for 5 s was not installed over.
const UPGRADE_NOTICE_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between `rho/daemon/status` requests while the update waits.
/// Each is one request on a local endpoint.
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Where an installer put the new rho.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Installed {
    /// At the path the old one had, as the install script, Cargo, and
    /// pacman do.
    OverTheExecutable,
    /// In a directory of its own, as mise and Scoop keep each version.
    AtAnotherPath,
}

/// The daemon as `rho update` found it before installing, to follow once
/// the installer ran.
pub(crate) struct Watched {
    /// The rho that runs the update, by the path an installer writes over.
    updating: Option<Executable>,
    daemon: Before,
}

/// What ran before the installer did.
enum Before {
    /// No daemon, or one already dealt with.
    Nothing,
    /// A connection to the daemon, held while the installer runs: the
    /// daemon closes it as it restarts, however long the installer goes on
    /// after writing the new binary.
    Running(Box<daemon::Connection>),
    /// A daemon of another protocol, which answers no request.
    OtherProtocol,
}

/// Finds the running daemon before `rho update` installs. Where the daemon
/// would keep the installer from replacing the executable, an idle one is
/// stopped, and `output` says how to stop a busy one.
pub(crate) async fn before_update(output: &mut impl Write) -> anyhow::Result<Watched> {
    let paths = daemon_cli::paths()?;
    let daemon = if daemon::REPLACEABLE_WHILE_RUNNING {
        match daemon::connect(&paths, ConnectAs::Command.hello()).await {
            Ok(connection) => Before::Running(Box::new(connection)),
            Err(ConnectError::NotRunning) => Before::Nothing,
            Err(ConnectError::Handshake(_)) => Before::OtherProtocol,
            Err(error) => return Err(error.into()),
        }
    } else {
        make_way(&paths, output).await?;
        Before::Nothing
    };
    Ok(Watched {
        updating: Executable::of_this_process().ok(),
        daemon,
    })
}

/// Stops an idle daemon so an installer can replace the executable it
/// holds; says how to stop one that is busy.
async fn make_way(paths: &DaemonPaths, output: &mut impl Write) -> anyhow::Result<()> {
    match stop_if_idle(paths).await? {
        Retired::NotRunning => {}
        Retired::Stopped => writeln!(
            output,
            "stopped the idle rho daemon so the installer can replace the rho executable; the next rho command starts it again"
        )?,
        Retired::Busy { pid, in_progress, .. } => writeln!(
            output,
            "the rho daemon (pid {pid}) keeps the rho executable open and has {in_progress} {IN_PROGRESS}; once that finished, run `rho daemon stop` before the command below"
        )?,
        Retired::OtherProtocol => writeln!(
            output,
            "a rho daemon of another version is running and keeps the rho executable open; run `rho daemon stop` before the command below"
        )?,
    }
    Ok(())
}

/// What a daemon's count of work in progress counts, after the number.
const IN_PROGRESS: &str = "job(s), login(s), or session task(s) in progress";

impl Watched {
    /// Moves the daemon to the rho the update `installed`, or says when it
    /// moves. Nothing is interrupted: a daemon with work in progress is
    /// left to finish it.
    pub(crate) async fn follow(
        self,
        installed: Installed,
        output: &mut impl Write,
    ) -> anyhow::Result<()> {
        // Nothing was installed over a daemon that holds its executable.
        if !daemon::REPLACEABLE_WHILE_RUNNING {
            return Ok(());
        }
        let paths = daemon_cli::paths()?;
        match installed {
            Installed::OverTheExecutable => {}
            Installed::AtAnotherPath => {
                drop(self.daemon);
                return retire(&paths, output).await;
            }
        }
        // An installer that found nothing newer leaves this rho's file as
        // it was, and the daemon as it is.
        if self
            .updating
            .is_some_and(|updating| updating.replacement().is_none())
        {
            return Ok(());
        }
        let daemon::Connection {
            client,
            mut notifications,
            hello,
        } = match self.daemon {
            Before::Nothing => return Ok(()),
            Before::OtherProtocol => {
                writeln!(
                    output,
                    "a rho daemon of another version is still running; run `rho daemon restart` to move it to the updated rho, which interrupts the work in progress"
                )?;
                return Ok(());
            }
            Before::Running(connection) => *connection,
        };
        let rho_protocol::DaemonInfo { pid, version, .. } = hello.daemon;
        match noticed(&client, &mut notifications).await {
            Noticed::Restarting => {
                drop(client);
                restarted(&paths, pid, output).await
            }
            Noticed::Waiting(PendingUpgrade {
                version,
                waiting_for,
            }) => {
                writeln!(
                    output,
                    "rho daemon (pid {pid}) restarts onto rho {version} once its {waiting_for} {IN_PROGRESS} finish; nothing is interrupted"
                )?;
                Ok(())
            }
            Noticed::Nothing => {
                writeln!(
                    output,
                    "rho daemon (pid {pid}, version {version}) did not restart within UPGRADE_NOTICE_TIMEOUT ({UPGRADE_NOTICE_TIMEOUT:?}): it runs another rho executable than the one updated. Run `rho daemon restart` from the updated rho to move it over, which interrupts the work in progress"
                )?;
                Ok(())
            }
        }
    }
}

/// Stops an idle daemon that cannot re-execute the updated rho, so the next
/// client starts it; says what to run for one that is busy.
async fn retire(paths: &DaemonPaths, output: &mut impl Write) -> anyhow::Result<()> {
    match stop_if_idle(paths).await? {
        Retired::NotRunning => {}
        Retired::Stopped => writeln!(
            output,
            "stopped the idle rho daemon; the next rho command starts it from the updated rho"
        )?,
        Retired::Busy {
            pid,
            version,
            in_progress,
        } => writeln!(
            output,
            "rho daemon (pid {pid}) still runs rho {version} and has {in_progress} {IN_PROGRESS}; once that finished, run `rho daemon restart` to move it to the updated rho"
        )?,
        Retired::OtherProtocol => writeln!(
            output,
            "a rho daemon of another version is still running; run `rho daemon restart` to move it to the updated rho, which interrupts the work in progress"
        )?,
    }
    Ok(())
}

/// What [`stop_if_idle`] found and did.
enum Retired {
    NotRunning,
    /// Nothing was in progress, and it was stopped.
    Stopped,
    /// It was left to its work.
    Busy {
        pid: u32,
        version: String,
        in_progress: u64,
    },
    /// It speaks a protocol this rho cannot, so it could not say what it
    /// runs and was left alone.
    OtherProtocol,
}

/// Stops the daemon unless a stop would interrupt something: what its
/// upgrade in place waits for too. Work that starts between the check and
/// the stop is recorded interrupted, as by any stop.
async fn stop_if_idle(paths: &DaemonPaths) -> anyhow::Result<Retired> {
    let connection = match daemon::connect(paths, ConnectAs::Command.hello()).await {
        Ok(connection) => connection,
        Err(ConnectError::NotRunning) => return Ok(Retired::NotRunning),
        Err(ConnectError::Handshake(
            ClientError::VersionMismatch(_) | ClientError::UnreadableHello(_),
        )) => return Ok(Retired::OtherProtocol),
        Err(error) => return Err(error.into()),
    };
    let in_progress = connection
        .client
        .request::<methods::Status>(&StatusParams {})
        .await?
        .in_progress;
    let daemon = connection.hello.daemon;
    drop(connection.client);
    if in_progress > 0 {
        return Ok(Retired::Busy {
            pid: daemon.pid,
            version: daemon.version,
            in_progress,
        });
    }
    daemon_cli::stop(paths).await?;
    Ok(Retired::Stopped)
}

/// What the daemon did about its replaced executable.
enum Noticed {
    /// It closed this connection to restart.
    Restarting,
    /// It waits for work in progress, then restarts.
    Waiting(PendingUpgrade),
    /// It said nothing in time: its executable is not the one installed
    /// over.
    Nothing,
}

/// Waits until the daemon restarts or reports the upgrade waiting for work,
/// for at most [`UPGRADE_NOTICE_TIMEOUT`].
async fn noticed(
    client: &rho_protocol::Client,
    notifications: &mut rho_protocol::Notifications,
) -> Noticed {
    let deadline = tokio::time::Instant::now() + UPGRADE_NOTICE_TIMEOUT;
    let mut polls = tokio::time::interval(STATUS_POLL_INTERVAL);
    loop {
        tokio::select! {
            notification = notifications.recv() => {
                if notification.is_none() {
                    return Noticed::Restarting;
                }
            }
            _ = polls.tick() => {
                // A request the restart cut short fails; the closed
                // connection then says so above.
                let Ok(status) = client.request::<methods::Status>(&StatusParams {}).await else {
                    continue;
                };
                // An upgrade waiting for nothing restarts at its next
                // check, so keep watching for that.
                if let Some(upgrade) = status.upgrade.filter(|upgrade| upgrade.waiting_for > 0) {
                    return Noticed::Waiting(upgrade);
                }
            }
            () = tokio::time::sleep_until(deadline) => return Noticed::Nothing,
        }
    }
}

/// Waits for the daemon that began restarting to answer, and reports it.
async fn restarted(paths: &DaemonPaths, pid: u32, output: &mut impl Write) -> anyhow::Result<()> {
    let restarted = tokio::time::timeout(
        DAEMON_RESTART_TIMEOUT,
        daemon::connect_restarting(paths, ConnectAs::Command.hello()),
    )
    .await;
    match restarted {
        Ok(Ok(connection)) => {
            let daemon = connection.hello.daemon;
            writeln!(
                output,
                "rho daemon restarted onto the updated rho (pid {}, version {})",
                daemon.pid, daemon.version
            )?;
            Ok(())
        }
        // This is still the rho that was replaced, so a daemon of the new
        // one may speak a protocol it cannot: it restarted all the same.
        Ok(Err(ConnectError::Handshake(ClientError::VersionMismatch(mismatch)))) => {
            writeln!(
                output,
                "rho daemon restarted onto the updated rho (protocol {})",
                mismatch.daemon
            )?;
            Ok(())
        }
        Ok(Err(error)) => Err(anyhow::Error::new(error).context(format!(
            "rho daemon (pid {pid}) did not come back after restarting; see {}",
            paths.log_file.display()
        ))),
        Err(_) => anyhow::bail!(
            "rho daemon (pid {pid}) did not answer within DAEMON_RESTART_TIMEOUT ({DAEMON_RESTART_TIMEOUT:?}) of restarting; see {}",
            paths.log_file.display()
        ),
    }
}

/// The daemon `rho uninstall` has to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RunningDaemon {
    /// Unknown while the daemon is starting or already stopping.
    pub(crate) pid: Option<u32>,
}

/// This rho home's running daemon, as its lock shows it: no request is
/// sent, so a daemon of any protocol is found. A rho home that cannot be
/// resolved has none this process could stop.
pub(crate) fn running_daemon() -> Option<RunningDaemon> {
    let paths = daemon_cli::paths().ok()?;
    match daemon::lock_holder(&paths) {
        LockHolder::Nobody => None,
        LockHolder::Unrecorded => Some(RunningDaemon { pid: None }),
        LockHolder::Daemon(record) => Some(RunningDaemon {
            pid: Some(record.pid),
        }),
    }
}

/// Stops this rho home's daemon as `rho daemon stop` does and waits for it
/// to exit. One that exited meanwhile counts as stopped.
pub(crate) async fn stop_for_uninstall() -> anyhow::Result<()> {
    let paths = daemon_cli::paths()?;
    match daemon_cli::stop(&paths).await {
        Err(_) if daemon::lock_holder(&paths) == LockHolder::Nobody => Ok(()),
        stopped => stopped,
    }
}
