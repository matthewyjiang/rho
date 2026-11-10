//! The single policy boundary for commands against a daemon-owned session.
//!
//! A command that still reads or mutates InteractiveRuntime must not silently
//! operate on the unused in-process session. Keep this match exhaustive: every
//! new builtin needs an intentional daemon classification.

use crate::commands::{CommandId, CommandInvocation};

use super::{App, Entry};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CommandRoute {
    Ui,
    Options,
    Prompt,
    Unavailable,
}

pub(super) fn command_route(command: CommandId) -> CommandRoute {
    match command {
        // Session state the daemon owns, changed through the session's
        // options, and settings it saves through `rho/config/set`.
        // In process, the settings commands say they need the daemon
        // (`needs_daemon`).
        CommandId::Model
        | CommandId::Permissions
        | CommandId::Fast
        | CommandId::Goal
        | CommandId::Advisor => CommandRoute::Options,
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
        // The daemon opens the session these follow next.
        | CommandId::New
        | CommandId::Resume
        | CommandId::Sessions => CommandRoute::Ui,
        // The conversation tree and rewind need daemon methods;
        // provider/auth editors and the advisor still own in-process
        // state. The /workflow hub likewise describes the wrong runtime,
        // even when opened read-only.
        CommandId::AddDir
        | CommandId::Login
        | CommandId::Logout
        | CommandId::RefreshModels
        | CommandId::Rewind
        | CommandId::Tree
        | CommandId::Context
        | CommandId::Hooks
        | CommandId::Codemode
        | CommandId::Workflow => CommandRoute::Unavailable,
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

    pub(super) fn refuse_daemon_command(&mut self, invocation: &CommandInvocation) -> bool {
        self.refuse_daemon_action(invocation.id, &invocation.name)
    }

    /// Also used by shortcuts and startup navigation that bypass slash dispatch.
    pub(super) fn refuse_daemon_action(&mut self, id: CommandId, name: &str) -> bool {
        if self.info.session.daemon.is_none() || command_route(id) != CommandRoute::Unavailable {
            return false;
        }
        self.daemon_command_unavailable(name);
        true
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

    /// The mirror of [`Self::daemon_command_unavailable`]: `/name` changes
    /// settings, which only the daemon saves.
    pub(super) fn needs_daemon(&mut self, name: &str) {
        let notice = if cfg!(unix) {
            format!("/{name} needs the rho daemon: start rho with RHO_TUI_DAEMON=1")
        } else {
            format!("/{name} needs the rho daemon, which does not run on this platform yet; edit the settings files instead")
        };
        self.insert_entry(&Entry::Notice(notice));
        self.set_status(format!("/{name} needs the rho daemon"));
    }

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
