//! CLI handler for `rho daemon`.
//!
//! `rho daemon run` wires the daemon host (`app::daemon_host`) to the generic
//! transport (`crate::daemon`). This is also the boundary that reads process
//! environment (`RHO_HOME`, `XDG_RUNTIME_DIR`, `RHO_LOG`, `JOURNAL_STREAM`);
//! `crate::daemon` takes the resolved values. `start` is the clients' ensure
//! running path ([`connect`]) as a command; `status` and `stop` never start a
//! daemon. `restart` re-executes the running daemon in place (#1713), as a
//! replaced executable does once the daemon is idle.

use crate::cli::{Command, DaemonCommand};

use {
    super::{
        config_repository::ConfigRepository,
        daemon_host::{DaemonHost, JobRecords, UNLOAD_IDLE_AFTER},
    },
    crate::daemon::{
        self, ConnectAs, DaemonPaths, Ending, Executable, Served, Shutdown, StopRequests,
        DAEMON_RESTART_TIMEOUT,
    },
    crate::logging::{self, LogSink},
    anyhow::Context,
    rho_protocol::{
        methods, ClientError, DaemonStatus, PendingUpgrade, RestartParams, StatusParams, StopParams,
    },
    std::{ffi::OsString, sync::Arc, time::Duration},
};

pub(super) async fn run(command: Option<DaemonCommand>) -> anyhow::Result<()> {
    let paths = paths()?;
    let run = DaemonCommand::Run {
        unload_idle_after: None,
        exec_handoff: None,
    };
    match command.unwrap_or(run) {
        DaemonCommand::Run {
            unload_idle_after,
            exec_handoff,
        } => {
            let unload_idle_after = unload_idle_after.unwrap_or(UNLOAD_IDLE_AFTER);
            run_foreground(&paths, unload_idle_after, exec_handoff.as_deref()).await
        }
        DaemonCommand::Start => start().await,
        DaemonCommand::Status { json } => status(&paths, json).await,
        DaemonCommand::Stop => stop(&paths).await,
        DaemonCommand::Restart => restart(&paths).await,
    }
}

/// The daemon's paths for this rho home, from the process environment.
pub(super) fn paths() -> anyhow::Result<DaemonPaths> {
    let rho_home = crate::paths::rho_dir()?;
    let rho_home = daemon::real_home(&rho_home)
        .with_context(|| format!("could not resolve rho home {}", rho_home.display()))?;
    Ok(DaemonPaths::resolve(&rho_home, |name| {
        std::env::var_os(name)
    })?)
}

/// Why [`connect`] reached no daemon: the socket path is over the
/// platform's limit, the daemon could not start or did not accept
/// connections in time (the reason names its log file), or its protocol
/// is incompatible (the reason says which side to change and, for an older
/// daemon someone is using, what a restart would cost).
#[derive(Debug, thiserror::Error)]
#[error("could not reach the rho daemon: {reason}; see https://matthewyjiang.github.io/rho/daemon")]
pub(crate) struct Unreachable {
    reason: String,
}

/// Whether connecting may start the daemon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Reach {
    /// Start one when none runs, as the commands that are its clients do.
    #[default]
    StartIfNeeded,
    /// Only reach one already running, as an agent's `sessions` tool does:
    /// a tool call never starts a long-lived daemon on its own.
    Running,
}

/// Connects to this rho home's daemon, starting one when none runs, for
/// the commands that answer no session's requests: `rho run --detach`,
/// `rho login`, and `rho daemon start`.
pub(super) async fn connect() -> Result<daemon::Connection, Unreachable> {
    connect_with(Reach::StartIfNeeded, ConnectAs::Command).await
}

/// Connects to this rho home's daemon as `reach` allows, declaring what
/// `connect_as` does.
pub(super) async fn connect_with(
    reach: Reach,
    connect_as: ConnectAs,
) -> Result<daemon::Connection, Unreachable> {
    let unreachable = |reason: String| Unreachable { reason };
    let paths = paths().map_err(|error| unreachable(format!("{error:#}")))?;
    if reach == Reach::Running {
        return daemon::connect(&paths, connect_as.hello())
            .await
            .map_err(|error| unreachable(error.to_string()));
    }
    let spawner = daemon::ProcessSpawner::current_exe()
        .map_err(|error| unreachable(format!("could not find the rho executable: {error}")))?;
    daemon::connect_or_spawn(&paths, connect_as.hello(), &CheckedSpawner(spawner))
        .await
        .map_err(|error| unreachable(error.to_string()))
}

