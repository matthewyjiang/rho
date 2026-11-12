//! Opens the daemon session an interactive launch follows (#1667), from the
//! launch's CLI flags. A new session opens with its first prompt, so until
//! then the TUI shows the options this launch's config and agent give it.
//!
//! The daemon reads config for the session's workspace itself, so the
//! flags that override this process's settings (`--provider`, `--model`,
//! `--auth`, `--reasoning`, `--permission-mode`) are sent as changes to the
//! session once it is open, and one the daemon refuses fails the launch.
//! `--config` names a file the daemon does not read, so it is refused rather
//! than ignored. `--no-tools`, `--no-system-prompt`, `--no-subagents`, and
//! `--no-save` leave the same parts out of the daemon session.

use std::path::Path;

use crate::{added_dirs::AddedDirs, cli::Cli, config::Config, tui};

use super::{super::agent_binding::BoundAgent, StartupSession};

/// Resumes the session `startup` names in the daemon, which starts when
/// none runs, and attaches to it, or connects for a new one its first
/// prompt opens. `config` is this launch's, with the
/// CLI's overrides applied, and `bound` the agent it bound: `--agent`'s, or
/// the default.
pub(super) async fn open(
    cli: &Cli,
    config: &Config,
    bound: &BoundAgent,
    startup: &StartupSession<'_>,
    cwd: &Path,
    added_dirs: &AddedDirs,
) -> anyhow::Result<tui::DaemonSession> {
    use rho_protocol::{
        ChildThreads, IdleWake, ModelRef, SessionChange, SessionPart, SessionSetup, SignIn,
    };

    use crate::{app::session_options::reasoning_to_wire, session::Session, tui::LaunchChange};

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
    if config_path.is_some() {
        // The daemon's sessions share the config in its Rho home.
        anyhow::bail!(
            "--config is not supported in a daemon session: the daemon reads the config in its Rho home; set RHO_HOME to run on another"
        );
    }

    // A model first, since it decides which reasoning levels it may take.
    let mut changes = Vec::new();
    let launch_model = || ModelRef {
        provider: config.provider.clone(),
        model: config.model.clone(),
    };
    match (provider.is_some() || model.is_some(), auth) {
        (true, _) => changes.push(LaunchChange::Change(SessionChange::Model {
            model: launch_model(),
            auth: auth.is_some().then(|| config.auth.clone()),
            setup: true,
        })),
        // The flag may name another provider's profile, which the launch's
        // config then holds.
        (false, Some(_)) => changes.push(LaunchChange::Auth {
            auth: config.auth.clone(),
            on: launch_model(),
        }),
        (false, None) => {}
    }
    if reasoning.is_some() {
        changes.push(LaunchChange::Change(SessionChange::Reasoning {
            level: reasoning_to_wire(config.reasoning),
        }));
    }
    if permission_mode.is_some() {
        changes.push(LaunchChange::Change(SessionChange::PermissionMode {
            mode: config.permission_mode.into(),
        }));
    }

    let without = [
        (SessionPart::Tools, *no_tools),
        (SessionPart::SystemPrompt, *no_system_prompt),
        (SessionPart::Subagents, *no_subagents),
        (SessionPart::Saving, *no_save),
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
            // The TUI follows the session, so it shows the turn the daemon
            // starts when a workflow or background process finishes.
            idle_wake: IdleWake::Turn,
            // A session without a login opens signed out, and the TUI
            // offers /login.
            sign_in: SignIn::Later,
        },
        config: std::sync::Arc::new(config.clone()),
    };

    // The picker opens over a new session, which opens only if prompted,
    // and switches to its choice, which runs as it was saved: a flag for
    // the session has nothing to apply to yet.
    if matches!(startup, StartupSession::Picker) && !changes.is_empty() {
        anyhow::bail!(
            "--provider, --model, --auth, --reasoning, and --permission-mode need a session id with --resume"
        );
    }
    // A session the daemon still has loaded keeps the setup it opened
    // with, so it would go on saving while this TUI treated it as unsaved,
    // and cancel its turn on quit.
    if *no_save && matches!(startup, StartupSession::ById(_) | StartupSession::Latest(_)) {
        anyhow::bail!(
            "--no-save cannot be combined with --resume or --continue: a resumed session keeps saving"
        );
    }
    let id = match startup {
        // The flags are this launch's config, which the draft is built
        // from, so the first prompt opens the session with them.
        StartupSession::Fresh | StartupSession::Picker => {
            let options = new_options(&launch, bound).await;
            return tui::DaemonSession::draft(launch, options).await;
        }
        // Resolve the id, but never take the writer lease from the daemon
        // that may still be running this session's turn.
        StartupSession::ById(prefix) => {
            resumed(Session::resolve_target(cwd, prefix)?, agent, bound)?
        }
        StartupSession::Latest(target) => resumed(target.clone(), agent, bound)?,
    };
    tui::DaemonSession::open(launch, id, tui::InitialSettings::Required(changes)).await
}

/// The options a new session of `launch` starts with, as its config and
/// `bound`, its agent, give them: the commands it lists before its MCP
/// servers connect, as the daemon's would.
async fn new_options(
    launch: &tui::DaemonLaunch,
    bound: &BoundAgent,
) -> crate::app::session_options::DraftOptions {
    use crate::app::{
        daemon_host::DAEMON_COMMANDS,
        session_commands::CommandFiles,
        session_options::{available_auths, drafted_commands, DraftOptions},
    };

    let home = crate::paths::home_dir();
    let files = CommandFiles {
        cwd: &launch.cwd,
        home: home.as_deref(),
        project_trust: crate::workspace::ProjectTrusts::of_this_process(),
        config: &launch.config,
    };
    let commands = drafted_commands(&files, bound.rho_capabilities(), DAEMON_COMMANDS);
    DraftOptions::new(
        (*launch.config).clone(),
        available_auths(std::sync::Arc::new(
            crate::credential_store::AppCredentialStore,
        ))
        .await,
        commands,
        launch.setup.additional_directories.clone(),
    )
}

/// Resuming `session`. It runs as the agent it was saved as, so `--agent`
/// may only name that one, unchanged since.
fn resumed(
    session: crate::session::SessionTarget,
    agent: &Option<String>,
    bound: &BoundAgent,
) -> anyhow::Result<rho_protocol::SessionId> {
    if agent.is_some() {
        crate::session::Session::validate_agent_definition_identity_of(
            &session,
            bound.definition(),
        )?;
    }
    Ok(rho_protocol::SessionId::new(session.id))
}
