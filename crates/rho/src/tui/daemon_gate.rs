//! The single policy boundary for how each command reaches the daemon-owned
//! session. Keep this match exhaustive: every new builtin needs an
//! intentional classification.

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
        CommandId::Model
        | CommandId::Permissions
        | CommandId::Fast
        | CommandId::Advisor
        | CommandId::Codemode => CommandRoute::Options,
        // These commands are implemented by the daemon's prompt pipeline.
        CommandId::Init | CommandId::Compact | CommandId::CreateAgent => CommandRoute::Prompt,
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
        // Agent definitions are files; the daemon is told when they change.
        | CommandId::Agents
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
    /// Says that the daemon's session does not offer the prompt command
    /// `/name`.
    pub(super) fn daemon_command_unavailable(&mut self, name: &str) {
        let notice = format!("/{name} is not available in a daemon session yet");
        self.insert_entry(&Entry::Notice(notice.clone()));
        self.set_status(notice);
    }

    /// Whether the daemon's session offers the built-in prompt command `id`
    /// under any of its names: `/agents create` runs as the `/create-agent`
    /// it lists.
    pub(super) fn daemon_offers_built_in(&self, id: CommandId) -> bool {
        crate::commands::COMMANDS
            .iter()
            .filter(|spec| spec.id == id)
            .any(|spec| self.daemon_prompt_command_available(spec.name))
    }

    /// Whether the daemon's session lists the skill `name`, or lists no
    /// skill at all: a session with the skill tool lists the built-in ones,
    /// so that one has no skill tool, and the daemon says so.
    pub(super) fn daemon_may_load_skill(&self, name: &str) -> bool {
        let mut skills = self
            .daemon_options
            .iter()
            .flat_map(|options| &options.commands)
            .filter(|command| command.kind == rho_protocol::CommandKind::Skill)
            .peekable();
        skills.peek().is_none() || skills.any(|skill| skill.name == name)
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