/// Starts the daemon once this process's own credential store choice is one
/// rho knows. The daemon does not inherit `RHO_CREDENTIAL_STORE` (#1779),
/// but it refuses every session and sign-in of a client whose environment
/// names an unknown store, so a daemon is not started for one: the mistake
/// is reported here, at once. A client of a running daemon hears it from
/// that daemon at its first such request.
struct CheckedSpawner(daemon::ProcessSpawner);

impl daemon::DaemonSpawner for CheckedSpawner {
    fn check(&self) -> std::io::Result<()> {
        crate::credential_store::environment_backend()
            .map(drop)
            .map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("RHO_CREDENTIAL_STORE in this environment: {error}"),
                )
            })
    }

    fn spawn(&self, paths: &DaemonPaths) -> std::io::Result<()> {
        self.0.spawn(paths)
    }
}

/// Whether this invocation runs the daemon, which installs its own always-on
/// subscriber instead of the `RHO_LOG`-gated terminal one.
pub(super) fn runs_daemon(command: Option<&Command>) -> bool {
    matches!(
        command,
        Some(Command::Daemon {
            command: None | Some(DaemonCommand::Run { .. })
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
// this covers: that wait runs from the stop request. An upgrade waits for
// its turns to finish before it asks to restart, so it needs no budget.
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs the daemon until it stops, unloading each session once it sat idle
/// for `unload_idle_after`. A daemon restarted in place takes over its
/// predecessor's lock and socket from `handoff`, the token it was run with.
async fn run_foreground(
    paths: &DaemonPaths,
    unload_idle_after: Duration,
    handoff: Option<&str>,
) -> anyhow::Result<()> {
    // Without it the daemon runs on, only unable to restart in place.
    let executable = Executable::of_this_process()
        .inspect_err(|error| {
            tracing::warn!(%error, "could not find the daemon's executable; it cannot upgrade or restart in place");
        })
        .ok()
        .map(Arc::new);
    paths.prepare_runtime_dir()?;
    // Read once: changing the filter or the sink needs a daemon restart.
    let filter = std::env::var("RHO_LOG")
        .ok()
        .filter(|filter| !filter.trim().is_empty())
        .unwrap_or_else(|| "info".to_owned());
    let journald = std::env::var_os("JOURNAL_STREAM")
        .is_some_and(|stream| daemon::stderr_is_journal_stream(&stream));
    let sink = if journald {
        LogSink::Journal
    } else {
        let file = paths
            .open_log_file()
            .with_context(|| format!("could not open daemon log {}", paths.log_file.display()))?;
        LogSink::File(file)
    };
    logging::install(&filter, sink);
    // Every rho process refuses a store it does not know rather than pick
    // one, so the daemon does before serving anyone. An auto-spawned
    // daemon's stderr goes nowhere, so the log says why it stopped.
    let store_environment = crate::credential_store::environment_backend()
        .context("RHO_CREDENTIAL_STORE in the daemon's environment")
        .inspect_err(|error| tracing::error!(error = %format!("{error:#}"), "not starting"))?;
    // Before #1779 these set every session's trust; a unit that still
    // carries them would otherwise lose it without a word.
    if crate::workspace::ProjectTrusts::of_this_process()
        != crate::workspace::ProjectTrusts::UNTRUSTED
    {
        tracing::warn!(
            "RHO_TRUST_PROJECT_* in the daemon's environment grants nothing: each session takes the project trust of the client that opens it"
        );
    }
    // Likewise for credentials: before #1779 a key here signed every session
    // in. Custom providers' variables are known only once a config is read,
    // so this names the built-in ones, any `RHO_*_API_KEY`, and the ones
    // no provider owns.
    let credentials: Vec<String> = rho_providers::credential_env_vars()
        .into_iter()
        .chain(["GITHUB_TOKEN", "GH_TOKEN", "CODEX_ACCOUNT_ID"].map(str::to_owned))
        .filter(|variable| std::env::var_os(variable).is_some())
        .collect();
    if !credentials.is_empty() {
        tracing::warn!(
            variables = credentials.join(", "),
            "credential variables in the daemon's environment sign in no session: each session reads those of the client that opens it, else the credential store"
        );
    }
    // A panicking request handler is answered with an internal error, and an
    // auto-spawned daemon's stderr goes nowhere, so the log is the only place
    // the panic can surface.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "daemon panicked");
        default_hook(info);
    }));

    let (lock, inherited) = match handoff {
        None => (daemon::acquire_lock(paths)?, None),
        Some(handoff) => daemon::take_handoff(paths, handoff)
            .context("could not take over the restarted daemon's lock and socket")?,
    };
    // Listen before the socket exists so an early request to stop still
    // shuts down cleanly.
    let mut stops = StopRequests::listen(paths)?;
    // Everything the host needs is open before the socket is published, so
    // a client never reaches a daemon that cannot serve it.
    open_credential_store();
    let jobs = JobRecords::open(&lock, &paths.state_dir).with_context(|| {
        format!(
            "could not open the daemon's job records in {}",
            paths.state_dir.display()
        )
    })?;
    let bound = match inherited {
        None => daemon::bind(paths, lock)?,
        Some(socket) => daemon::rebind(paths, lock, socket)?,
    };
    if !journald {
        eprintln!(
            "rho daemon running (pid {}); logs: {}",
            std::process::id(),
            paths.log_file.display()
        );
    }

    let shutdown = Shutdown::default();
    let host = Arc::new(DaemonHost::new(
        jobs,
        &paths.home,
        paths.socket.clone(),
        bound.started_at(),
        shutdown.clone(),
        store_environment,
    ));
    let unloads = {
        let host = Arc::clone(&host);
        tokio::spawn(async move { host.unload_idle_sessions(unload_idle_after).await })
    };
    let upgrades = executable.clone().map(|executable| {
        let host = Arc::clone(&host);
        tokio::spawn(async move { host.upgrade_when_replaced(&executable).await })
    });
    let on_signal = Arc::clone(&host);
    // The first signal asks for a graceful shutdown. Shutdown waits for every
    // request already read and for the turns under way to save, so a signal
    // that arrives once it is under way, whether a signal or `rho daemon
    // stop` began it, forces the exit instead.
    let signal_task = tokio::spawn(async move {
        loop {
            let signal = stops.recv().await;
            if on_signal.stopping() {
                tracing::warn!(%signal, "exiting without waiting for in-flight requests");
                std::process::exit(1);
            }
            tracing::info!(%signal, "shutting down on signal");
            on_signal.begin_stop();
        }
    });
    // `stop_requested` is when the stop was asked for, whether by `rho
    // daemon stop`, `rho daemon restart`, an upgrade, or a signal: the turns'
    // drain budget runs from then, not from when the requests in flight
    // finished.
    let (served, stop_requested) = tokio::join!(
        async {
            let served = bound.serve(Arc::clone(&host), shutdown.clone()).await;
            // Serving ends only once the stop was asked for; asking here
            // keeps the join from hanging if that ever changes.
            if !shutdown.is_requested() {
                shutdown.request(Ending::Stop, |_| {});
            }
            served
        },
        async {
            shutdown.requested().await;
            tokio::time::Instant::now()
        },
    );
    // Still holding the lock, so no other daemon starts, and `rho daemon
    // stop` keeps waiting, until the turns are saved and recorded.
    // Also waits for a session the idle unload is tearing down.
    let drained = host.stop_sessions(stop_requested).await;
    unloads.abort();
    if let Some(upgrades) = upgrades {
        upgrades.abort();
    }
    signal_task.abort();
    // A cancelled task resolves only once its future is dropped, and with
    // it what the daemon listens for stops on. A successor that started
    // before then could find that still taken.
    let _ = signal_task.await;
    match (served, shutdown.ending(), executable) {
        (Served::Restarting(restarting), Ending::Restart, Some(executable)) if drained => {
            let args = |handoff| rerun_args(unload_idle_after, handoff);
            restarting
                .relaunch(&executable, args)
                .context("could not re-execute the rho daemon")
        }
        // A turn stuck past the drain budget holds processes only exiting
        // ends, as a stop does; the next client starts a new daemon.
        (served, ..) => Ok(served.release()?),
    }
}

/// The arguments that run `rho daemon run` again in this daemon's place, as
/// it ran, with the `handoff` token that passes the new run the daemon's
/// lock and socket where the platform's restart keeps them.
fn rerun_args(unload_idle_after: Duration, handoff: Option<String>) -> Vec<OsString> {
    let unload_idle_after = humantime::format_duration(unload_idle_after).to_string();
    let run = ["daemon", "run", "--unload-idle-after"].map(str::to_owned);
    let handoff = handoff
        .into_iter()
        .flat_map(|handoff| ["--exec-handoff".to_owned(), handoff]);
    run.into_iter()
        .chain([unload_idle_after])
        .chain(handoff)
        .map(OsString::from)
        .collect()
}

/// Opens the credential store every session's provider reads as the
/// daemon starts, like every rho process; a session open and a sign-in
/// open the one config selects then.
/// A config that does not load is logged, and sessions then fail to open
/// with the same error.
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

/// `rho daemon start`: connects to the running daemon, or starts one and
/// waits until it answers. Idempotent, so a daemon that already runs counts
/// as started.
async fn start() -> anyhow::Result<()> {
    let connection = connect().await?;
    println!(
        "rho daemon is running (pid {})",
        connection.hello.daemon.pid
    );
    Ok(())
}

async fn status(paths: &DaemonPaths, json: bool) -> anyhow::Result<()> {
    let client = daemon::connect(paths, ConnectAs::Command.hello())
        .await?
        .client;
    let status = client.request::<methods::Status>(&StatusParams {}).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        print!("{}", status_text(&status));
    }
    Ok(())
}

