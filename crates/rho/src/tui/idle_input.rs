use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use super::{
    command_palette::slash_command_args,
    commands, goal_command,
    paste_burst::{next_word_boundary, previous_word_boundary},
    skill_actions, App, CommandId, ComposerMode, GoalState, HistoryDirection, InputSubmissionMode,
    InteractiveRuntime, TurnOutcome, TurnPrompt,
};

impl App {
    /// Route keys owned by modal/overlay composers. Returns true when handled.
    async fn handle_composer_mode_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        match &self.input_ui.composer {
            ComposerMode::Input => Ok(false),
            ComposerMode::OAuthPending(_) => self.handle_oauth_pending_key(key),
            ComposerMode::InlineChoice(_) => {
                self.handle_inline_choice_key(key, terminal, agent).await
            }
            ComposerMode::Questionnaire(_) => self.handle_questionnaire_key(key),
            ComposerMode::SecretInput(_) => self.handle_secret_key(key, terminal, agent).await,
            ComposerMode::ConfigNumberInput(_) => self.handle_config_number_key(key, terminal),
            ComposerMode::ConfigTextInput(_) => self.handle_config_text_key(key),
            ComposerMode::Picker(_) => self.handle_picker_key(key, terminal, agent).await,
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

        if self.handle_reasoning_cycle_key(key, agent)? {
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

        if self.handle_configurable_composer_key(key, terminal, agent)? {
            return Ok(());
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    self.clear_submitted_input();
                    self.input_ui.submission_mode = InputSubmissionMode::ParseCommands;
                    self.input_ui.pending_images.clear();
                    self.notify_status("input cleared; press ctrl-c again to quit");
                    self.ctrl_c_streak = 1;
                } else {
                    self.should_quit = true;
                }
            }
            (_, KeyCode::Esc) => {
                if !self.cancel_inline_shells() {
                    let _ = self.exit_shell_mode();
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
                self.input_ui.cursor =
                    previous_word_boundary(&self.input_ui.text, self.input_ui.cursor);
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Right) => {
                self.input_ui.cursor =
                    next_word_boundary(&self.input_ui.text, self.input_ui.cursor);
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
                self.input_ui.cursor = 0;
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::End) => {
                self.reset_input_history_navigation();
                self.input_ui.cursor = self.input_char_len();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Enter) => {
                self.insert_input_char('\n');
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_input_char('\n');
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Enter) => {
                self.submit(terminal, agent).await?;
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_input_char(ch);
                self.ctrl_c_streak = 0;
            }
            _ => {
                self.input_ui.paste_burst.clear();
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
                    self.input_ui.command_selection = if self.input_ui.command_selection == 0 {
                        matches.len() - 1
                    } else {
                        self.input_ui.command_selection - 1
                    };
                }
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.input_ui.command_selection =
                        (self.input_ui.command_selection + 1) % matches.len();
                }
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.input_ui.command_palette_dismissed = false;
                    self.clamp_command_selection();
                }
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.clamp_command_selection();
                }
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit(terminal, agent).await?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.input_ui.command_palette_dismissed = true;
                self.input_ui.command_selection = 0;
                self.input_ui.paste_burst.clear();
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
        let mut turn = TurnPrompt::standard(
            self.expanded_input().trim().to_string(),
            self.input_ui.text.trim().to_string(),
        );
        if turn.model.is_empty()
            && self.input_ui.pending_images.is_empty()
            && self.input_ui.shell_mode.is_none()
        {
            self.clear_submitted_input();
            return Ok(());
        }
        if let Some((mode, command)) = self.shell_submission() {
            if !self.input_ui.paste_segments.is_empty() {
                return self.block_pasted_inline_shell();
            }
            self.clear_submitted_input();
            self.ensure_session(agent)?;
            self.start_inline_shell(mode, command)?;
            return Ok(());
        }

        match self.parse_input_command() {
            Ok(Some(mut invocation)) => {
                if invocation.id == CommandId::Goal {
                    invocation.raw_args = slash_command_args(&turn.model).to_string();
                    invocation.args = invocation.raw_args.trim().to_string();
                }
                self.clear_submitted_input();
                self.execute_command(invocation, terminal, agent).await?;
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
                } else {
                    match self.skill_command_action(
                        &name,
                        turn.model,
                        turn.display,
                        agent.has_tool("skill"),
                    )? {
                        skill_actions::SkillCommandAction::Prompt(prompt) => turn = prompt,
                        skill_actions::SkillCommandAction::Rejected => return Ok(()),
                        skill_actions::SkillCommandAction::NotSkill => {
                            self.report_unknown_command(&name);
                            return Ok(());
                        }
                    }
                }
            }
        }

        let images = std::mem::take(&mut self.input_ui.pending_images);
        self.clear_submitted_input();
        let turn = self.prepare_goal_resumption_turn(turn);
        let mut outcome = self.run_prompt_turn(turn, images, terminal, agent).await?;
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
                    pending_goal_retries.push_back(failed_turn);
                }
            }

            let should_drain_queue =
                goal_command::should_drain_queued_prompts(outcome_kind, resume_goal);
            if self.should_quit
                || !should_drain_queue
                || self.input_ui.composer.blocks_auto_continue()
            {
                break outcome_kind;
            }
            let Some(prompt) = self.pending.queued_prompts.pop_front() else {
                break outcome_kind;
            };
            self.pending_input_changed();
            self.select_pending_recall_target();
            outcome = self
                .run_prompt_turn(
                    TurnPrompt::standard(prompt.prompt, prompt.display_prompt),
                    Vec::new(),
                    terminal,
                    agent,
                )
                .await?;
        };
        if !self.input_ui.composer.blocks_auto_continue()
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
