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
    self, ConnectAs, ConnectError, DaemonPaths, LockHolder, DAEMON_RESTART_TIMEOUT,
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

/// Gets the running daemon out of an installer's way, before `rho update`
/// installs, where it would otherwise keep the installer from replacing the
/// executable. An idle daemon is stopped; one with jobs in progress is left
/// alone, and `output` says how to stop it.
pub(crate) async fn make_way_for_update(output: &mut impl Write) -> anyhow::Result<()> {
    if daemon::REPLACEABLE_WHILE_RUNNING {
        return Ok(());
    }
    match stop_if_idle(&daemon_cli::paths()?).await? {
        Retired::NotRunning => {}
        Retired::Stopped => writeln!(
            output,
            "stopped the idle rho daemon so the installer can replace the rho executable; the next rho command starts it again"
        )?,
        Retired::Busy { pid, jobs, .. } => writeln!(
            output,
            "the rho daemon (pid {pid}) keeps the rho executable open and has {jobs} job(s) in progress; once they finish, run `rho daemon stop` before the command below"
        )?,
        Retired::OtherProtocol => writeln!(
            output,
            "a rho daemon of another version is running and keeps the rho executable open; run `rho daemon stop` before the command below"
        )?,
    }
    Ok(())
}

/// Moves the running daemon to the rho `rho update` just `installed`, or
/// says when it moves. Nothing is interrupted: a daemon with work in
/// progress is left to finish it.
pub(crate) async fn follow_update(
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
        Installed::AtAnotherPath => return retire(&paths, output).await,
    }
    let daemon::Connection {
        client,
        mut notifications,
        hello,
    } = match daemon::connect(&paths, ConnectAs::Command.hello()).await {
        Ok(connection) => connection,
        Err(ConnectError::NotRunning) => return Ok(()),
        // A daemon of another protocol answers no status request.
        Err(ConnectError::Handshake(_)) => return retire(&paths, output).await,
        Err(error) => return Err(error.into()),
    };
    let pid = hello.daemon.pid;
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
                "rho daemon (pid {pid}) restarts onto rho {version} once the {waiting_for} in progress finished; nothing is interrupted"
            )?;
            Ok(())
        }
        // It runs from another path than the one installed over.
        Noticed::Nothing => {
            drop(client);
            retire(&paths, output).await
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
            "stopped the idle rho daemon, which ran the rho this update replaced; the next rho command starts the updated one"
        )?,
        Retired::Busy { pid, version, jobs } => writeln!(
            output,
            "rho daemon (pid {pid}) still runs rho {version} and has {jobs} job(s) in progress; once they finish, run `rho daemon restart` to move it to the updated rho"
        )?,
        Retired::OtherProtocol => writeln!(
            output,
            "a rho daemon of another version is still running; run `rho daemon restart` to move it to the updated rho, which interrupts the turns under way"
        )?,
    }
    Ok(())
}

/// What [`stop_if_idle`] found and did.
enum Retired {
    NotRunning,
    /// It had no job in progress and was stopped.
    Stopped,
    /// It was left running its jobs.
    Busy {
        pid: u32,
        version: String,
        jobs: u64,
    },
    /// It speaks a protocol this rho cannot, so it could not say what it
    /// runs and was left alone.
    OtherProtocol,
}

/// Stops the daemon unless it has a job in progress, as `rho daemon stop`
/// does. A job that starts between the check and the stop is recorded
/// interrupted, as by any stop.
async fn stop_if_idle(paths: &DaemonPaths) -> anyhow::Result<Retired> {
    let connection = match daemon::connect(paths, ConnectAs::Command.hello()).await {
        Ok(connection) => connection,
        Err(ConnectError::NotRunning) => return Ok(Retired::NotRunning),
        Err(ConnectError::Handshake(
            ClientError::VersionMismatch(_) | ClientError::UnreadableHello(_),
        )) => return Ok(Retired::OtherProtocol),
        Err(error) => return Err(error.into()),
    };
    let jobs = connection
        .client
        .request::<methods::Status>(&StatusParams {})
        .await?
        .jobs;
    let daemon = connection.hello.daemon;
    drop(connection.client);
    if jobs > 0 {
        return Ok(Retired::Busy {
            pid: daemon.pid,
            version: daemon.version,
            jobs,
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
    /// Its executable is the one it started from.
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