fn status_text(status: &DaemonStatus) -> String {
    let DaemonStatus {
        pid,
        version,
        protocol,
        uptime_secs,
        jobs,
        socket,
        credential_store,
        upgrade,
        started_at: _,
    } = status;
    let uptime = humantime::format_duration(Duration::from_secs(*uptime_secs));
    let upgrade = match upgrade {
        None => String::new(),
        Some(PendingUpgrade {
            version,
            waiting_for,
        }) => {
            format!("\n  upgrade      pending to {version} ({waiting_for} in progress)")
        }
    };
    let (store, chosen) = crate::credential_store::wire::words(credential_store);
    let reach = match &credential_store.unreachable {
        None => String::new(),
        Some(reason) => format!("\n  unreachable  {reason}"),
    };
    format!(
        "rho daemon is running\n  pid          {pid}\n  version      {version}\n  protocol     {protocol}\n  uptime       {uptime}\n  jobs         {jobs}{upgrade}\n  socket       {}\n  credentials  {store} store, chosen {chosen}{reach}\n",
        socket.display()
    )
}

/// `rho daemon stop`: asks the daemon to stop and waits until it exited.
pub(super) async fn stop(paths: &DaemonPaths) -> anyhow::Result<()> {
    let pid = match daemon::connect(paths, ConnectAs::Command.hello()).await {
        Ok(connection) => {
            connection
                .client
                .request::<methods::Stop>(&StopParams {})
                .await?;
            connection.hello.daemon.pid
        }
        // An incompatible daemon cannot take requests. Asking it outside the
        // protocol triggers the same graceful shutdown, so the
        // version-mismatch advice to run `rho daemon restart`, which stops
        // it this way, still works.
        Err(daemon::ConnectError::Handshake(
            ClientError::VersionMismatch(_) | ClientError::UnreadableHello(_),
        )) => daemon::terminate_recorded_daemon(paths)?.ok_or(daemon::ConnectError::NotRunning)?,
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

/// `rho daemon restart`: asks the daemon to re-execute in place, then waits
/// until the restarted daemon answers. A daemon of an incompatible protocol
/// cannot take the request, so it is stopped and a new one started, as is
/// one when none runs. So is a restarted daemon older than this `rho`, or
/// of another build: its executable is not the one this `rho` was
/// installed as, as when an installer switched a symlink to a new version's
/// directory, or a rebuilt `rho` sits at another path.
async fn restart(paths: &DaemonPaths) -> anyhow::Result<()> {
    let daemon::Connection {
        client,
        mut notifications,
        hello,
    } = match daemon::connect(paths, ConnectAs::Command.hello()).await {
        Ok(connection) => connection,
        Err(daemon::ConnectError::NotRunning) => {
            let started = connect().await?;
            println!(
                "rho daemon was not running; started it (pid {})",
                started.hello.daemon.pid
            );
            return Ok(());
        }
        Err(daemon::ConnectError::Handshake(
            ClientError::VersionMismatch(_) | ClientError::UnreadableHello(_),
        )) => {
            stop(paths).await?;
            let started = connect().await?;
            println!("rho daemon started (pid {})", started.hello.daemon.pid);
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    client
        .request::<methods::Restart>(&RestartParams {})
        .await?;
    let pid = hello.daemon.pid;
    let restarted = tokio::time::timeout(DAEMON_RESTART_TIMEOUT, async {
        // The daemon closes this connection as it restarts. The restarted
        // daemon answers on the same socket.
        while notifications.recv().await.is_some() {}
        drop(client);
        daemon::connect_restarting(paths, ConnectAs::Command.hello()).await
    })
    .await;
    match restarted {
        Ok(Ok(restarted)) => {
            let daemon::Connection { hello, .. } = restarted;
            let daemon = hello.daemon;
            let still = if crate::update::version_is_newer(env!("CARGO_PKG_VERSION"), &daemon.version)
            {
                Some(daemon.version.clone())
            } else {
                hello.skew.map(|_| format!("another build ({})", daemon.build))
            };
            if let Some(still) = still {
                stop(paths).await?;
                let started = connect().await?;
                println!(
                    "rho daemon restarted as this rho (pid {}, version {}); it was still {still}",
                    started.hello.daemon.pid, started.hello.daemon.version,
                );
                return Ok(());
            }
            println!(
                "rho daemon restarted (pid {}, version {})",
                daemon.pid, daemon.version
            );
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

#[cfg(test)]
#[path = "daemon_cli_tests.rs"]
mod tests;
