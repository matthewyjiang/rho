use super::CommandId;

/// Current session facts used to advertise built-in commands. These do not
/// authorize execution: a hidden command can still accept useful actions,
/// such as disabling a saved preference on an unsupported model.
pub(crate) struct CommandContext {
    pub fast_mode_supported: bool,
}

impl CommandContext {
    pub(crate) fn is_discoverable(&self, command: CommandId) -> bool {
        match command {
            CommandId::Fast => self.fast_mode_supported,
            CommandId::Advisor
            | CommandId::New
            | CommandId::Login
            | CommandId::Logout
            | CommandId::Model
            | CommandId::RefreshModels
            | CommandId::Resume
            | CommandId::Rewind
            | CommandId::Sessions
            | CommandId::Tree
            | CommandId::Config
            | CommandId::Permissions
            | CommandId::Info
            | CommandId::Help
            | CommandId::Compact
            | CommandId::Computer
            | CommandId::Copy
            | CommandId::Goal
            | CommandId::Skills
            | CommandId::Theme
            | CommandId::Hooks
            | CommandId::Agents
            | CommandId::CreateAgent
            | CommandId::Attach
            | CommandId::Changelog
            | CommandId::Diff
            | CommandId::Doctor
            | CommandId::Limits
            | CommandId::Export
            | CommandId::Mcp
            | CommandId::Title
            | CommandId::Workflow
            | CommandId::Side
            | CommandId::Exit => true,
        }
    }
}

#[cfg(test)]
#[path = "availability_tests.rs"]
mod tests;
