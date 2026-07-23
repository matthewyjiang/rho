//! Input and command handling while a model turn is running.
//!
//! Owns key routing, steering/follow-up queues, during-turn slash commands,
//! running picker/config overlays, and terminal event draining for the live
//! turn loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use super::{
    activity::LoadingSpinner,
    commands::{self, CommandId, CommandInvocation},
    config_editor::{ConfigNumberInput, ConfigNumberKey, ConfigTextKey},
    config_picker, model_picker, mouse_capture, next_word_boundary, normalize_paste,
    previous_word_boundary, App, ApprovalKeyOutcome, ComposerMode, Entry, HistoryDirection,
    InputSubmissionMode, InteractiveModelSelection, InteractiveRuntime, PasteSegment, PickerAction,
    QueuedPrompt, RunningInputMode, StreamControl, MAX_TERMINAL_EVENTS_PER_TICK,
};

impl App {
    pub(super) fn handle_key_during_turn(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if self.handle_paste_burst_key(key) {
            return Ok(false);
        }

        if self.handle_pending_input_key(key) {
            return Ok(false);
        }

        match self.handle_approval_key(key, terminal.size()?.width as usize)? {
            ApprovalKeyOutcome::Ignored => {}
            ApprovalKeyOutcome::Handled => return Ok(false),
            ApprovalKeyOutcome::Resolved => return Ok(true),
        }

        if self.handle_history_key(key, terminal)? {
            return Ok(false);
        }

        if self.handle_questionnaire_key(key)? {
            return Ok(false);
        }
        if self.handle_running_config_number_key(key, terminal)? {
            return Ok(false);
        }
        if self.handle_running_config_text_key(key)? {
            return Ok(false);
        }
        if self.handle_running_picker_key(key, terminal)? {
            return Ok(false);
        }
        if self.handle_running_command_palette_key(key, terminal)? {
            return Ok(false);
        }
        if self.handle_running_file_palette_key(key)? {
            return Ok(false);
        }
        if self.handle_configurable_running_key(key, terminal)? {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.ctrl_c_streak == 0 {
                    self.clear_submitted_input();
                    self.input_submission_mode = InputSubmissionMode::ParseCommands;
                    self.pending_images.clear();
                    self.notify_status("input cleared; press esc to interrupt model");
                    self.ctrl_c_streak = 1;
                } else {
                    self.should_quit = true;
                }
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
                self.input_cursor = previous_word_boundary(&self.input, self.input_cursor);
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Right) => {
                self.input_cursor = next_word_boundary(&self.input, self.input_cursor);
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
                self.input_cursor = 0;
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::End) => {
                self.reset_input_history_navigation();
                self.input_cursor = self.input_char_len();
                self.ctrl_c_streak = 0;
            }
            (KeyModifiers::ALT, KeyCode::Enter) => {
                self.queue_prompt_after_turn()?;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_input_char('\n');
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Enter) => {
                self.submit_during_turn(terminal)?;
                self.ctrl_c_streak = 0;
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_input_char(ch);
                self.ctrl_c_streak = 0;
            }
            _ => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
            }
        }
        self.clamp_command_selection();
        self.clamp_file_selection();
        Ok(false)
    }

    pub(super) fn submit_during_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let prompt = self.expanded_input().trim().to_string();
        let display_prompt = self.input.clone();
        let paste_segments = self.paste_segments.clone();
        if prompt.is_empty() && self.shell_mode.is_none() {
            self.clear_submitted_input();
            return Ok(());
        }
        if let Some((mode, command)) = self.shell_submission() {
            if !self.paste_segments.is_empty() {
                return self.block_pasted_inline_shell();
            }
            self.clear_submitted_input();
            self.start_inline_shell(mode, command)?;
            return Ok(());
        }

        match self.parse_input_command() {
            Ok(Some(invocation)) => {
                self.clear_submitted_input();
                self.execute_command_during_turn(invocation, terminal)?;
            }
            Ok(None) => {
                self.queue_steering_prompt(prompt, display_prompt, paste_segments)?;
            }
            Err(commands::CommandParseError::Unknown(name)) => {
                self.clear_submitted_input();
                self.insert_entry(&Entry::Error(format!(
                    "unknown or unavailable command '/{name}' while a model turn is running"
                )));
                self.status = "command unavailable while running".into();
            }
        }
        Ok(())
    }

    pub(super) fn queue_steering_prompt(
        &mut self,
        prompt: String,
        display_prompt: String,
        paste_segments: Vec<PasteSegment>,
    ) -> anyhow::Result<()> {
        self.reset_input_history_navigation();
        self.clear_submitted_input();
        self.steering_prompts.push_back(QueuedPrompt {
            prompt,
            display_prompt,
            paste_segments,
        });
        self.select_pending_recall_target();
        self.insert_entry(&Entry::Notice(format!(
            "queued steer {} for after the current assistant turn",
            self.steering_prompts.len()
        )));
        self.status = format!("queued {} steer(s)", self.steering_prompts.len());
        Ok(())
    }

    pub(super) fn queue_prompt_after_turn(&mut self) -> anyhow::Result<()> {
        let prompt = self.expanded_input().trim().to_string();
        let display_prompt = self.input.clone();
        let paste_segments = self.paste_segments.clone();
        if prompt.is_empty() {
            self.clear_submitted_input();
            return Ok(());
        }
        self.queue_prompt(prompt, display_prompt, paste_segments)
    }

    pub(super) fn queue_prompt(
        &mut self,
        prompt: String,
        display_prompt: String,
        paste_segments: Vec<PasteSegment>,
    ) -> anyhow::Result<()> {
        self.reset_input_history_navigation();
        self.clear_submitted_input();
        self.queued_prompts.push_back(QueuedPrompt {
            prompt,
            display_prompt,
            paste_segments,
        });
        self.select_pending_recall_target();
        self.insert_entry(&Entry::Notice(format!(
            "queued message {} for after the current turn",
            self.queued_prompts.len()
        )));
        self.status = format!("queued {} message(s)", self.queued_prompts.len());
        Ok(())
    }

    pub(super) fn execute_model_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        let model = invocation.args.trim();
        if model.is_empty() {
            self.refresh_available_auths();
            let picker = model_picker::model_picker_during_run(
                &self.info.runtime,
                self.pending_model_selection
                    .as_ref()
                    .map(|pending| &pending.selection),
                &self.available_auths,
            );
            if picker.items.is_empty() {
                self.insert_entry(&Entry::Notice(
                    "no cached API models. refresh model lists from /config after the current run ends."
                        .into(),
                ));
                self.status = "running".into();
            } else {
                self.composer = ComposerMode::Picker(picker);
                self.status = "select model for next turn".into();
            }
            return Ok(());
        }

        self.refresh_available_auths();
        match self.resolve_model_selection(
            model,
            &self.info.runtime.provider,
            &self.info.runtime.auth,
        ) {
            Ok(selection) => self.queue_model_selection(selection),
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "model switch failed".into();
                Ok(())
            }
        }
    }

    pub(super) fn queue_model_selection(
        &mut self,
        selection: InteractiveModelSelection,
    ) -> anyhow::Result<()> {
        let provider_model = format!(
            "{}/{}",
            selection.selection.provider, selection.selection.model
        );
        self.pending_model_selection = Some(selection);
        self.insert_entry(&Entry::Notice(format!(
                "model change to {provider_model} queued; the current agent run will finish on its existing model, and the change will apply after the full run ends"
            )),
        );
        self.status = format!("model queued: {provider_model}");
        Ok(())
    }

    pub(super) fn apply_pending_model_selection(
        &mut self,
        agent: &mut InteractiveRuntime,
        after_successful_turn: bool,
    ) -> anyhow::Result<()> {
        let Some(pending) = self.pending_model_selection.take() else {
            return Ok(());
        };
        if after_successful_turn {
            self.request_model_selection_after_turn(pending, agent)
        } else {
            self.select_model_with_omission_notice(pending, agent)
        }
    }

    pub(super) fn execute_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Exit => self.execute_exit_command(),
            CommandId::Config => self.execute_config_command(terminal),
            CommandId::Info => self.execute_info_command(),
            CommandId::Help => self.execute_help_command(),
            CommandId::Skills => self.execute_skills_command(),
            CommandId::Agents => self.execute_agents_command(),
            CommandId::Diff => self.execute_diff_command(),
            CommandId::Doctor => self.execute_doctor_command(),
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::Goal => self.execute_goal_command_during_turn(invocation),
            CommandId::Model => self.execute_model_command_during_turn(invocation),
            CommandId::Limits => {
                self.start_limits_command();
                Ok(())
            }
            CommandId::New
            | CommandId::Compact
            | CommandId::Login
            | CommandId::Logout
            | CommandId::Resume
            | CommandId::Tree => {
                self.insert_entry(&Entry::Notice(format!(
                    "/{} is unavailable while a model turn is running",
                    invocation.name
                )));
                self.status = "command unavailable while running".into();
                Ok(())
            }
        }
    }

    pub(super) fn handle_running_command_palette_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !self.command_palette_visible() {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.command_selection = if self.command_selection == 0 {
                        matches.len() - 1
                    } else {
                        self.command_selection - 1
                    };
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let matches = self.command_matches();
                if !matches.is_empty() {
                    self.command_selection = (self.command_selection + 1) % matches.len();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.command_palette_dismissed = false;
                    self.clamp_command_selection();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(choice) = self.selected_command() {
                    self.complete_command_choice(&choice);
                    self.clamp_command_selection();
                }
                self.submit_during_turn(terminal)?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.command_palette_dismissed = true;
                self.command_selection = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn submit_picker_selection_during_turn(&mut self) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            self.composer = ComposerMode::Input;
            self.status = "running".into();
            return Ok(());
        };

        let return_picker = self.take_picker_parent_after_selection(action);
        if !matches!(action, PickerAction::Config) {
            self.composer = ComposerMode::Input;
        }
        match action {
            PickerAction::InsertSkillCommand => {
                self.input = format!("/skill:{value}");
                self.input_cursor = self.input_char_len();
                self.command_palette_dismissed = true;
                self.status = "skill command inserted".into();
            }
            PickerAction::ResumeSession | PickerAction::SelectTreeNode => {
                self.insert_entry(&Entry::Notice(
                    "session navigation is unavailable while a model turn is running".into(),
                ));
                self.status = "session navigation unavailable while running".into();
            }
            PickerAction::Config => self.submit_config_selection_during_turn(&value)?,
            PickerAction::Dismiss | PickerAction::ViewAgent => {
                self.status = "running".into();
            }
            PickerAction::SelectInternalAgentModel => {
                self.insert_entry(&Entry::Notice(
                    "internal agent model changes are unavailable while a model turn is running"
                        .into(),
                ));
                self.status = "internal agent model change unavailable while running".into();
            }
            PickerAction::SelectModel => {
                self.refresh_available_auths();
                match self.resolve_model_selection(
                    &value,
                    &self.info.runtime.provider,
                    &self.info.runtime.auth,
                ) {
                    Ok(selection) => self.queue_model_selection(selection)?,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "model switch failed".into();
                    }
                }
            }
            PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::RefreshModelList => {
                self.insert_entry(&Entry::Notice(
                    "that picker action is unavailable while a model turn is running".into(),
                ));
                self.status = "picker action unavailable while running".into();
            }
        }
        if let Some((picker, selected_value)) = return_picker {
            self.open_main_config_picker(selected_value, picker.filter)?;
        }
        Ok(())
    }

    pub(super) fn submit_config_selection_during_turn(
        &mut self,
        value: &str,
    ) -> anyhow::Result<()> {
        match value {
            value if config_picker::is_category(value) => {
                self.open_config_category(value)?;
            }
            config_picker::CONVERSATION_MODEL_VALUE => {
                self.open_config_conversation_model_picker_during_turn();
            }
            config_picker::REFRESH_MODEL_LIST_VALUE
            | config_picker::PROVIDER_LOGIN_VALUE
            | config_picker::PROVIDER_LOGOUT_VALUE => {
                self.insert_entry(&Entry::Notice(
                    "provider configuration is unavailable while a model turn is running".into(),
                ));
                self.status = "config action unavailable while running".into();
            }
            config_picker::PERMISSION_MODE_VALUE => {
                self.reject_permission_mode_change();
            }
            value if value.starts_with(config_picker::PERMISSION_MODE_PREFIX) => {
                self.reject_permission_mode_change();
            }
            config_picker::MAX_OUTPUT_BYTES_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxOutputBytes,
                    config.max_output_bytes,
                ));
                self.status = "edit max output bytes".into();
            }
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxToolOutputLines,
                    config.max_tool_output_lines,
                ));
                self.status = "edit max tool output lines".into();
            }
            config_picker::REASONING_VALUE => {
                self.insert_entry(&Entry::Notice(
                    "reasoning changes are unavailable while a model turn is running".into(),
                ));
                self.status = "config action unavailable while running".into();
            }
            config_picker::SHOW_REASONING_OUTPUT_VALUE => {
                self.toggle_reasoning_output()?;
            }
            config_picker::CHECK_FOR_UPDATES_VALUE => {
                self.toggle_check_for_updates()?;
            }
            config_picker::ENABLE_SUBAGENTS_VALUE => {
                self.toggle_enable_subagents()?;
            }
            config_picker::AUTO_COMPACT_VALUE => {
                self.toggle_auto_compact()?;
            }
            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactThresholdPercent,
                    config.compact_threshold_percent as usize,
                ));
                self.status = "edit compact threshold percent".into();
            }
            config_picker::COMPACT_TARGET_PERCENT_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactTargetPercent,
                    config.compact_target_percent as usize,
                ));
                self.status = "edit compact target percent".into();
            }
            config_picker::INLINE_SHELL_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(config_picker::inline_shell_picker(&config));
                self.status = "select inline shell".into();
            }
            value if value.starts_with(config_picker::INLINE_SHELL_PREFIX) => {
                let shell = value[config_picker::INLINE_SHELL_PREFIX.len()..].to_string();
                self.info.services.config_repository.update(|config| {
                    config.inline_shell.clone_from(&shell);
                })?;
                self.open_main_config_picker_selected(config_picker::INLINE_SHELL_VALUE)?;
                self.status = format!("inline shell: {shell}");
            }
            config_picker::WEB_SEARCH_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(config_picker::web_search_config_picker(
                    &config,
                    self.credential_store.as_ref(),
                ));
                self.status = "web search config".into();
            }
            config_picker::WEB_SEARCH_PROVIDER_VALUE => self.cycle_web_search_provider()?,
            config_picker::WEB_SEARCH_OPENAI_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::OpenAiSearch)?;
            }
            config_picker::WEB_SEARCH_EXA_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::Exa)?;
            }
            config_picker::WEB_SEARCH_BRAVE_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::Brave)?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_running_config_number_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigNumberInput(_)) {
            return Ok(false);
        }
        self.handle_config_number_key(key, terminal)
    }

    pub(super) fn handle_running_config_text_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigTextInput(_)) {
            return Ok(false);
        }
        self.handle_config_text_key(key)
    }

    pub(super) fn next_running_frame_deadline(
        &self,
        deferred_frame_deadline: Option<Instant>,
    ) -> tokio::time::Instant {
        let spinner_deadline = Instant::now() + LoadingSpinner::FRAME_INTERVAL;
        let deadline = deferred_frame_deadline.map_or(spinner_deadline, |deferred_deadline| {
            deferred_deadline.min(spinner_deadline)
        });
        let deadline = self
            .stream_preview_deadline
            .map_or(deadline, |stream_deadline| stream_deadline.min(deadline));
        let deadline = self
            .paste_burst
            .deadline()
            .map_or(deadline, |paste_deadline| paste_deadline.min(deadline));
        tokio::time::Instant::from_std(deadline)
    }

    pub(super) fn handle_running_terminal_events(
        &mut self,
        first_event: Event,
        terminal: &mut DefaultTerminal,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
        input_mode: RunningInputMode,
    ) -> Result<StreamControl, rho_providers::model::ModelError> {
        let mut control = StreamControl::Continue;
        let mut approval_resolved = false;
        let mut next_event = Some(first_event);
        for _ in 0..MAX_TERMINAL_EVENTS_PER_TICK {
            let event = match next_event.take() {
                Some(event) => event,
                None => {
                    let event = self
                        .terminal_events
                        .as_mut()
                        .expect("terminal events initialized")
                        .try_next();
                    let Some(event) = event else {
                        break;
                    };
                    event?
                }
            };
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    self.text_selection = None;
                    if key.code == KeyCode::Esc
                        && matches!(self.composer, ComposerMode::Approval(_))
                    {
                        self.handle_approval_key(key, 1).map_err(|error| {
                            rho_providers::model::ModelError::InvalidResponse(error.to_string())
                        })?;
                        self.cancel_inline_shells();
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                    if key.code == KeyCode::Esc && self.cancel_inline_shells() {
                        continue;
                    }
                    if key.code == KeyCode::Esc && self.exit_shell_mode() {
                        continue;
                    }
                    if key.code == KeyCode::Esc && !self.running_escape_has_overlay_target() {
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                    if input_mode == RunningInputMode::Turn {
                        let resolved =
                            self.handle_key_during_turn(key, terminal).map_err(|err| {
                                rho_providers::model::ModelError::InvalidResponse(err.to_string())
                            })?;
                        approval_resolved |= resolved;
                        if self.pending_input_action.is_some() {
                            break;
                        }
                    }
                    if self.should_quit {
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                }
                Event::Paste(text) if input_mode == RunningInputMode::Turn => {
                    let text = normalize_paste(&text);
                    self.flush_pending_paste_burst();
                    self.insert_paste(&text);
                    self.paste_burst.clear();
                }
                Event::Resize(_, _) => {
                    self.flush_pending_paste_burst();
                    self.clamp_overlay_detail_scroll(terminal);
                    self.text_selection = None;
                    self.hovered_code_block_copy = None;
                    self.hide_history_scrollbar();
                    self.clamp_history_scroll_for_terminal(terminal)?;
                    self.drain_streams(terminal)?;
                    control = StreamControl::Resize;
                }
                Event::Mouse(mouse) if input_mode == RunningInputMode::Turn => {
                    self.handle_mouse_event(mouse.kind, mouse.column, mouse.row, terminal)?;
                }
                Event::FocusGained => {
                    mouse_capture::reassert();
                    self.statusline.refresh_git_branch();
                }
                _ => {}
            }
        }
        self.flush_due_paste_burst();
        if approval_resolved {
            Ok(StreamControl::ApprovalResolved)
        } else {
            Ok(control)
        }
    }

    pub(super) fn running_escape_has_overlay_target(&self) -> bool {
        self.command_palette_visible()
            || self.file_palette_visible()
            || self.pending_input_focused()
            || !matches!(self.composer, ComposerMode::Input)
    }

    pub(super) fn request_running_interrupt(
        &mut self,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
    ) -> StreamControl {
        interrupt_requested.store(true, Ordering::SeqCst);
        if tool_call_active.load(Ordering::SeqCst) {
            self.status = "interrupting tool".into();
        }
        StreamControl::Interrupt
    }
}
