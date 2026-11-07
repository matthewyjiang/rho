//! CLI handler for `rho daemon`.
//!
//! `rho daemon run` wires the daemon host (`app::daemon_host`) to the generic
//! transport (`crate::daemon`). This is also the boundary that reads process
//! environment (`RHO_HOME`, `XDG_RUNTIME_DIR`, `RHO_LOG`, `JOURNAL_STREAM`);
//! `crate::daemon` takes the resolved values. `status` and `stop` never start
//! a daemon.

use crate::cli::{Command, DaemonCommand};

#[cfg(unix)]
use {
    super::{
        config_repository::ConfigRepository,
        daemon_host::{DaemonHost, JobRecords},
    },
    crate::daemon::{self, cli_client_info, DaemonPaths},
    crate::logging::{self, LogSink},
    anyhow::Context,
    rho_protocol::{methods, ClientError, DaemonStatus, StatusParams, StopParams},
    rho_sdk::CancellationToken,
    std::{sync::Arc, time::Duration},
};

/// Why a daemon command fails on Windows.
#[cfg(not(unix))]
pub(super) const UNSUPPORTED: &str = "the rho daemon is not supported on Windows yet (#1665)";

#[cfg(not(unix))]
pub(super) async fn run(_command: Option<DaemonCommand>) -> anyhow::Result<()> {
    anyhow::bail!(UNSUPPORTED)
}

#[cfg(unix)]
pub(super) async fn run(command: Option<DaemonCommand>) -> anyhow::Result<()> {
    let paths = paths()?;
    match command.unwrap_or(DaemonCommand::Run) {
        DaemonCommand::Run => run_foreground(&paths).await,
        DaemonCommand::Status { json } => status(&paths, json).await,
        DaemonCommand::Stop => stop(&paths).await,
    }
}

/// The daemon's paths for this rho home, from the process environment.
#[cfg(unix)]
pub(super) fn paths() -> anyhow::Result<DaemonPaths> {
    let rho_home = crate::paths::rho_dir()?;
    let rho_home = daemon::real_home(&rho_home)
        .with_context(|| format!("could not resolve rho home {}", rho_home.display()))?;
    Ok(DaemonPaths::resolve(&rho_home, |name| {
        std::env::var_os(name)
    })?)
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
// exit with no sessions, 15 runs each: median 20 ms idle, max 44 ms with all
// 24 cores saturated. A daemon with turns under way also waits up to
// `DRAIN_TIMEOUT` (4 s, in `daemon_host::sessions`) for them to save, which
// this covers: that wait runs from the stop request. Draining that lets turns
// finish (#1713) needs its own budget.
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
    let journald =
        std::env::var_os("JOURNAL_STREAM").is_some_and(|stream| stderr_is_journal_stream(&stream));
    let sink = if journald {
        LogSink::Journal
    } else {
        let file = paths
            .open_log_file()
            .with_context(|| format!("could not open daemon log {}", paths.log_file.display()))?;
        LogSink::File(file)
    };
    logging::install(&filter, sink);
    // A panicking request handler is answered with an internal error, and an
    // auto-spawned daemon's stderr goes nowhere, so the log is the only place
    // the panic can surface.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "daemon panicked");
        default_hook(info);
    }));

    let lock = daemon::acquire_lock(paths)?;
    // Install handlers before the socket exists so an early SIGTERM still
    // shuts down cleanly.
    let mut signals = super::shutdown_signal::ShutdownSignals::install()?;
    // Everything the host needs is open before the socket is published, so
    // a client never reaches a daemon that cannot serve it.
    open_credential_store();
    let jobs = JobRecords::open(&lock, &paths.state_dir).with_context(|| {
        format!(
            "could not open the daemon's job records in {}",
            paths.state_dir.display()
        )
    })?;
    let bound = daemon::bind(paths, lock)?;
    if !journald {
        eprintln!(
            "rho daemon running (pid {}); logs: {}",
            std::process::id(),
            paths.log_file.display()
        );
    }

    let shutdown = CancellationToken::new();
    let host = Arc::new(DaemonHost::new(
        jobs,
        paths.socket.clone(),
        bound.started_at(),
        shutdown.clone(),
    ));
    let on_signal = Arc::clone(&host);
    // The first signal asks for a graceful shutdown. Shutdown waits for every
    // request already read and for the turns under way to save, so a signal
    // that arrives once it is under way, whether a signal or `rho daemon
    // stop` began it, forces the exit instead.
    let signal_task = tokio::spawn(async move {
        loop {
            match signals.recv().await {
                Ok(signal) if on_signal.stopping() => {
                    tracing::warn!(%signal, "exiting without waiting for in-flight requests");
                    std::process::exit(1);
                }
                Ok(signal) => {
                    tracing::info!(%signal, "shutting down on signal");
                    on_signal.begin_stop();
                }
                Err(error) => {
                    tracing::warn!(%error, "signal handling failed");
                    return;
                }
            }
        }
    });
    // `stop_requested` is when the stop was asked for, whether by `rho
    // daemon stop` or a signal: the turns' drain budget runs from then, not
    // from when the requests in flight finished.
    let (served, stop_requested) = tokio::join!(
        async {
            let served = bound.serve(Arc::clone(&host), shutdown.clone()).await;
            // Serving ends only once the stop was asked for; cancelling
            // again is a no-op that keeps the join from hanging if that
            // ever changes.
            shutdown.cancel();
            served
        },
        async {
            shutdown.cancelled().await;
            tokio::time::Instant::now()
        },
    );
    // Still holding the lock, so no other daemon starts, and `rho daemon
    // stop` keeps waiting, until the turns are saved and recorded.
    host.stop_sessions(stop_requested).await;
    signal_task.abort();
    Ok(served.release()?)
}

/// Opens the credential store every session's provider reads, once, as the
/// daemon starts: like every rho process, the daemon opens it at startup.
/// A config that does not load is logged, and sessions then fail to open
/// with the same error.
#[cfg(unix)]
fn open_credential_store() {
    let repository = ConfigRepository::new(None);
    let opened = repository.configured_path().and_then(|path| {
        let mut config = repository.load()?;
        crate::credential_store::initialize_from_config(&mut config, &path)
    });
    if let Err(error) = opened {
        tracing::warn!(error = %format!("{error:#}"), "could not open the credential store");
    }
}

/// Whether stderr is the journal stream systemd describes in `JOURNAL_STREAM`
/// (`<device>:<inode>`). Per systemd.exec(5), the variable alone is not
/// enough: a child of a systemd service inherits it even when its stderr was
/// redirected elsewhere.
#[cfg(unix)]
fn stderr_is_journal_stream(stream: &std::ffi::OsStr) -> bool {
    use std::os::{fd::AsFd, unix::fs::MetadataExt};
    let Some((device, inode)) = stream.to_str().and_then(|value| value.split_once(':')) else {
        return false;
    };
    let (Ok(device), Ok(inode)) = (device.parse::<u64>(), inode.parse::<u64>()) else {
        return false;
    };
    std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .and_then(|fd| std::fs::File::from(fd).metadata())
        .is_ok_and(|stderr| stderr.dev() == device && stderr.ino() == inode)
}

#[cfg(unix)]
async fn status(paths: &DaemonPaths, json: bool) -> anyhow::Result<()> {
    let client = daemon::connect(paths, cli_client_info()).await?.client;
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
        Ok(connection) => {
            connection
                .client
                .request::<methods::Stop>(&StopParams {})
                .await?;
            connection.hello.daemon.pid
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
