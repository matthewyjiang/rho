use std::sync::atomic::AtomicBool;

use ratatui::DefaultTerminal;

use super::{
    ActivityPhase, App, CommandId, CommandInvocation, ComposerMode, Entry, InteractiveRuntime,
    LoadingSpinner, RunningInputMode, StreamControl, ToolEntry, ToolEntryState, ViewModelEvent,
};

impl App {
    pub(super) async fn execute_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Exit => self.execute_exit_command(),
            CommandId::New => self.execute_new_command(terminal, agent),
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
            CommandId::Tree => self.execute_tree_command(agent),
            CommandId::Config => self.execute_config_command(terminal),
            CommandId::Info => self.execute_info_command().await,
            CommandId::Help => self.execute_help_command(),
            CommandId::Compact => self
                .execute_compact_command(terminal, agent)
                .await
                .map(|_| ()),
            CommandId::Goal => self.execute_goal_command(invocation, terminal, agent).await,
            CommandId::Skills => self.execute_skills_command(),
            CommandId::Agents => self.execute_agents_command(),
            CommandId::Diff => self.execute_diff_command(),
            CommandId::Doctor => self.execute_doctor_command_with_probes(terminal).await,
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::Limits => self.execute_limits_command(terminal),
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
        self.status = "unknown command".into();
    }

    pub(super) async fn execute_compact_command(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        use super::compaction_display::{
            compaction_call_id, running_card, CompactionDisplayFacts, CompactionUiOutcome,
        };

        self.pending.steering_prompts_mut().clear();
        self.pending_input_changed();
        self.status = "compacting context".into();
        self.begin_compact_ui();
        self.turn.set_activity_phase(ActivityPhase::Compacting);
        self.turn.start_loading();
        self.turn.tool_started(compaction_call_id(), running_card());
        terminal.draw(|frame| self.draw(frame))?;

        let interrupt_requested = AtomicBool::new(false);
        let tool_call_active = AtomicBool::new(false);
        let mut compact_future = Box::pin(agent.compact());
        let compacted = loop {
            tokio::select! {
                result = &mut compact_future => break result,
                terminal_event = self.terminal_session.as_mut().expect("terminal session initialized").next_event() => {
                    match self.handle_running_terminal_events(
                        terminal_event?,
                        terminal,
                        &interrupt_requested,
                        &tool_call_active,
                        RunningInputMode::Compacting,
                    )
                    .await
                    .map_err(super::during_turn::RunningTerminalError::into_anyhow)?
                    {
                        StreamControl::Interrupt => {
                            break Err(anyhow::anyhow!("compaction interrupted"));
                        }
                        StreamControl::Continue
                        | StreamControl::Resize
                        | StreamControl::ApprovalResolved => {}
                    }
                    self.clamp_history_scroll_for_terminal(terminal)?;
                    terminal.draw(|frame| self.draw(frame))?;
                }
                _ = tokio::time::sleep(LoadingSpinner::FRAME_INTERVAL) => {
                    terminal.draw(|frame| self.draw(frame))?;
                }
            }
        };
        drop(compact_future);
        if let Some(context) = agent.take_context_usage() {
            self.record_agent_event(ViewModelEvent::ContextUsage(context));
        }
        self.end_busy_ui();
        self.turn.stop_loading();
        let expanded = self.turn.tool_finished(&compaction_call_id());

        let (outcome, status, succeeded) = match compacted {
            Ok(Some(outcome)) => (
                CompactionUiOutcome::Completed(CompactionDisplayFacts::from_outcome(&outcome)),
                "context compacted",
                true,
            ),
            Ok(None) => (
                CompactionUiOutcome::Unchanged {
                    detail: "not enough conversation history to compact, or the model context window is unknown".into(),
                },
                "context not compacted",
                false,
            ),
            Err(err) if err.to_string().contains("interrupted") => (
                CompactionUiOutcome::Cancelled,
                "context compaction cancelled",
                false,
            ),
            Err(err) => (
                CompactionUiOutcome::Failed {
                    detail: err.to_string(),
                },
                "context compaction failed",
                false,
            ),
        };
        self.insert_entry(&Entry::Tool(ToolEntry {
            state: ToolEntryState::Finished { ok: outcome.ok() },
            card: outcome.card(),
            expanded,
            image: None,
        }));
        self.status = status.into();
        Ok(succeeded)
    }

    pub(super) fn execute_exit_command(&mut self) -> anyhow::Result<()> {
        self.insert_entry(&Entry::Notice("exiting rho".into()));
        self.should_quit = true;
        self.status = "exiting".into();
        Ok(())
    }

    fn execute_new_command(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        agent.reset()?;
        self.info.session.session_id = None;
        self.input_ui.set_composer(ComposerMode::Input);
        self.input_ui.clear_text();
        self.input_ui.clear_paste_segments();
        self.input_ui.set_shell_mode(None);
        self.input_ui.set_cursor(0);
        self.input_ui.clear_pending_images();
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
        self.turn.set_current_turn_start(None);
        self.history.clear_entries();
        self.history.images_mut().clear();
        self.history.set_images_dirty_from(None);
        self.history.lines_mut().invalidate_from(0);
        self.scroll_history_to_bottom();
        self.clamp_history_scroll_for_terminal(terminal)?;
        self.status = "new session".into();
        Ok(())
    }
}
