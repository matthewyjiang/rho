//! Input and command handling while a model turn is running.
//!
//! Owns key routing, steering/follow-up queues, during-turn slash commands,
//! running picker/config overlays, and terminal event routing for the live
//! turn loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use super::{
    activity::LoadingSpinner,
    command_palette::CommandPaletteKeyOutcome,
    commands::{self, CommandId, CommandInvocation},
    App, ApprovalKeyOutcome, ComposerMode, Entry, HistoryDirection, InputSubmissionMode,
    InteractiveModelSelection, InteractiveRuntime, PasteSegment, QueuedPrompt, StreamControl,
};

/// What Esc does in the live TUI.
///
/// Approval deny-and-abort stays first. Visible or focused overlays and
/// non-input composer modes then own Esc, matching during-turn key routing,
/// so background pending inline shells and shell mode cannot steal a key
/// the overlay would handle. Event handling and the empty-composer hint
/// share this so chrome never advertises abort unless Esc would actually
/// interrupt through the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunningEscapeAction {
    DenyApprovalAndAbort,
    CancelInlineShells,
    ExitShellMode,
    Overlay,
    AbortTurn,
}

pub(super) enum RunningTerminalError {
    Recoverable(rho_providers::model::ModelError),
    Terminal(anyhow::Error),
}

impl RunningTerminalError {
    pub(super) fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Recoverable(error) => error.into(),
            Self::Terminal(error) => error,
        }
    }
}

impl From<std::io::Error> for RunningTerminalError {
    fn from(error: std::io::Error) -> Self {
        Self::Terminal(error.into())
    }
}

impl From<rho_providers::model::ModelError> for RunningTerminalError {
    fn from(error: rho_providers::model::ModelError) -> Self {
        Self::Recoverable(error)
    }
}

