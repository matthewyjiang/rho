use crate::tui::DefaultTerminal;

use super::{
    command_palette::slash_command_args,
    daemon_gate::{command_route, CommandRoute},
    App, ChatMedia, CommandId, CommandInvocation, Entry, PasteSegment, TurnPrompt,
};

/// Fully-owned composer state transferred to a slash command.
pub(super) struct CommandSubmission {
    invocation: CommandInvocation,
    turn: TurnPrompt,
    media: Vec<ChatMedia>,
    paste_segments: Vec<PasteSegment>,
}

impl CommandSubmission {
    pub(super) fn new(
        invocation: CommandInvocation,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
    ) -> Self {
        Self {
            invocation,
            turn,
            media,
            paste_segments,
        }
    }

    #[cfg(test)]
    pub(super) fn media_len(&self) -> usize {
        self.media.len()
    }

    #[cfg(test)]
    pub(super) fn model(&self) -> &str {
        &self.turn.model
    }

    #[cfg(test)]
    pub(super) fn display(&self) -> &str {
        &self.turn.display
    }
}

impl App {
    pub(super) fn execute_command<'a>(
        &'a mut self,
        submission: CommandSubmission,
        terminal: &'a mut DefaultTerminal,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + 'a {
        // Even /exit shares the largest command future. Construct it outside
        // the input poll frames so their debug move temporaries do not exhaust
        // the Windows main stack before command dispatch.
        Box::pin(self.execute_command_inner(submission, terminal))
    }

    async fn execute_command_inner(
        &mut self,
        submission: CommandSubmission,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let CommandSubmission {
            mut invocation,
            turn,
            media,
            paste_segments,
        } = submission;
        match command_route(invocation.id) {
            CommandRoute::Options => return self.execute_daemon_option_command(&invocation),
            CommandRoute::Prompt => {
                if !self.daemon_offers_built_in(invocation.id) {
                    self.daemon_command_unavailable(&invocation.name);
                    return Ok(());
                }
                // `/compact` is no send: it passes no confirmation.
                if invocation.id == CommandId::Compact {
                    return self
                        .send_daemon_command(turn, media, paste_segments, terminal)
                        .await;
                }
                return self
                    .submit_daemon_turn(turn, media, paste_segments, terminal)
                    .await;
            }
            CommandRoute::Ui => {}
        }
        match invocation.id {
            // These read what only an opened session has: its context,
            // hooks, and MCP servers. Asking opens a new session as its
            // first prompt would. It saves nothing until prompted, so the
            // daemon discards it once this TUI no longer follows it.
            CommandId::Context | CommandId::Hooks | CommandId::Mcp
                if !self.open_daemon_draft(terminal).await? =>
            {
                Ok(())
            }
            CommandId::AddDir => self.execute_add_dir_command(&invocation),
            CommandId::Exit => self.execute_exit_command(),
            CommandId::New => self.execute_new_command(terminal).await,
            CommandId::Login => self.execute_login_command(invocation, terminal).await,
            CommandId::Logout => self.execute_logout_command(invocation).await,
            CommandId::Resume => self.execute_resume_command(invocation, terminal).await,
            CommandId::Remember => {
                invocation.args = slash_command_args(&turn.model).trim().to_string();
                self.execute_remember_command(&invocation)
            }
            CommandId::Rewind => self.execute_rewind_command(invocation),
            CommandId::Sessions => {
                self.execute_sessions_command();
                Ok(())
            }
            CommandId::Tree => self.execute_tree_command(),
            CommandId::Config => {
                self.open_daemon_config();
                Ok(())
            }
            CommandId::Info => self.execute_info_command(),
            CommandId::Context => self.execute_context_command(),
            CommandId::Help => self.execute_help_command(),
            CommandId::Copy => self.execute_copy_command(),
            CommandId::Hooks => self.execute_hooks_command(),
            CommandId::Skills => self.execute_skills_command(),
            CommandId::Theme => self.open_theme_picker(),
            CommandId::Agents => self.execute_agents_command(),
            CommandId::Attach => self.execute_attach_command(),
            CommandId::Changelog => self.execute_changelog_command(&invocation, terminal),
            CommandId::Diff => self.execute_diff_command(),
            CommandId::Doctor => self.execute_doctor_command(terminal),
            CommandId::Jobs => self.execute_jobs_command(),
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::Mcp => self.execute_mcp_command(),
            CommandId::Title => self.execute_title_command(&invocation),
            CommandId::Limits => self.execute_limits_command(terminal),
            CommandId::Spend => self.execute_spend_command(),
            CommandId::Todo => self.execute_todo_command(terminal),
            CommandId::RefreshModels => self.execute_refresh_models_command(terminal).await,
            CommandId::Workflow => self.execute_workflow_command(terminal).await,
            CommandId::Side => self.execute_side_command(invocation).await,
            CommandId::Model
            | CommandId::Permissions
            | CommandId::Fast
            | CommandId::Advisor
            | CommandId::Codemode
            | CommandId::Init
            | CommandId::Compact
            | CommandId::CreateAgent => {
                unreachable!("command_route sends /{} to the daemon", invocation.name)
            }
        }
    }

    pub(super) fn report_unknown_command(&mut self, name: &str) {
        self.insert_entry(&Entry::Error(format!(
            "unknown command '/{name}'. Type / to choose one of: {}",
            super::commands::COMMANDS
                .iter()
                .map(|command| command.usage)
                .collect::<Vec<_>>()
                .join(", ")
        )));
        self.set_status("unknown command");
    }

    pub(super) fn execute_exit_command(&mut self) -> anyhow::Result<()> {
        self.should_quit = true;
        self.set_status("exiting rho");
        Ok(())
    }

    pub(super) async fn execute_new_command(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match self
            .switch_daemon_session(super::daemon_launch::DaemonTarget::New, terminal)
            .await
        {
            Ok(()) => self.set_status("new session"),
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not start new session: {error:#}"
            ))),
        }
        Ok(())
    }
}
