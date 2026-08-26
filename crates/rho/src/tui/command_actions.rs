use ratatui::DefaultTerminal;

use super::{
    command_palette::slash_command_args, App, ChatMedia, CommandId, CommandInvocation,
    ComposerMode, Entry, InteractiveRuntime, PasteSegment, TurnPrompt,
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
    pub(super) async fn execute_command(
        &mut self,
        submission: CommandSubmission,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let CommandSubmission {
            mut invocation,
            turn,
            media,
            paste_segments,
        } = submission;
        match invocation.id {
            CommandId::Advisor => self.execute_advisor_command(invocation, agent).await,
            CommandId::Exit => self.execute_exit_command(),
            CommandId::New => self.execute_new_command(terminal, agent).await,
            CommandId::Model => {
                self.execute_model_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Login => {
                self.execute_login_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Logout => self.execute_logout_command(invocation, agent).await,
            CommandId::Resume => {
                self.execute_resume_command(invocation, terminal, agent)
                    .await
            }
            CommandId::Rewind => self.execute_rewind_command(invocation, agent),
            CommandId::Sessions => self.execute_sessions_command(terminal),
            CommandId::Tree => self.execute_tree_command(agent),
            CommandId::Config => self.execute_config_command(terminal),
            CommandId::Info => self.execute_info_command().await,
            CommandId::Help => self.execute_help_command(),
            CommandId::Compact => {
                self.start_compact(agent, super::compact_work::CompactFollowUp::None)
            }
            CommandId::Copy => self.execute_copy_command(),
            CommandId::Goal => {
                invocation.raw_args = slash_command_args(&turn.model).to_string();
                invocation.args = invocation.raw_args.trim().to_string();
                self.execute_goal_command(invocation, media, terminal, agent)
                    .await
            }
            CommandId::Hooks => self.execute_hooks_command(agent),
            CommandId::Skills => self.execute_skills_command(),
            CommandId::Theme => self.open_theme_picker(),
            CommandId::Agents => self.execute_agents_command(),
            CommandId::CreateAgent => {
                self.execute_create_agent_command(
                    &invocation,
                    turn,
                    media,
                    paste_segments,
                    terminal,
                    agent,
                )
                .await
            }
            CommandId::Attach => self.execute_attach_command(),
            CommandId::Changelog => self.execute_changelog_command(&invocation, terminal),
            CommandId::Diff => self.execute_diff_command(),
            CommandId::Doctor => self.execute_doctor_command_with_probes(terminal).await,
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::Mcp => self.execute_mcp_command(),
            CommandId::Title => self.execute_title_command(&invocation),
            CommandId::Limits => self.execute_limits_command(terminal),
            CommandId::Fast => self.execute_fast_command(invocation, agent),
            CommandId::Workflow => self.execute_workflow_command(terminal).await,
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

    async fn execute_new_command(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.abort_compact(agent).await;
        self.held_turns.clear();
        self.clear_mcp_connecting_activity();
        self.start_follow_ups = None;
        agent.reset().await?;
        self.info.session.session_id = None;
        self.input_ui.set_composer(ComposerMode::Input);
        self.input_ui.clear_text();
        self.input_ui.clear_paste_segments();
        self.input_ui.set_shell_mode(None);
        self.input_ui.set_cursor(0);
        self.cancel_all_pending_attachments();
        self.input_ui.clear_attachments();
        self.input_ui.set_command_palette_dismissed(false);
        self.clamp_command_selection();
        self.pending.clear_follow_ups();
        self.goal = None;
        self.pending.clear_steering();
        self.pending.clear_input_action();
        self.pending_input_changed();
        self.reset_streams();
        self.end_busy_ui();
        self.turn.clear_tool_calls();
        self.reset_usage();
        self.usage.current_context = None;
        self.pending_session_title = None;
        self.session_title_locked = false;
        self.turn.set_current_turn_start(None);
        self.history.clear_entries();
        self.history.images_mut().clear();
        self.history.set_images_dirty_from(None);
        self.history.lines_mut().invalidate_from(0);
        self.scroll_history_to_bottom();
        self.clamp_history_scroll_for_terminal(terminal)?;
        self.set_status("new session");
        Ok(())
    }
}