impl App {
    pub(super) async fn handle_key_during_turn(
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

        let size = terminal.size()?;
        match self.handle_approval_key(key, size.width as usize, size.height as usize)? {
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
        if self.handle_running_text_input_key(key)? {
            return Ok(false);
        }
        if self.handle_running_picker_key(key, terminal).await? {
            return Ok(false);
        }
        if self.handle_limits_overlay_key(key, terminal) {
            return Ok(false);
        }
        if self.handle_doctor_overlay_key(key, terminal) {
            return Ok(false);
        }
        if self.handle_side_chat_key(key, terminal) {
            return Ok(false);
        }
        match self.handle_command_palette_key(key) {
            CommandPaletteKeyOutcome::Ignored => {}
            CommandPaletteKeyOutcome::Handled => return Ok(false),
            CommandPaletteKeyOutcome::Submit => {
                self.submit_during_turn(terminal).await?;
                return Ok(false);
            }
        }
        if self.handle_file_palette_key(key)? {
            return Ok(false);
        }
        // Same order as the idle composer: pin cycle wins when a user binds
        // `cycle_pinned_model` onto a key that `handle_configurable_*` also
        // owns (for example Ctrl-P rebound to `toggle_tool_output`).
        if self.handle_running_favorite_cycle_key(key)? {
            return Ok(false);
        }
        if self.handle_configurable_running_key(key, terminal)? {
            return Ok(false);
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
            (modifiers, KeyCode::Enter) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_input_char('\n');
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Enter) => {
                self.submit_during_turn(terminal).await?;
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
        Ok(false)
    }

    pub(super) async fn submit_during_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let prompt = self.expanded_input().trim().to_string();
        let display_prompt = self.input_ui.text().to_string();
        let paste_segments = self.input_ui.paste_segments().to_vec();
        if prompt.is_empty() && self.input_ui.shell_mode().is_none() {
            self.clear_submitted_input();
            return Ok(());
        }
        if let Some((mode, command)) = self.shell_submission() {
            if !self.input_ui.paste_segments().is_empty() {
                return self.block_pasted_inline_shell();
            }
            self.clear_submitted_input();
            self.start_inline_shell(mode, command)?;
            return Ok(());
        }

        match self.parse_input_command() {
            Ok(Some(invocation)) => {
                self.clear_submitted_input();
                self.execute_command_during_turn(invocation, terminal)
                    .await?;
            }
            Ok(None) => {
                self.queue_steering_prompt(prompt, display_prompt, paste_segments)?;
            }
            Err(commands::CommandParseError::Unknown(name)) => {
                self.clear_submitted_input();
                self.insert_entry(&Entry::Error(format!(
                    "unknown or unavailable command '/{name}' while a model turn is running"
                )));
                self.set_status("command unavailable while running");
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
        self.pending.steering_prompts_mut().push_back(QueuedPrompt {
            prompt,
            display_prompt,
            paste_segments,
            media: Vec::new(),
        });
        self.select_pending_recall_target();
        self.set_status(format!(
            "queued steer {} for after the current assistant turn",
            self.pending.steering_prompts().len()
        ));
        Ok(())
    }

    pub(super) fn queue_prompt_after_turn(&mut self) -> anyhow::Result<()> {
        let prompt = self.expanded_input().trim().to_string();
        let display_prompt = self.input_ui.text().to_string();
        let paste_segments = self.input_ui.paste_segments().to_vec();
        if prompt.is_empty() {
            self.clear_submitted_input();
            return Ok(());
        }
        self.queue_prompt(prompt, display_prompt, paste_segments, Vec::new())
    }

    pub(super) fn queue_prompt(
        &mut self,
        prompt: String,
        display_prompt: String,
        paste_segments: Vec<PasteSegment>,
        media: Vec<super::ChatMedia>,
    ) -> anyhow::Result<()> {
        self.reset_input_history_navigation();
        self.clear_submitted_input();
        self.pending.push_follow_up(QueuedPrompt {
            prompt,
            display_prompt,
            paste_segments,
            media,
        });
        self.select_pending_recall_target();
        self.pending_input_changed();
        let when = if self.turn.is_compacting() {
            "after compact"
        } else {
            "after the current turn"
        };
        self.set_status(format!(
            "queued message {} for {when}",
            self.pending.queued_prompts().len()
        ));
        Ok(())
    }

    pub(super) fn execute_model_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        let model = invocation.args.trim();
        if model.is_empty() {
            self.refresh_available_auths();
            let picker = self.conversation_model_picker_during_run();
            if picker.items.is_empty() {
                self.report_missing_cached_provider_models();
            } else {
                self.input_ui.set_composer(ComposerMode::Picker(picker));
                self.set_status("select model for next turn");
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
                self.set_status("model switch failed");
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
        self.set_status(format!(
            "model change to {provider_model} queued for after this run"
        ));
        Ok(())
    }

    pub(super) async fn apply_pending_model_selection(
        &mut self,
        agent: &mut InteractiveRuntime,
        after_successful_turn: bool,
    ) -> anyhow::Result<()> {
        let Some(pending) = self.pending_model_selection.take() else {
            return Ok(());
        };
        if after_successful_turn {
            self.request_model_selection_after_turn(pending, agent)
                .await
        } else {
            self.select_model_with_omission_notice(pending, agent).await
        }
    }

    pub(super) async fn execute_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match invocation.id {
            CommandId::Exit => self.execute_exit_command(),
            CommandId::Theme => self.open_theme_picker(),
            CommandId::Config => self.execute_config_command(terminal),
            CommandId::Info => self.execute_info_command().await,
            CommandId::Help => self.execute_help_command(),
            CommandId::Skills => self.execute_skills_command(),
            CommandId::Agents => self.execute_agents_command(),
            CommandId::Attach => self.execute_attach_command(),
            CommandId::Changelog => self.execute_changelog_command(&invocation, terminal),
            CommandId::Diff => self.execute_diff_command(),
            CommandId::Doctor => self.start_doctor_command(),
            CommandId::Copy => self.execute_copy_command(),
            CommandId::Export => self.execute_export_command(&invocation),
            CommandId::Mcp => self.execute_mcp_command(),
            CommandId::Title => self.execute_title_command(&invocation),
            CommandId::Goal => self.execute_goal_command_during_turn(invocation),
            CommandId::Model => self.execute_model_command_during_turn(invocation),
            CommandId::Limits => {
                self.start_limits_command();
                Ok(())
            }
            CommandId::Side => self.execute_side_command(invocation).await,
            CommandId::CreateAgent => {
                self.set_status("agent creation is unavailable while a model turn is running");
                Ok(())
            }
            CommandId::Advisor
            | CommandId::Hooks
            | CommandId::New
            | CommandId::Fast
            | CommandId::Compact
            | CommandId::Login
            | CommandId::Logout
            | CommandId::RefreshModels
            | CommandId::Resume
            | CommandId::Rewind
            | CommandId::Sessions
            | CommandId::Tree
            | CommandId::Workflow => {
                self.set_status(format!(
                    "/{} is unavailable while a model turn is running",
                    invocation.name
                ));
                Ok(())
            }
        }
    }

    pub(super) fn handle_running_config_number_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), ComposerMode::ConfigNumberInput(_)) {
            return Ok(false);
        }
        self.handle_config_number_key(key, terminal)
    }

