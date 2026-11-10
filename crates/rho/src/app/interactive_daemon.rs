//! Opens the daemon session a `RHO_TUI_DAEMON=1` launch follows (#1667),
//! from the CLI flags the in-process path would read.
//!
//! The daemon reads config for the session's workspace itself, so the
//! flags that override this process's settings (`--provider`, `--model`,
//! `--reasoning`, `--permission-mode`) are sent as changes to the session
//! once it is open, and one the daemon refuses fails the launch. Flags with
//! nothing in the daemon to change yet (`--config`, `--auth`, `--no-save`)
//! are refused rather than ignored. `--no-tools`, `--no-system-prompt`, and
//! `--no-subagents` leave the same parts out of the daemon session.

use std::path::Path;

use crate::{added_dirs::AddedDirs, cli::Cli, config::Config, tui};

use super::StartupSession;

#[cfg(not(unix))]
pub(super) async fn open(
    _cli: &Cli,
    _config: &Config,
    _startup: &StartupSession<'_>,
    _cwd: &Path,
    _added_dirs: &AddedDirs,
) -> anyhow::Result<tui::DaemonSession> {
    anyhow::bail!(super::super::daemon_cli::UNSUPPORTED)
}

/// Opens or resumes the session `startup` names in the daemon, which starts
/// when none runs, and attaches to it. `config` is this launch's, with the
/// CLI's overrides applied.
#[cfg(unix)]
pub(super) async fn open(
    cli: &Cli,
    config: &Config,
    startup: &StartupSession<'_>,
    cwd: &Path,
    added_dirs: &AddedDirs,
) -> anyhow::Result<tui::DaemonSession> {
    use rho_protocol::{
        ChildThreads, ModelRef, SessionChange, SessionId, SessionPart, SessionSetup,
    };

    use crate::{app::session_options::reasoning_to_wire, session::Session};

    let Cli {
        provider,
        model,
        config: config_path,
        auth,
        no_system_prompt,
        no_tools,
        no_subagents,
        agent,
        reasoning,
        permission_mode,
        add_dirs: _,
        // Startup already wrote the overrides to the config the daemon reads.
        save: _,
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
        ("--config", config_path.is_some()),
        ("--auth", auth.is_some()),
        // The daemon saves every session that runs a turn.
        ("--no-save", *no_save),
    ];
    if let Some((flag, _)) = refused.iter().find(|(_, set)| *set) {
        anyhow::bail!("{flag} is not supported with RHO_TUI_DAEMON=1 yet");
    }

    // A model first, since it decides which reasoning levels it may take.
    let mut changes = Vec::new();
    if provider.is_some() || model.is_some() {
        changes.push(SessionChange::Model {
            model: ModelRef {
                provider: config.provider.clone(),
                model: config.model.clone(),
            },
        });
    }
    if reasoning.is_some() {
        changes.push(SessionChange::Reasoning {
            level: reasoning_to_wire(config.reasoning),
        });
    }
    if permission_mode.is_some() {
        changes.push(SessionChange::PermissionMode {
            mode: config.permission_mode.into(),
        });
    }

    let without = [
        (SessionPart::Tools, *no_tools),
        (SessionPart::SystemPrompt, *no_system_prompt),
        (SessionPart::Subagents, *no_subagents),
    ]
    .into_iter()
    .filter_map(|(part, left_out)| left_out.then_some(part))
    .collect();
    let launch = tui::DaemonLaunch {
        cwd: cwd.to_path_buf(),
        agent: agent.clone(),
        setup: SessionSetup {
            additional_directories: added_dirs.as_slice().to_vec(),
            mcp_servers: Vec::new(),
            without,
            child_threads: ChildThreads::Daemon,
        },
    };

    // The picker opens over a new session, which the daemon discards
    // unused, and switches to its choice, which runs as it was saved: a
    // flag for the session has nothing to apply to yet.
    if matches!(startup, StartupSession::Picker) && !changes.is_empty() {
        anyhow::bail!(
            "--provider, --model, --reasoning, and --permission-mode need a session id with --resume"
        );
    }
    // A reopened session runs as the agent it was saved as.
    if !matches!(startup, StartupSession::Fresh) && agent.is_some() {
        anyhow::bail!("--agent with --resume or --continue is not supported with RHO_TUI_DAEMON=1");
    }
    let target = match startup {
        StartupSession::Fresh | StartupSession::Picker => tui::DaemonTarget::New,
        // Resolve as in process, but never take the writer lease from
        // the daemon that may still be running this session's turn.
        StartupSession::ById(prefix) => {
            tui::DaemonTarget::Resume(SessionId::new(Session::resolve_id(cwd, prefix)?))
        }
        StartupSession::Latest(target) => {
            tui::DaemonTarget::Resume(SessionId::new(target.id.clone()))
        }
    };
    tui::DaemonSession::open(launch, target, tui::InitialSettings::Required(changes)).await
}
