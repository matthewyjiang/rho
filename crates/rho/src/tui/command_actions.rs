use crate::tui::DefaultTerminal;

use super::{
    command_palette::slash_command_args, App, ChatMedia, CommandId, CommandInvocation, Entry,
    InteractiveRuntime, PasteSegment, TurnPrompt,
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
        agent: &'a mut InteractiveRuntime,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + 'a {
        // Even /exit shares the largest command future. Construct it outside
        // the input poll frames so their debug move temporaries do not exhaust
        // the Windows main stack before command dispatch.
        Box::pin(self.execute_command_inner(submission, terminal, agent))
    }

    async fn execute_command_inner(
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
        if self.info.session.daemon.is_some() {
            match super::daemon_gate::command_route(invocation.id) {
                super::daemon_gate::CommandRoute::Options => {
                    return self.execute_daemon_option_command(&invocation)
                }
                super::daemon_gate::CommandRoute::Prompt => {
                    if !self.daemon_prompt_command_available(&invocation.name) {
                        self.daemon_command_unavailable(&invocation.name);
                        return Ok(());
                    }
                    return self
                        .submit_daemon_turn(turn, media, paste_segments, terminal)
                        .await;
                }
                super::daemon_gate::CommandRoute::Ui => {}
            }
        }
        match invocation.id {
            CommandId::AddDir => self.execute_add_dir_command(&invocation, agent).await,
            CommandId::Advisor => {
                self.needs_daemon("advisor");
                Ok(())
            }
            CommandId::Codemode => self.execute_codemode_command(invocation, agent),
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
            CommandId::Remember => {
                invocation.args = slash_command_args(&turn.model).trim().to_string();
                self.execute_remember_command(&invocation, agent)
            }
            CommandId::Rewind => self.execute_rewind_command(invocation, agent),
            CommandId::Sessions => {
                self.execute_sessions_command();
                Ok(())
            }
            CommandId::Tree => self.execute_tree_command(agent),
            CommandId::Config => {
                self.execute_config_command();
                Ok(())
            }
            CommandId::Permissions => self.execute_permissions_command(invocation, agent).await,
            CommandId::Info => self.execute_info_command(),
            CommandId::Context => self.execute_context_command(agent),
            CommandId::Init => {
                self.execute_init_command(turn, media, paste_segments, terminal, agent)
                    .await
            }
            CommandId::Help => self.execute_help_command(),
            CommandId::Compact => {
                // Expanded model text, so pasted guidance arrives in full.
                let instructions = slash_command_args(&turn.model).trim();
                self.start_compact(
                    agent,
                    super::compact_work::CompactFollowUp::None,
                    (!instructions.is_empty()).then(|| instructions.to_owned()),
                )
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
            CommandId::Doctor => self.execute_doctor_command(terminal),
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::Mcp => self.execute_mcp_command(),
            CommandId::Title => self.execute_title_command(&invocation),
            CommandId::Limits => self.execute_limits_command(terminal),
            CommandId::Spend => self.execute_spend_command(),
            CommandId::Todo => {
                if self.info.session.daemon.is_none() {
                    self.refresh_todo_list(agent);
                }
                self.execute_todo_command(terminal)
            }
            CommandId::Fast => self.execute_fast_command(invocation, agent),
            CommandId::RefreshModels => self.execute_refresh_models_command(terminal, agent).await,
            CommandId::Workflow => self.execute_workflow_command(terminal).await,
            CommandId::Side => self.execute_side_command(invocation).await,
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
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.info.session.daemon.is_some() {
            match self
                .switch_daemon_session(super::daemon_session::DaemonTarget::New, terminal)
                .await
            {
                Ok(()) => self.set_status("new session"),
                Err(error) => self.insert_entry(&Entry::Error(format!(
                    "could not start new session: {error:#}"
                ))),
            }
            return Ok(());
        }
        // Reset refuses to run under compaction, so stop it first.
        self.abort_compact(agent).await;
        if let Err(error) = agent.reset() {
            // The old session is still live: report the failure and keep its
            // pending work instead of clearing the UI as though one started.
            self.insert_entry(&Entry::Error(format!(
                "could not start new session: {error}"
            )));
            return Ok(());
        }
        self.info.runtime.added_dirs = agent.added_dirs().clone();
        self.held_turns.clear();
        self.clear_tool_sources_loading_activity();
        self.info.session.session_id = None;
        self.reset_session_view(terminal)?;
        self.set_status("new session");
        Ok(())
    }
}