    pub(super) fn handle_running_text_input_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), ComposerMode::TextInput(_)) {
            return Ok(false);
        }
        self.handle_text_input_key(key)
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
            .streams
            .stream_tick_deadline
            .map_or(deadline, |stream_deadline| stream_deadline.min(deadline));
        let deadline = self
            .input_ui
            .paste_burst()
            .deadline()
            .map_or(deadline, |paste_deadline| paste_deadline.min(deadline));
        tokio::time::Instant::from_std(deadline)
    }

    pub(super) async fn handle_running_terminal_events(
        &mut self,
        first_event: Event,
        terminal: &mut DefaultTerminal,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
    ) -> Result<StreamControl, RunningTerminalError> {
        let mut control = StreamControl::Continue;
        let mut approval_resolved = false;
        'event: {
            match self.take_exclusive_event(first_event) {
                Ok(resize) => {
                    if resize {
                        self.apply_terminal_resize(terminal)?;
                        self.drain_streams(terminal)?;
                        control = StreamControl::Resize;
                    }
                    if self.should_quit {
                        return Ok(
                            self.request_running_interrupt(interrupt_requested, tool_call_active)
                        );
                    }
                    break 'event;
                }
                Err(first_event) => match first_event {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.clear_selections();
                        self.clear_rail_pointer_state();
                        if key.code == KeyCode::Esc {
                            match self.running_escape_action() {
                                Some(RunningEscapeAction::DenyApprovalAndAbort) => {
                                    self.handle_approval_key(key, 1, 1).map_err(|error| {
                                        RunningTerminalError::Recoverable(
                                            rho_providers::model::ModelError::InvalidResponse(
                                                error.to_string(),
                                            ),
                                        )
                                    })?;
                                    self.cancel_inline_shells();
                                    return Ok(self.request_running_interrupt(
                                        interrupt_requested,
                                        tool_call_active,
                                    ));
                                }
                                Some(RunningEscapeAction::CancelInlineShells) => {
                                    let _ = self.cancel_inline_shells();
                                    break 'event;
                                }
                                Some(RunningEscapeAction::ExitShellMode) => {
                                    let _ = self.exit_shell_mode();
                                    break 'event;
                                }
                                Some(RunningEscapeAction::AbortTurn) => {
                                    return Ok(self.request_running_interrupt(
                                        interrupt_requested,
                                        tool_call_active,
                                    ));
                                }
                                Some(RunningEscapeAction::Overlay) | None => {}
                            }
                        }
                        if self.external_editor_shortcut_matches(key) {
                            self.open_composer_in_editor(terminal)
                                .await
                                .map_err(RunningTerminalError::Terminal)?;
                            control = StreamControl::Resize;
                            break 'event;
                        }
                        let resolved =
                            self.handle_key_during_turn(key, terminal)
                                .await
                                .map_err(|err| {
                                    RunningTerminalError::Recoverable(
                                        rho_providers::model::ModelError::InvalidResponse(
                                            err.to_string(),
                                        ),
                                    )
                                })?;
                        approval_resolved |= resolved;
                        if self.pending.input_action().is_some() {
                            break 'event;
                        }
                        if self.should_quit {
                            return Ok(self
                                .request_running_interrupt(interrupt_requested, tool_call_active));
                        }
                    }
                    Event::Paste(text) => {
                        self.input_ui.cancel_pointer_click_sequence();
                        self.apply_external_paste(&text);
                    }
                    Event::Resize(_, _) => {
                        self.apply_terminal_resize(terminal)?;
                        self.drain_streams(terminal)?;
                        control = StreamControl::Resize;
                    }
                    Event::Mouse(mouse) => {
                        self.flush_pending_paste_burst();
                        self.handle_mouse_event(mouse.kind, mouse.column, mouse.row, terminal)?;
                    }
                    Event::FocusGained => self.on_focus_gained(),
                    Event::FocusLost => {
                        self.input_ui.cancel_pointer_click_sequence();
                        self.input_ui.finalize_selection();
                        self.clear_rail_pointer_state();
                    }
                    _ => {}
                },
            }
        }
        self.flush_due_paste_burst();
        if approval_resolved {
            Ok(StreamControl::ApprovalResolved)
        } else {
            Ok(control)
        }
    }

    pub(super) fn running_escape_action(&mut self) -> Option<RunningEscapeAction> {
        if matches!(self.input_ui.composer(), ComposerMode::Approval(_)) {
            Some(RunningEscapeAction::DenyApprovalAndAbort)
        } else if self.running_escape_has_overlay_target() {
            Some(RunningEscapeAction::Overlay)
        } else if !self.pending_inline_shells.is_empty() {
            Some(RunningEscapeAction::CancelInlineShells)
        } else if self.input_ui.shell_mode().is_some() {
            Some(RunningEscapeAction::ExitShellMode)
        } else if self.turn.session_ui().esc_aborts_operation() {
            Some(RunningEscapeAction::AbortTurn)
        } else {
            None
        }
    }

    fn running_escape_has_overlay_target(&mut self) -> bool {
        self.active_palette().is_some()
            || self.pending_input_focused()
            || !matches!(self.input_ui.composer(), ComposerMode::Input)
    }

    /// Empty-composer abort copy. Overlays, shells, and palettes are already
    /// excluded: this path only paints when the input is empty.
    pub(super) fn composer_shows_abort_hint(&self) -> bool {
        matches!(self.input_ui.composer(), ComposerMode::Input)
            && self.input_ui.shell_mode().is_none()
            && !self.pending_input_focused()
            && self.pending_inline_shells.is_empty()
            && self.turn.session_ui().esc_aborts_operation()
    }

    pub(super) fn request_running_interrupt(
        &mut self,
        interrupt_requested: &AtomicBool,
        tool_call_active: &AtomicBool,
    ) -> StreamControl {
        interrupt_requested.store(true, Ordering::SeqCst);
        if tool_call_active.load(Ordering::SeqCst) {
            self.set_status("interrupting tool");
        }
        StreamControl::Interrupt
    }
}

#[cfg(test)]
#[path = "during_turn_tests.rs"]
mod tests;
