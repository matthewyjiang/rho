//! `rho run --detach`: starts a prompt's turn in the rho daemon, starting
//! the daemon when none runs, and exits once the daemon has saved the prompt
//! (#1669). It prints the session's id on stdout; the turn runs on with no
//! client attached, and `rho attach <ID>` follows and answers it.
//!
//! The daemon opens the session with the workspace's config, as `rho`
//! started there would; `cli_config::validate` refuses the flags that would
//! change it, before anything here runs.

use std::time::Duration;

#[cfg(not(unix))]
pub(super) async fn run(
    _parts: Vec<String>,
    _read_stdin: bool,
    _timeout: Option<Duration>,
) -> anyhow::Result<()> {
    anyhow::bail!(super::daemon_cli::UNSUPPORTED)
}

/// Starts the turn of the prompt `rho run` takes from `parts` and, with
/// `read_stdin`, stdin, in a new daemon session in the current directory,
/// timed out after `timeout`, and prints the session's id.
#[cfg(unix)]
pub(super) async fn run(
    parts: Vec<String>,
    read_stdin: bool,
    timeout: Option<Duration>,
) -> anyhow::Result<()> {
    use std::io::{self, Write};

    use rho_protocol::{
        methods::{SessionNew, SessionPrompt},
        Environment, ErrorCode, SessionNewParams, SessionPromptParams,
    };

    use super::{
        automation::{self, AutomationExit},
        daemon_cli,
    };
    use crate::daemon::{self, cli_client_info, ProcessSpawner};

    let timeout_ms = timeout
        .map(|timeout| {
            u64::try_from(timeout.as_millis()).map_err(|_| {
                AutomationExit::configuration(format!(
                    "--timeout {} is over the daemon's limit of {} ms",
                    humantime::format_duration(timeout),
                    u64::MAX
                ))
            })
        })
        .transpose()?;
    let text = automation::prompt_from_stdin(parts, read_stdin)?;
    // The session's processes run with this environment, not the daemon's
    // (#1659).
    let environment = Environment::of_this_process();
    let cwd = std::env::current_dir().map_err(|error| {
        AutomationExit::other(format!("could not read the current directory: {error}"))
    })?;
    let paths = daemon_cli::paths().map_err(|error| AutomationExit::other(format!("{error:#}")))?;
    let spawner = ProcessSpawner::current_exe().map_err(|error| {
        AutomationExit::other(format!("could not find the rho executable: {error}"))
    })?;
    let client = daemon::connect_or_spawn(&paths, cli_client_info(), &spawner)
        .await
        .map_err(|error| {
            AutomationExit::other(format!("could not reach the rho daemon: {error:#}"))
        })?
        .client;
    let session = client
        .request::<SessionNew>(&SessionNewParams {
            cwd,
            environment: environment.clone(),
        })
        .await
        .map_err(|error| {
            let message = format!("could not open a daemon session: {error}");
            match error.error_code() {
                Some(ErrorCode::SessionOpenFailed) => AutomationExit::configuration(message),
                _ => AutomationExit::other(message),
            }
        })?
        .session;
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            text,
            timeout_ms,
            environment,
        })
        .await
        .map_err(|error| {
            // A turn that ended before it saved the prompt exits as one
            // that ended later would under `rho attach`.
            match error
                .turn_ended()
                .map(|outcome| automation::outcome_exit(&outcome))
            {
                Some(Err(exit)) => exit,
                _ => AutomationExit::other(format!("could not start the turn: {error}")),
            }
        })?;
    // The turn runs on either way, so a failed write still names it.
    writeln!(io::stdout(), "{session}").map_err(|error| {
        AutomationExit::other(format!(
            "could not write the session id {session}: {error}; the turn runs on in the daemon"
        ))
    })?;
    // Only a hint: the run already succeeded.
    let _ = writeln!(
        io::stderr(),
        "rho: the turn runs in the daemon; follow it with `rho attach {session}`"
    );
    Ok(())
}
