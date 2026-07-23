use std::io::Write;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEventKind};
use ratatui::DefaultTerminal;

use super::{
    mouse_capture, paste_burst::normalize_paste, ActivityPhase, ActivityStatus, App, ComposerMode,
    Entry, HerdrState, HerdrUserWait, InteractiveRuntime, TuiResult, ViewModelEvent,
};

pub(super) fn print_exit_summary(summary: Option<&str>) -> std::io::Result<()> {
    let Some(summary) = summary else {
        return Ok(());
    };
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{summary}")?;
    stdout.flush()
}

impl App {
    pub(super) async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TuiResult> {
        self.start_model_metadata_fetch(agent);
        self.insert_session_intro(terminal)?;
        self.insert_recovered_history(terminal)?;
        self.maybe_offer_loaded_session_context_handoff(agent)?;
        if self.info.session.open_resume_picker {
            self.open_resume_picker()?;
        }
        if self.info.services.auth_unavailable.is_some() {
            self.insert_entry(&Entry::Notice(
                "no providers configured. run /login to sign in.".into(),
            ));
        }
        let mut needs_redraw = true;
        while !self.should_quit {
            let background_ready = self
                .pending_model_metadata
                .as_ref()
                .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_update_notice
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_oauth_login
                    .as_ref()
                    .is_some_and(|pending| pending.handle.is_finished())
                || self
                    .pending_usage_limits
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished());
            self.poll_model_metadata_fetch(agent);
            self.poll_update_notice();
            needs_redraw |= self.poll_pending_session_title()?;
            self.poll_pending_oauth_login(terminal, agent).await?;
            needs_redraw |= self.poll_limits_command().await?;
            needs_redraw |= self.poll_markdown_images();
            let shell_changed = self.finish_completed_inline_shells().await?;
            if !self.is_ui_busy() {
                self.insert_deferred_inline_shell_context(agent)?;
            }
            needs_redraw |= shell_changed;
            needs_redraw |= background_ready;
            needs_redraw |= self.update_subagent_panel(agent);
            needs_redraw |= self.poll_subagent_completions(terminal, agent).await?;
            if needs_redraw {
                terminal.draw(|frame| self.draw(frame))?;
                needs_redraw = false;
            }
            let subagents_active = agent.subagents().is_some_and(|manager| {
                manager.has_active_or_pending_notification(agent.session_id().as_str())
            });
            let idle_timeout = if self.pending_model_metadata.is_some()
                || self.pending_update_notice.is_some()
                || self.pending_session_title.is_some()
                || self.pending_oauth_login.is_some()
                || self.pending_usage_limits.is_some()
                || !self.pending_inline_shells.is_empty()
                || self.history.images().has_pending()
            {
                Duration::from_millis(100)
            } else if subagents_active {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(3600)
            };
            let redraw_on_timeout = self.animation_active(Instant::now());
            let timeout = self.event_poll_timeout(idle_timeout);
            tokio::select! {
                biased;
                event = self.terminal_session.as_mut().expect("terminal session initialized").next_event() => {
                    self.handle_terminal_event(event?, terminal, agent).await?;
                    needs_redraw = true;
                    needs_redraw |= self.flush_due_paste_burst();
                }
                _ = tokio::time::sleep(timeout) => {
                    needs_redraw |= self.flush_due_paste_burst();
                    needs_redraw |= redraw_on_timeout;
                }
            }
        }
        self.cancel_limits_command().await;
        if let Some(mut pending) = self.pending_session_title.take() {
            pending.cancel();
            let _ = (&mut pending).await;
        }
        Ok(TuiResult {
            resume_session_id: self.info.session.session_id.clone(),
            exit_summary: self.exit_summary(),
        })
    }

    pub(super) async fn handle_terminal_event(
        &mut self,
        event: Event,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                self.history.clear_text_selection();
                self.handle_key(key, terminal, agent).await?;
            }
            Event::Paste(text) => {
                self.flush_pending_paste_burst();
                let text = normalize_paste(&text);
                self.insert_paste(&text);
                self.input_ui.clear_paste_burst();
            }
            Event::Resize(_, _) => {
                self.flush_pending_paste_burst();
                self.clamp_overlay_detail_scroll(terminal);
                self.history.clear_text_selection();
                self.history.set_hovered_code_block_copy(None);
                self.hide_history_scrollbar();
                self.clamp_history_scroll_for_terminal(terminal)?;
            }
            Event::Mouse(mouse) => {
                self.handle_mouse_event(mouse.kind, mouse.column, mouse.row, terminal)?;
            }
            Event::FocusGained => {
                // Some Windows hosts drop application mouse tracking on focus
                // changes; re-assert so wheel scrolling keeps working.
                mouse_capture::reassert();
                self.statusline.refresh_git_branch();
            }
            Event::FocusLost | Event::Key(_) => {}
        }
        Ok(())
    }

    pub(super) fn event_poll_timeout(&self, idle_timeout: Duration) -> Duration {
        let now = Instant::now();
        let timeout = self.input_ui.paste_burst().poll_timeout(now, idle_timeout);
        let timeout = self
            .history
            .copy_notice()
            .and_then(|notice| notice.visible_until().checked_duration_since(now))
            .map_or(timeout, |remaining| remaining.min(timeout));
        if self.history.scrollbar_hovered() || self.history.scrollbar_drag().is_some() {
            return timeout;
        }
        self.history
            .scrollbar_visible_until()
            .and_then(|visible_until| visible_until.checked_duration_since(now))
            .map_or(timeout, |remaining| remaining.min(timeout))
    }

    pub(super) fn animation_active(&self, now: Instant) -> bool {
        self.loading_active()
            || self.subagent_panel.is_active()
            || self
                .history
                .copy_notice()
                .is_some_and(|notice| now < notice.visible_until())
            || self.history.scrollbar_hovered()
            || self.history.scrollbar_drag().is_some()
            || self
                .history
                .scrollbar_visible_until()
                .is_some_and(|until| now < until)
    }

    pub(super) async fn report_herdr_state(&self, state: HerdrState, message: Option<&str>) {
        self.info
            .services
            .herdr
            .report_state(state, message, self.info.session.session_id.as_deref())
            .await;
    }

    pub(super) async fn report_herdr_working(&self) {
        self.report_herdr_state(HerdrState::Working, None).await;
    }

    pub(super) async fn report_herdr_waiting_for_user(&self, wait: HerdrUserWait) {
        self.report_herdr_state(HerdrState::Blocked, Some(wait.message()))
            .await;
    }

    pub(super) async fn report_resting_herdr_state(&self) {
        let goal_blocked_reason = self
            .goal
            .as_ref()
            .filter(|goal| goal.is_blocked())
            .and_then(|goal| goal.last_reason.as_deref());
        let message = self
            .info
            .services
            .auth_unavailable
            .as_deref()
            .or(goal_blocked_reason);
        let state = if message.is_some() {
            HerdrState::Blocked
        } else {
            HerdrState::Idle
        };
        self.report_herdr_state(state, message).await;
    }

    pub(super) fn activity_status(&self) -> Option<ActivityStatus> {
        let phase = match self.input_ui.composer() {
            ComposerMode::Approval(_) => ActivityPhase::WaitingForApproval,
            ComposerMode::Questionnaire(_) => ActivityPhase::WaitingForInput,
            _ => self.turn.activity_phase(),
        };
        ActivityStatus::from_parent_and_subagents(
            self.loading_active().then_some(phase),
            self.subagent_panel.count(),
        )
    }

    pub(super) fn update_subagent_panel(&mut self, agent: &InteractiveRuntime) -> bool {
        let changed = self.subagent_panel.update(agent.subagents());
        if self.subagent_panel.is_active() {
            self.turn.start_loading_if_needed();
        }
        changed
    }

    pub(super) fn loading_active(&self) -> bool {
        self.is_ui_busy() || self.streams.loading_streams_active()
    }

    pub(super) fn handle_queued_agent_event(
        &mut self,
        event: ViewModelEvent,
        terminal: &mut DefaultTerminal,
    ) -> Result<bool, rho_providers::model::ModelError> {
        Ok(self.handle_agent_event(event, terminal)?)
    }

    pub(super) fn reset_usage(&mut self) {
        self.usage.cumulative_usage = None;
        self.usage.usage_cost_tracker.reset();
        self.usage.usage_before_current_run = None;
        self.usage.usage_before_current_step = None;
        self.usage.usage_before_current_attempt = None;
        self.usage.current_run_usage = None;
        self.usage.latest_usage = None;
    }

    pub(super) fn exit_summary(&self) -> Option<String> {
        self.info
            .session
            .session_id
            .as_ref()
            .map(|session_id| format!("rho session saved: {session_id}"))
    }
}
