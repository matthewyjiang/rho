//! CLI handler for `rho daemon`.
//!
//! This is the boundary that reads process environment (`RHO_HOME`,
//! `XDG_RUNTIME_DIR`, `RHO_LOG`, `JOURNAL_STREAM`); `crate::daemon` takes the
//! resolved values. `status` and `stop` never start a daemon.

use crate::cli::{Command, DaemonCommand};

#[cfg(unix)]
use {
    crate::daemon::{self, cli_client_info, DaemonPaths},
    crate::logging::{self, LogSink},
    anyhow::Context,
    rho_protocol::{methods, ClientError, DaemonStatus, StatusParams, StopParams},
    rho_sdk::CancellationToken,
    std::time::Duration,
};

#[cfg(not(unix))]
pub(super) async fn run(_command: Option<DaemonCommand>) -> anyhow::Result<()> {
    anyhow::bail!("the rho daemon is not supported on Windows yet (#1665)")
}

#[cfg(unix)]
pub(super) async fn run(command: Option<DaemonCommand>) -> anyhow::Result<()> {
    let rho_home = crate::paths::rho_dir()?;
    let paths = DaemonPaths::resolve(&rho_home, |name| std::env::var_os(name))?;
    match command.unwrap_or(DaemonCommand::Run) {
        DaemonCommand::Run => run_foreground(&paths).await,
        DaemonCommand::Status { json } => status(&paths, json).await,
        DaemonCommand::Stop => stop(&paths).await,
    }
}

/// Whether this invocation runs the daemon, which installs its own always-on
/// subscriber instead of the `RHO_LOG`-gated terminal one.
pub(super) fn runs_daemon(command: Option<&Command>) -> bool {
    matches!(
        command,
        Some(Command::Daemon {
            command: None | Some(DaemonCommand::Run)
        })
    )
}

/// How long `rho daemon stop` waits for the daemon to exit after it
/// acknowledges the request.
//
// Tripwire. Measured with the debug binary from `rho daemon stop` to process
// exit, 15 runs each: median 20 ms idle, max 44 ms with all 24 cores
// saturated. Turn draining (#1661) will need a separate, explicit budget.
#[cfg(unix)]
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(unix)]
async fn run_foreground(paths: &DaemonPaths) -> anyhow::Result<()> {
    paths.prepare_runtime_dir()?;
    // Read once: changing the filter or the sink needs a daemon restart.
    let filter = std::env::var("RHO_LOG")
        .ok()
        .filter(|filter| !filter.trim().is_empty())
        .unwrap_or_else(|| "info".to_owned());
    let journald = std::env::var_os("JOURNAL_STREAM").is_some();
    let sink = if journald {
        LogSink::Journal
    } else {
        let file = paths
            .open_log_file()
            .with_context(|| format!("could not open daemon log {}", paths.log_file.display()))?;
        LogSink::File(file)
    };
    logging::install(&filter, sink);

    let lock = daemon::acquire_lock(paths)?;
    // Install handlers before the socket exists so an early SIGTERM still
    // shuts down cleanly.
    let mut signals = super::shutdown_signal::ShutdownSignals::install()?;
    let bound = daemon::bind(paths, lock)?;
    if !journald {
        eprintln!(
            "rho daemon running (pid {}); logs: {}",
            std::process::id(),
            paths.log_file.display()
        );
    }

    let shutdown = CancellationToken::new();
    let on_signal = shutdown.clone();
    let signal_task = tokio::spawn(async move {
        tokio::select! {
            signal = signals.recv() => match signal {
                Ok(signal) => {
                    tracing::info!(%signal, "shutting down on signal");
                    on_signal.cancel();
                }
                Err(error) => tracing::warn!(%error, "signal handling failed"),
            },
            () = on_signal.cancelled() => {}
        }
    });
    let served = bound.serve(shutdown).await;
    signal_task.abort();
    Ok(served?)
}

#[cfg(unix)]
async fn status(paths: &DaemonPaths, json: bool) -> anyhow::Result<()> {
    let (mut client, _) = daemon::connect(paths, cli_client_info()).await?;
    let status = client.request::<methods::Status>(&StatusParams {}).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        print!("{}", status_text(&status));
    }
    Ok(())
}

#[cfg(unix)]
fn status_text(status: &DaemonStatus) -> String {
    let DaemonStatus {
        pid,
        version,
        protocol,
        uptime_secs,
        jobs,
        socket,
        started_at: _,
    } = status;
    let uptime = humantime::format_duration(Duration::from_secs(*uptime_secs));
    format!(
        "rho daemon is running\n  pid       {pid}\n  version   {version}\n  protocol  {protocol}\n  uptime    {uptime}\n  jobs      {jobs}\n  socket    {}\n",
        socket.display()
    )
}

#[cfg(unix)]
async fn stop(paths: &DaemonPaths) -> anyhow::Result<()> {
    let pid = match daemon::connect(paths, cli_client_info()).await {
        Ok((mut client, hello)) => {
            client.request::<methods::Stop>(&StopParams {}).await?;
            hello.daemon.pid
        }
        // An incompatible daemon cannot take requests. SIGTERM triggers the
        // same graceful shutdown, so the version-mismatch advice to run
        // `rho daemon stop` still works.
        Err(daemon::ConnectError::Handshake(ClientError::VersionMismatch(_))) => {
            daemon::terminate_recorded_daemon(paths)?.ok_or(daemon::ConnectError::NotRunning)?
        }
        Err(error) => return Err(error.into()),
    };
    match daemon::wait_until_released(paths, DAEMON_STOP_TIMEOUT).await? {
        Some(_) => {
            println!("rho daemon stopped (pid {pid})");
            Ok(())
        }
        None => anyhow::bail!(
            "rho daemon (pid {pid}) did not exit within DAEMON_STOP_TIMEOUT ({DAEMON_STOP_TIMEOUT:?}); see {}",
            paths.log_file.display()
        ),
    }
}
