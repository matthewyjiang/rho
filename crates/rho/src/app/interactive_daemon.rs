//! Opens the daemon session a `RHO_TUI_DAEMON=1` launch follows (#1667),
//! from the CLI flags the in-process path would read.
//!
//! The daemon reads config for the session's workspace itself, so a flag
//! that overrides this process's config (`--provider`, `--model`, ...) has
//! nothing to override and is refused rather than ignored. Interactive
//! controls configure the attached session through `rho/session/configure`;
//! startup overrides remain unported. `--no-tools`, `--no-system-prompt`,
//! and `--no-subagents` leave the same parts out of the daemon session.

use std::path::Path;

use crate::{added_dirs::AddedDirs, cli::Cli, tui};

use super::StartupSession;

#[cfg(not(unix))]
pub(super) async fn open(
    _cli: &Cli,
    _startup: &StartupSession<'_>,
    _cwd: &Path,
    _added_dirs: &AddedDirs,
) -> anyhow::Result<tui::DaemonSession> {
    anyhow::bail!(super::super::daemon_cli::UNSUPPORTED)
}

/// Opens or resumes the session `startup` names in the daemon, which starts
/// when none runs, and attaches to it.
#[cfg(unix)]
pub(super) async fn open(
    cli: &Cli,
    startup: &StartupSession<'_>,
    cwd: &Path,
    added_dirs: &AddedDirs,
) -> anyhow::Result<tui::DaemonSession> {
    use rho_protocol::{
        ChildThreads, ClientEnvironment, SessionId, SessionNewParams, SessionPart, SessionSetup,
    };

    use crate::session::Session;

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
        add_dirs: _,
        save,
        no_save,
        resume: _,
        continue_latest: _,
        // The TUI submits it from the composer, which sends it to the
        // session like any prompt.
        prompt: _,
        // `None`: interactive launches have no subcommand.
        command: _,
    } = cli;
    let refused = [
        ("--provider", provider.is_some()),
        ("--model", model.is_some()),
        ("--config", config.is_some()),
        ("--auth", auth.is_some()),
        ("--reasoning", reasoning.is_some()),
        ("--permission-mode", permission_mode.is_some()),
        ("--save", *save),
        // The daemon saves every session that runs a turn.
        ("--no-save", *no_save),
    ];
    if let Some((flag, _)) = refused.iter().find(|(_, set)| *set) {
        anyhow::bail!("{flag} is not supported with RHO_TUI_DAEMON=1 yet");
    }

    let without = [
        (SessionPart::Tools, *no_tools),
        (SessionPart::SystemPrompt, *no_system_prompt),
        (SessionPart::Subagents, *no_subagents),
    ]
    .into_iter()
    .filter_map(|(part, left_out)| left_out.then_some(part))
    .collect();
    let setup = SessionSetup {
        additional_directories: added_dirs.as_slice().to_vec(),
        mcp_servers: Vec::new(),
        without,
        child_threads: ChildThreads::Daemon,
    };

    let target = match startup {
        StartupSession::Fresh => tui::DaemonTarget::New(SessionNewParams {
            cwd: cwd.to_path_buf(),
            environment: ClientEnvironment::of_this_process(),
            agent: agent.clone(),
            setup,
            parent: None,
            grant: None,
        }),
        // The resume picker lists sessions from this process's store, which
        // does not hold the daemon's yet.
        StartupSession::Picker => {
            anyhow::bail!("--resume needs a session id with RHO_TUI_DAEMON=1 for now")
        }
        StartupSession::ById(_) | StartupSession::Latest(_) if agent.is_some() => {
            // A reopened session runs as the agent it was saved as.
            anyhow::bail!(
                "--agent with --resume or --continue is not supported with RHO_TUI_DAEMON=1"
            )
        }
        // Resolve as in process, but never take the writer lease from
        // the daemon that may still be running this session's turn.
        StartupSession::ById(prefix) => tui::DaemonTarget::Resume {
            session: SessionId::new(Session::resolve_id(cwd, prefix)?),
            setup,
        },
        StartupSession::Latest(target) => tui::DaemonTarget::Resume {
            session: SessionId::new(target.id.clone()),
            setup,
        },
    };
    tui::DaemonSession::open(target).await
}
