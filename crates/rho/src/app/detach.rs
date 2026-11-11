//! `rho run --detach`: starts a prompt's turn in the rho daemon, starting
//! the daemon when none runs, and exits once the daemon has saved the prompt
//! (#1669). It prints the session's id on stdout; the turn runs on with no
//! client attached, and `rho attach <ID>` follows and answers it.
//!
//! The daemon opens the session with the workspace's config, as `rho`
//! started there would, as the `--agent` given and with the `--add-dir` and
//! `--no-*` flags as its setup (see [`detach_flags`]), which the session
//! keeps when the daemon reopens it; `cli_config::validate` refuses the
//! flags that would change the config, before anything here runs.

use std::{collections::BTreeSet, path::PathBuf, time::Duration};

use rho_protocol::SessionPart;
#[cfg(unix)]
use rho_protocol::{ChildThreads, SessionNewParams, SessionSetup};

#[cfg(unix)]
use super::automation::AutomationExit;
use crate::cli::Cli;

/// The flags a detached run passes to the daemon. Off unix only
/// `cli_config::validate` sorts them, and nothing reads them (#1665).
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) struct Passed<'a> {
    /// `--agent`, the agent the session runs as.
    agent: Option<&'a str>,
    /// `--add-dir`, as given.
    add_dirs: &'a [PathBuf],
    /// The parts the `--no-*` flags leave out of the session.
    without: BTreeSet<SessionPart>,
}

/// Sorts `cli`'s flags into those a detached run passes to the daemon, or
/// names those set that it refuses, since the daemon opens the session with
/// the workspace's config (#1669). Every field is named, so a new flag does
/// not compile until someone decides whether a detached run takes it.
pub(super) fn detach_flags(cli: &Cli) -> Result<Passed<'_>, Vec<&'static str>> {
    let Cli {
        provider,
        model,
        config,
        auth,
        no_system_prompt,
        no_tools,
        no_subagents,
        agent,
        reasoning,
        permission_mode,
        add_dirs,
        save,
        // Refused in `validate` with any command.
        no_save: _,
        resume: _,
        continue_latest: _,
        prompt: _,
        command: _,
    } = cli;
    let refused: Vec<_> = [
        ("--provider", provider.is_some()),
        ("--model", model.is_some()),
        ("--config", config.is_some()),
        ("--auth", auth.is_some()),
        ("--reasoning", reasoning.is_some()),
        ("--permission-mode", permission_mode.is_some()),
        ("--save", *save),
    ]
    .into_iter()
    .filter_map(|(name, set)| set.then_some(name))
    .collect();
    if !refused.is_empty() {
        return Err(refused);
    }
    let without = [
        (SessionPart::Tools, *no_tools),
        (SessionPart::SystemPrompt, *no_system_prompt),
        (SessionPart::Subagents, *no_subagents),
    ]
    .into_iter()
    .filter_map(|(part, set)| set.then_some(part))
    .collect();
    Ok(Passed {
        agent: agent.as_deref(),
        add_dirs,
        without,
    })
}

/// Why a detached run refuses the `refused` flags.
pub(super) fn refusal(refused: &[&str]) -> String {
    format!(
        "rho run --detach does not take {}: the daemon opens the session with the workspace's config",
        refused.join(", ")
    )
}

/// The session `rho run --detach` asks the daemon for with `cli`'s flags,
/// in the current directory, with `environment`. `--add-dir` resolves as it
/// does for a foreground run, so `~` and relative paths mean the same; a
/// bad one is a configuration error.
#[cfg(unix)]
fn session_new(
    cli: &Cli,
    environment: rho_protocol::ClientEnvironment,
) -> Result<SessionNewParams, AutomationExit> {
    let passed =
        detach_flags(cli).map_err(|refused| AutomationExit::configuration(refusal(&refused)))?;
    let cwd = std::env::current_dir().map_err(|error| {
        AutomationExit::other(format!("could not read the current directory: {error}"))
    })?;
    let added = crate::added_dirs::launch_added_dirs(passed.add_dirs, &cwd)
        .map_err(|error| AutomationExit::configuration(format!("{error:#}")))?;
    Ok(SessionNewParams {
        cwd,
        environment,
        agent: passed.agent.map(str::to_owned),
        setup: SessionSetup {
            additional_directories: added.as_slice().to_vec(),
            mcp_servers: Vec::new(),
            without: passed.without,
            child_threads: ChildThreads::Daemon,
            idle_wake: rho_protocol::IdleWake::Never,
            // A run without a login exits as an authentication failure.
            sign_in: rho_protocol::SignIn::Required,
        },
        parent: None,
        grant: None,
    })
}

/// Starts the turn of the prompt `rho run` takes from `parts` and, with
/// `read_stdin`, stdin, in a new daemon session `cli`'s flags describe,
/// timed out after `timeout`, and prints the session's id.
#[cfg(unix)]
pub(super) async fn run(
    cli: &Cli,
    parts: Vec<String>,
    read_stdin: bool,
    timeout: Option<Duration>,
) -> anyhow::Result<()> {
    // The session's processes run with this environment, not the daemon's
    // (#1659).
    let new = session_new(cli, rho_protocol::ClientEnvironment::of_this_process())?;
    start(new, parts, read_stdin, timeout).await
}

/// Off unix there is no daemon (#1665), which a detached run says before it
/// reads its prompt or builds its request.
#[cfg(not(unix))]
pub(super) async fn run(
    _cli: &Cli,
    _parts: Vec<String>,
    _read_stdin: bool,
    _timeout: Option<Duration>,
) -> anyhow::Result<()> {
    anyhow::bail!(super::daemon_cli::UNSUPPORTED)
}

/// [`run`] once its session's params are known: opens the session, starts
/// its turn, and prints its id.
#[cfg(unix)]
async fn start(
    new: SessionNewParams,
    parts: Vec<String>,
    read_stdin: bool,
    timeout: Option<Duration>,
) -> anyhow::Result<()> {
    use std::io::{self, Write};

    use rho_protocol::{
        methods::{SessionNew, SessionPrompt},
        ContentBlock, ErrorCode, SessionPromptParams,
    };

    use super::{automation, daemon_cli};

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
    let environment = new.environment.clone();
    let client = daemon_cli::connect()
        .await
        .map_err(|error| AutomationExit::other(error.to_string()))?
        .client;
    let session = client
        .request::<SessionNew>(&new)
        .await
        .map_err(|error| {
            let message = format!("could not open a daemon session: {error}");
            match error.error_code() {
                // An agent or setup the session cannot open with, or an
                // environment or cwd the daemon refuses.
                Some(ErrorCode::SessionOpenFailed | ErrorCode::InvalidParams) => {
                    AutomationExit::configuration(message)
                }
                // As a turn that failed on its credentials exits.
                Some(ErrorCode::SignInRequired) => AutomationExit::authentication(message),
                _ => AutomationExit::other(message),
            }
        })?
        .session;
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            content: vec![ContentBlock::Text { text }],
            timeout_ms,
            environment,
            idempotency_key: None,
            grant: None,
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
