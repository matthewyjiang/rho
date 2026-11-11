//! The single policy boundary for commands against a daemon-owned session.
//!
//! A command must not silently operate on the unused in-process session, so
//! each says how it reaches the daemon's. Keep this match exhaustive: every
//! new builtin needs an intentional daemon classification.

use crate::commands::CommandId;

use super::{App, Entry};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CommandRoute {
    Ui,
    Options,
    Prompt,
}

pub(super) fn command_route(command: CommandId) -> CommandRoute {
    match command {
        // Session state the daemon owns, changed through the session's
        // options, and settings it saves through `rho/config/set`.
        // In process, `/advisor` says it needs the daemon (`needs_daemon`).
        CommandId::Model
        | CommandId::Permissions
        | CommandId::Fast
        | CommandId::Goal
        | CommandId::Advisor
        | CommandId::Codemode => CommandRoute::Options,
        // These commands are implemented by the daemon's prompt pipeline.
        CommandId::Init | CommandId::Compact => CommandRoute::Prompt,
        CommandId::Exit
        | CommandId::Theme
        | CommandId::Help
        | CommandId::Copy
        | CommandId::Changelog
        | CommandId::Diff
        | CommandId::Spend
        | CommandId::Todo
        | CommandId::Config
        | CommandId::Skills
        | CommandId::Mcp
        // Read from this process's disk and config, which the daemon
        // shares, written through the session (`/title`, `/remember`), or
        // run beside it on a copy of its transcript (`/side`).
        | CommandId::Info
        | CommandId::Doctor
        | CommandId::Limits
        | CommandId::Export
        | CommandId::Title
        | CommandId::Remember
        | CommandId::Side
        // Reads run artifacts the daemon writes to the shared rho home.
        | CommandId::Attach
        // Agent definitions are files; the daemon is told when they change,
        // and the creator runs as a prompt in its session.
        | CommandId::Agents
        | CommandId::CreateAgent
        // Asked of the daemon's session: its reports, the directories its
        // tools reach, and the model lists its options are built from.
        | CommandId::Context
        | CommandId::Hooks
        | CommandId::AddDir
        | CommandId::RefreshModels
        // The pickers and prompts are this screen's; the daemon checks and
        // saves the credential, runs the login, and signs out.
        | CommandId::Login
        | CommandId::Logout
        // The daemon opens the session these follow next.
        | CommandId::New
        | CommandId::Resume
        | CommandId::Sessions
        // The hub lists the workspace's workflow store, which the daemon
        // shares; a launch is a prompt the daemon runs.
        | CommandId::Workflow
        // Read from the session's file, which the daemon shares; the
        // daemon moves the session to the node chosen.
        | CommandId::Tree
        // Checkpoints are read from the session's folder too; the daemon
        // restores the files and the conversation.
        | CommandId::Rewind => CommandRoute::Ui,
    }
}

impl App {
    pub(super) fn daemon_command_choices(&self) -> Vec<super::CommandChoice> {
        let Some(prefix) = crate::commands::command_prefix(self.input_ui.text()) else {
            return Vec::new();
        };
        let prefix = prefix.trim_start_matches('/').to_ascii_lowercase();
        let mut choices: Vec<_> = crate::commands::matching_commands(&prefix)
            .into_iter()
            .filter(|command| {
                matches!(
                    command_route(command.id),
                    CommandRoute::Ui | CommandRoute::Options
                )
            })
            .filter(|command| {
                command.id != CommandId::Fast
                    || self
                        .daemon_options
                        .as_ref()
                        .is_some_and(|options| options.fast_mode.is_some())
            })
            .map(|command| super::CommandChoice {
                name: command.name.into(),
                usage: command.usage.into(),
                description: command.description.into(),
                kind: super::CommandChoiceKind::Builtin(command),
            })
            .collect();
        if let Some(options) = &self.daemon_options {
            choices.extend(
                options
                    .commands
                    .iter()
                    .filter(|command| {
                        command.kind == rho_protocol::CommandKind::Slash
                            && command.name.to_ascii_lowercase().starts_with(&prefix)
                    })
                    .filter(|command| {
                        !crate::commands::COMMANDS.iter().any(|builtin| {
                            builtin.name == command.name
                                && command_route(builtin.id) != CommandRoute::Prompt
                        })
                    })
                    .map(|command| super::CommandChoice {
                        name: command.name.clone(),
                        usage: match &command.input_hint {
                            Some(hint) => format!("/{} {hint}", command.name),
                            None => format!("/{}", command.name),
                        },
                        description: command.description.clone(),
                        kind: super::CommandChoiceKind::SessionCommand,
                    }),
            );
        }
        choices
    }

    /// `/config` edits the daemon's settings; in process there is no
    /// daemon to save them (#1720).
    pub(super) fn execute_config_command(&mut self) {
        if self.info.session.daemon.is_some() {
            self.open_daemon_config();
        } else {
            self.needs_daemon("config");
        }
    }

    /// `/name` changes settings, which only the daemon saves.
    pub(super) fn needs_daemon(&mut self, name: &str) {
        let notice = if cfg!(unix) {
            format!("/{name} needs the rho daemon: start rho with RHO_TUI_DAEMON=1")
        } else {
            format!("/{name} needs the rho daemon, which does not run on this platform yet; edit the settings files instead")
        };
        self.insert_entry(&Entry::Notice(notice));
        self.set_status(format!("/{name} needs the rho daemon"));
    }

    /// Says that the daemon's session does not offer the prompt command
    /// `/name`.
    pub(super) fn daemon_command_unavailable(&mut self, name: &str) {
        let notice = format!("/{name} is not available with RHO_TUI_DAEMON yet");
        self.insert_entry(&Entry::Notice(notice.clone()));
        self.set_status(notice);
    }

    /// Dynamic commands must be sent verbatim, not expanded with the local
    /// workflow, MCP, template or skill runtime.
    pub(super) fn daemon_prompt_command_available(&self, name: &str) -> bool {
        self.daemon_options.as_ref().is_some_and(|options| {
            options.commands.iter().any(|command| {
                command.kind == rho_protocol::CommandKind::Slash
                    && command.name.eq_ignore_ascii_case(name)
            })
        })
    }
}
