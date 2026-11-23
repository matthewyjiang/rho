//! CLI arguments for `rho daemon`.

use std::{net::SocketAddr, time::Duration};

use clap::Subcommand;

use crate::app::automation_protocol::parse_duration;

/// The shortest `rho daemon run --unload-idle-after`. Shorter, a session
/// could unload between one client's consecutive requests, as a reopen
/// takes up to 41 ms measured (`UNLOAD_IDLE_AFTER` has the receipt), and
/// the sweep, which runs once per period, would spin.
const MIN_UNLOAD_IDLE_AFTER: Duration = Duration::from_millis(100);

fn parse_unload_idle_after(value: &str) -> Result<Duration, String> {
    let duration = parse_duration(value)?;
    if duration < MIN_UNLOAD_IDLE_AFTER {
        return Err(format!(
            "invalid duration '{value}': must be at least {}",
            humantime::format_duration(MIN_UNLOAD_IDLE_AFTER)
        ));
    }
    Ok(duration)
}

#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
pub enum DaemonCommand {
    /// Run the daemon in the foreground until stopped. The default.
    Run {
        /// Unload a session once it has sat idle this long: no turn under
        /// way and no client attached. Unloading closes its MCP servers and
        /// lets other rho processes open it; the next prompt or attach
        /// reopens it. A session that saved nothing is discarded instead.
        /// Defaults to 60s; at least 100ms.
        #[arg(long, value_name = "DURATION", value_parser = parse_unload_idle_after)]
        unload_idle_after: Option<Duration>,
        /// The lock and socket descriptors a daemon that re-executes itself
        /// in place hands the new binary. Only the daemon passes it.
        #[arg(long, value_name = "LOCK:SOCKET", hide = true)]
        exec_handoff: Option<String>,
    },
    /// Start the daemon in the background unless it is already running,
    /// and wait until it answers.
    ///
    /// Exits 0 once a daemon runs, including one that already did. Waits
    /// for a daemon that is stopping to exit before starting a new one.
    Start,
    /// Show the running daemon's pid, version, uptime, socket, and any
    /// upgrade it waits to run.
    ///
    /// Never starts a daemon. Exits with status 1 when none is running.
    Status {
        /// Print one JSON document instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Stop the running daemon and wait for it to exit.
    ///
    /// Exits with status 1 when none is running.
    Stop,
    /// Restart the running daemon in place and wait until it answers again.
    ///
    /// The daemon keeps its pid, socket, and lock, so clients reconnect to
    /// it rather than start another, and re-executes its executable, which
    /// runs an installed upgrade at once. Turns under way are interrupted;
    /// a TUI session's turn with no time limit continues once the TUI
    /// attaches to its session again. Starts the daemon when none is
    /// running.
    Restart,
    /// Expose the local daemon's `rho-protocol` over HTTP WebSockets (`GET /ws`)
    /// so clients on hosts that cannot use SSH can reach it.
    ///
    /// Binds a loopback or private/overlay network address; every upgrade
    /// requires a bearer token from `rho daemon token rotate`.
    Serve(DaemonServeArgs),
    /// Manage the bearer tokens `rho daemon serve --http` accepts.
    Token {
        #[command(subcommand)]
        command: DaemonTokenCommand,
    },
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct DaemonServeArgs {
    /// Serve `rho-protocol` over HTTP WebSocket (`GET /ws`) on this address:
    /// a loopback address, such as 127.0.0.1:8766, or this machine's address
    /// on a private network, such as a NetBird or Tailscale peer's
    /// 100.x.y.z:8766, where traffic travels as plain HTTP/WS.
    #[arg(long, value_name = "ADDR")]
    pub http: SocketAddr,
    /// Also accept this `Host` header, such as a tunnel's public hostname,
    /// on any port unless it names one (rho.example.com:443). May be
    /// repeated.
    #[arg(long, value_name = "HOST")]
    pub allow_host: Vec<String>,
    /// Accept WebSocket upgrades from this browser origin, such as
    /// https://app.example.com. Any other `Origin` is refused. Browsers
    /// cannot send the bearer token themselves, so they need a proxy that
    /// adds `Authorization`. May be repeated.
    #[arg(long, value_name = "ORIGIN")]
    pub allow_origin: Vec<String>,
    /// Internal: the listening socket a server that upgraded in place
    /// handed this one.
    #[arg(long, value_name = "FD", hide = true)]
    pub listener_fd: Option<i32>,
}

#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
pub enum DaemonTokenCommand {
    /// Create token NAME, or give it a new secret, and print the secret once.
    /// The old secret stops working at once.
    ///
    /// A daemon HTTP token grants full user-level daemon protocol access
    /// (opening sessions in any workspace, answering approvals, and editing
    /// config). Treat it like SSH access.
    Rotate {
        /// The token's name, which `list` and logs show.
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// List daemon HTTP tokens by name.
    List {
        /// Print one JSON document instead of text rows.
        #[arg(long)]
        json: bool,
    },
    /// Delete token NAME; new connections presenting it are refused at once.
    /// Open WebSockets stay connected until they close or the bridge
    /// restarts.
    Revoke {
        #[arg(value_name = "NAME")]
        name: String,
    },
}
