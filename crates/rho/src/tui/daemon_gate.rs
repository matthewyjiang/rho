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
        CommandId::Model | CommandId::Permissions | CommandId::Fast => CommandRoute::Options,
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
        | CommandId::Mcp => CommandRoute::Ui,
        // Session navigation/deletion and title writes need daemon methods;
        // config/provider/auth editors, agents, advisor and goals still own
        // in-process state. Hubs (/workflow, /attach, /side) likewise
        // describe the wrong runtime, even when opened read-only.
        CommandId::AddDir
        | CommandId::Advisor
        | CommandId::New
        | CommandId::Login
        | CommandId::Logout
        | CommandId::RefreshModels
        | CommandId::Remember
        | CommandId::Resume
        | CommandId::Rewind
        | CommandId::Sessions
        | CommandId::Tree
        | CommandId::Context
        | CommandId::Info
        | CommandId::Goal
        | CommandId::Hooks
        | CommandId::Agents
        | CommandId::Attach
        | CommandId::Codemode
        | CommandId::Doctor
        | CommandId::Limits
        | CommandId::Export
        | CommandId::Title
        | CommandId::Workflow
        | CommandId::Side
        | CommandId::CreateAgent => CommandRoute::Unavailable,
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

    pub(super) fn daemon_command_unavailable(&mut self, name: &str) {
        let notice = format!("/{name} is not available with RHO_TUI_DAEMON yet");
        self.insert_entry(&Entry::Notice(notice.clone()));
        self.set_status(notice);
    }

    /// Both ! and !! are unported: even excluded output currently persists
    /// through the local session path. Do not start a shell against it.
    pub(super) fn refuse_daemon_shell(&mut self) -> bool {
        if self.info.session.daemon.is_none() {
            return false;
        }
        self.daemon_command_unavailable("!shell");
        true
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
