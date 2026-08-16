use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use super::{
    command_actions::CommandSubmission, command_palette::slash_command_args, commands,
    goal_command, skill_actions, App, ChatMedia, ComposerMode, GoalState, HistoryDirection,
    InputSubmissionMode, InteractiveRuntime, PasteSegment, QueuedPrompt, TurnOutcome, TurnPrompt,
};

/// A turn held until MCP connect settles.
pub(super) struct HeldTurn {
    pub(super) turn: TurnPrompt,
    pub(super) media: Vec<ChatMedia>,
    /// Kept so `esc` can hand the prompt back exactly as it was typed.
    pub(super) paste_segments: Vec<PasteSegment>,
}

impl App {
    fn take_command_submission(
        &mut self,
        invocation: super::CommandInvocation,
        expanded_input: String,
    ) -> CommandSubmission {
        let media = self
            .input_ui
            .take_ready_media()
            .expect("pending attachments block submission");
        let submission = CommandSubmission::new(invocation, expanded_input, media);
        self.clear_submitted_input();
        submission
    }

    /// Route keys owned by modal/overlay composers. Returns true when handled.
    async fn handle_composer_mode_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        match self.input_ui.composer() {
            ComposerMode::Input => Ok(false),
            ComposerMode::InteractivePending(_) => self.handle_interactive_pending_key(key),
            ComposerMode::InlineChoice(_) => {
                self.handle_inline_choice_key(key, terminal, agent).await
            }
            ComposerMode::Questionnaire(_) => self.handle_questionnaire_key(key),
            ComposerMode::SecretInput(_) => self.handle_secret_key(key, terminal, agent).await,
            ComposerMode::ConfigNumberInput(_) => self.handle_config_number_key(key, terminal),
            ComposerMode::TextInput(_) => self.handle_text_input_key(key),
            ComposerMode::Picker(_) => self.handle_picker_key(key, terminal, agent).await,
            ComposerMode::Limits(_) => Ok(self.handle_limits_overlay_key(key, terminal)),
            // Approvals are handled on the during-turn path, not idle input.
            ComposerMode::Approval(_) => Ok(false),
        }
    }

    pub(super) async fn handle_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.handle_paste_burst_key(key) {
            return Ok(());
        }

        if self.handle_pending_input_key(key) {
            return Ok(());
        }

        if self.external_editor_shortcut_matches(key) {
            self.open_composer_in_editor(terminal).await?;
            return Ok(());
        }

        if self.handle_history_key(key, terminal)? {
            return Ok(());
        }

        // Overlay / modal composers own keys first. Dispatch by mode so the
        // shared free-text path below only runs for ComposerMode::Input.
        if self.handle_composer_mode_key(key, terminal, agent).await? {
            return Ok(());
        }

        if self.handle_reasoning_cycle_key(key, agent).await? {
            return Ok(());
        }

        if self
            .handle_command_palette_key(key, terminal, agent)
            .await?
        {
            return Ok(());
        }

        if self.handle_file_palette_key(key)? {
            return Ok(());
        }

        if self
            .handle_configurable_composer_key(key, terminal, agent)
            .await?
        {
            return Ok(());
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    self.clear_submitted_input();
                    self.input_ui
                        .set_submission_mode(InputSubmissionMode::ParseCommands);
                    self.notify_status("input cleared; press ctrl-c again to quit");
                    self.ctrl_c_streak = 1;
                } else {
                    self.should_quit = true;
                }
            }
            (_, KeyCode::Esc) => {
                let cancelled_compact = self.cancel_compact(terminal, agent).await?;
                if cancelled_compact || (!self.cancel_inline_shells() && !self.exit_shell_mode()) {
                    self.take_back_held_turn();
                }
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Backspace) => {
                self.delete_word_before_cursor();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Backspace) => {
                self.backspace_input();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Delete) => {
                self.delete_input();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Left) => {
                self.move_input_cursor_to_previous_word();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Right) => {
                self.move_input_cursor_to_next_word();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Left) => {
                self.move_input_cursor_left();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Right) => {
                self.move_input_cursor_right();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Up) => {
                let width = terminal.size()?.width as usize;
                self.recall_input_history_or_move_cursor(HistoryDirection::Previous, width);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Down) => {
                let width = terminal.size()?.width as usize;
                self.recall_input_history_or_move_cursor(HistoryDirection::Next, width);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Home) => {
                self.reset_input_history_navigation();
                self.input_ui.clear_selection();
                self.input_ui.set_cursor(0);
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::End) => {
                self.reset_input_history_navigation();
                self.input_ui.clear_selection();
                self.input_ui.set_cursor(self.input_char_len());
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Enter) => {
                if agent.is_compacting() {
                    self.queue_prompt_after_turn()?;
                } else {
                    self.insert_input_char('\n');
                }
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_input_char('\n');
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Enter) => {
                self.submit_from_composer(terminal, agent).await?;
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_input_char(ch);
                self.ctrl_c_streak = 0;
            }
            _ => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
            }
        }
        self.clamp_command_selection();
        self.clamp_file_selection();
        Ok(())
    }

    pub(super) async fn handle_command_palette_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !self.command_palette_visible() {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.input_ui.set_command_selection(
                        if self.input_ui.command_selection() == 0 {
                            matches.len() - 1
                        } else {
                            self.input_ui.command_selection() - 1
                        },
                    );
                }
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.input_ui.set_command_selection(
                        (self.input_ui.command_selection() + 1) % matches.len(),
                    );
                }
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.input_ui.set_command_palette_dismissed(false);
                    self.clamp_command_selection();
                }
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.clamp_command_selection();
                }
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                self.submit_from_composer(terminal, agent).await?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.input_ui.set_command_palette_dismissed(true);
                self.input_ui.set_command_selection(0);
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) async fn submit(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.input_ui.has_pending_attachments() {
            self.notify_status("wait for document extraction to finish before submitting");
            return Ok(());
        }
        let mut turn = TurnPrompt::standard(
            self.expanded_input().trim().to_string(),
            self.input_ui.text().trim().to_string(),
        );
        if turn.model.is_empty()
            && self.input_ui.attachments().is_empty()
            && self.input_ui.shell_mode().is_none()
        {
            self.clear_submitted_input();
            return Ok(());
        }
        if let Some((mode, command)) = self.shell_submission() {
            if !self.input_ui.paste_segments().is_empty() {
                return self.block_pasted_inline_shell();
            }
            self.clear_submitted_input();
            self.ensure_session(agent)?;
            self.start_inline_shell(mode, command)?;
            return Ok(());
        }

        match self.parse_input_command() {
            Ok(Some(invocation)) => {
                let submission = self.take_command_submission(invocation, turn.model);
                self.execute_command(submission, terminal, agent).await?;
                return Ok(());
            }
            Ok(None) => {}
            Err(commands::CommandParseError::Unknown(name)) => {
                let trailing_prompt = slash_command_args(&turn.model).trim().to_string();
                self.clear_submitted_input();
                let template = name
                    .get(.."prompt:".len())
                    .filter(|prefix| prefix.eq_ignore_ascii_case("prompt:"))
                    .and_then(|_| name.get("prompt:".len()..))
                    .and_then(|template_name| {
                        crate::prompt_templates::find(
                            &self.info.runtime.prompt_templates,
                            template_name,
                        )
                    });
                if let Some(template) = template {
                    let prompt = crate::prompt_templates::expand(template, &trailing_prompt);
                    turn = TurnPrompt::standard(prompt.clone(), prompt);
                } else if let Some(expanded) = self
                    .expand_mcp_prompt(&name, &trailing_prompt, &turn.display, agent)
                    .await?
                {
                    turn = expanded;
                } else {
                    match self.skill_command_action(
                        &name,
                        turn.model,
                        turn.display,
                        agent.has_tool("skill"),
                    )? {
                        skill_actions::SkillCommandAction::Prompt(prompt) => turn = *prompt,
                        skill_actions::SkillCommandAction::Rejected => return Ok(()),
                        skill_actions::SkillCommandAction::NotSkill => {
                            self.report_unknown_command(&name);
                            return Ok(());
                        }
                    }
                }
            }
        }

        if !self.setup_state().signed_in {
            return self.offer_login_instead_of_turn(turn);
        }

        let media = self
            .input_ui
            .take_ready_media()
            .expect("pending attachments block submission");
        let paste_segments = self.input_ui.paste_segments().to_vec();
        self.clear_submitted_input();

        // MCP connect now runs after the first frame, so a prompt can land
        // before the servers report. Hold the turn and start it once connect
        // settles; the alternative is a person watching a status line and
        // pressing enter again for up to the full connect budget.
        if agent.mcp_connect_pending() {
            // Queue rather than replace: someone who submits twice while the
            // servers are still connecting must not lose the first prompt.
            self.hold_turn(turn, media, paste_segments);
            self.set_mcp_connecting_status();
            return Ok(());
        }

        self.run_turn_sequence_held(turn, media, paste_segments, terminal, agent)
            .await
    }

    async fn submit_from_composer(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if agent.is_compacting() {
            self.submit_during_turn(terminal).await
        } else {
            self.submit(terminal, agent).await
        }
    }

    fn hold_turn(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
    ) {
        self.held_turns.push_back(HeldTurn {
            turn,
            media,
            paste_segments,
        });
    }

    /// Hand the most recently held turn back to the composer, so `esc` unwinds
    /// prompts newest first instead of leaving them to run. Does nothing unless
    /// the composer is empty, so it cannot overwrite something the person has
    /// started typing since.
    fn take_back_held_turn(&mut self) {
        if !self.input_ui.text().is_empty() || !self.input_ui.attachments().is_empty() {
            return;
        }
        let Some(HeldTurn {
            turn,
            media,
            paste_segments,
            ..
        }) = self.held_turns.pop_back()
        else {
            return;
        };
        self.restore_pending_prompt(QueuedPrompt {
            prompt: turn.model,
            display_prompt: turn.display,
            paste_segments,
            media: Vec::new(),
        });
        if !self.held_turns.is_empty() {
            // Older holds are still waiting. Leave their status alone: writing
            // any status here would hand `status_source` back to `Other`, and
            // `poll_startup_hydrates` would never retire the indicator.
            return;
        }
        // Attachments cannot go back into the composer, so say so rather than
        // let them disappear with the hold.
        if media.is_empty() {
            self.set_status_quiet("");
        } else {
            self.notify_status("prompt returned to the composer; attach the files again");
        }
    }

    fn first_held_turn_is_releasable(&self, mcp_pending: bool, compacting: bool) -> bool {
        !self.held_turns.is_empty() && !mcp_pending && !compacting
    }

    /// Start the next held turn whose wait is over. One per call, so several
    /// held turns run in submission order. Reports whether anything changed so
    /// the caller can redraw.
    pub(super) async fn release_pending_held_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if self.input_ui.composer().blocks_held_turn_start() {
            return Ok(false);
        }
        if !self.first_held_turn_is_releasable(agent.mcp_connect_pending(), agent.is_compacting()) {
            return Ok(false);
        }
        let Some(HeldTurn {
            turn,
            media,
            paste_segments,
        }) = self.held_turns.pop_front()
        else {
            return Ok(false);
        };
        self.set_status_quiet("");
        self.run_turn_sequence_held(turn, media, paste_segments, terminal, agent)
            .await?;
        Ok(true)
    }

    /// Start the next queued follow-up once armed and the composer is free.
    /// Compact arms this with auto-compact off; model-switch handoff arms it on.
    pub(super) async fn start_next_follow_up(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(allow_auto_compact) = self.start_follow_ups else {
            return Ok(false);
        };
        if agent.is_compacting() || agent.mcp_connect_pending() {
            return Ok(false);
        }
        if self.input_ui.composer().blocks_held_turn_start() {
            return Ok(false);
        }
        if self.is_ui_busy() {
            self.start_follow_ups = None;
            return Ok(false);
        }
        let Some(prompt) = self.pending.pop_follow_up() else {
            self.start_follow_ups = None;
            return Ok(false);
        };
        self.start_follow_ups = None;
        self.pending_input_changed();
        self.select_pending_recall_target();
        let turn = TurnPrompt::standard(prompt.prompt, prompt.display_prompt);
        if allow_auto_compact {
            self.run_turn_sequence(turn, prompt.media, terminal, agent)
                .await?;
        } else {
            self.run_turn_sequence_without_auto_compact(turn, prompt.media, terminal, agent)
                .await?;
        }
        Ok(true)
    }

    /// Run a submitted turn plus any goal resumption or queued follow-ups it
    /// triggers. Entered directly on submit, or later when a held turn is
    /// released.
    pub(super) async fn run_turn_sequence(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.run_turn_sequence_held(turn, media, Vec::new(), terminal, agent)
            .await
    }

    async fn run_turn_sequence_held(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if agent.is_compacting() {
            return self.queue_prompt(turn.model, turn.display, paste_segments, media);
        }
        if agent.should_auto_compact() {
            self.start_compact(agent, super::compact_work::CompactFollowUp::None)?;
            return self.queue_prompt(turn.model, turn.display, paste_segments, media);
        }
        self.run_turn_sequence_without_auto_compact(turn, media, terminal, agent)
            .await
    }

    async fn run_turn_sequence_without_auto_compact(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let turn = self.prepare_goal_resumption_turn(turn);
        let mut outcome = self.run_prompt_turn(turn, media, terminal, agent).await?;
        self.finish_goal_resumption_turn(outcome.kind());
        let mut pending_goal_retries = VecDeque::new();
        let final_outcome = loop {
            let outcome_kind = outcome.kind();
            let resume_goal = goal_command::should_resume_goal_after_turn(
                outcome_kind,
                self.goal.as_ref().map(GoalState::loop_state),
                self.should_quit,
            );
            if let TurnOutcome::Failed(failed_turn) = outcome {
                if resume_goal {
                    pending_goal_retries.push_back(*failed_turn);
                }
            }

            let should_drain_queue =
                goal_command::should_drain_queued_prompts(outcome_kind, resume_goal);
            if self.should_quit
                || !should_drain_queue
                || self.input_ui.composer().blocks_auto_continue()
            {
                break outcome_kind;
            }
            let Some(prompt) = self.pending.pop_follow_up() else {
                break outcome_kind;
            };
            self.pending_input_changed();
            self.select_pending_recall_target();
            outcome = self
                .run_prompt_turn(
                    TurnPrompt::standard(prompt.prompt, prompt.display_prompt),
                    prompt.media,
                    terminal,
                    agent,
                )
                .await?;
        };
        if !self.input_ui.composer().blocks_auto_continue()
            && goal_command::should_resume_goal_after_turn(
                final_outcome,
                self.goal.as_ref().map(GoalState::loop_state),
                self.should_quit,
            )
        {
            self.continue_goal(terminal, agent, pending_goal_retries)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "idle_input_tests.rs"]
mod tests;
