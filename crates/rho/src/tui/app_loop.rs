use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEventKind};
use ratatui::DefaultTerminal;

use super::{
    media_attach, mouse_capture, ActivityPhase, ActivityStatus, App, BackgroundCounts,
    ComposerMode, ExitReceipt, HerdrState, HerdrUserWait, InteractiveRuntime, ViewModelEvent,
};

impl App {
    fn insert_recovered_history(&mut self) -> std::io::Result<bool> {
        let messages = std::mem::take(&mut self.info.session.recovered_messages);
        let had_recovered_messages = !messages.is_empty();
        let entries = self.transcript_entries(&messages);
        if entries.is_empty() {
            return Ok(had_recovered_messages);
        }

        self.set_history_entries(entries);
        Ok(had_recovered_messages)
    }

    pub(super) async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<Option<ExitReceipt>> {
        self.start_model_metadata_fetch(agent);
        self.start_github_pr_fetch();
        let had_recovered_messages = self.insert_recovered_history()?;
        self.maybe_offer_loaded_session_context_handoff(agent, had_recovered_messages)?;
        let open_resume_after_draw = self.info.session.open_resume_picker;
        self.info.session.open_resume_picker = false;
        // A first launch opens the full-screen setup instead of a session.
        // Afterwards the header and statusline carry setup state, so a
        // signed-out session needs no history entry that would scroll away.
        self.start_setup_screen(terminal);
        self.reconcile_auto_classifier_gate(agent).await?;
        if agent.mcp_connect_pending() {
            self.set_status_quiet("connecting MCP servers");
        }
        let mut needs_redraw = true;
        let mut first_frame = true;
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
                    .pending_interactive_login
                    .as_ref()
                    .is_some_and(|pending| pending.handle.is_finished())
                || self
                    .pending_usage_limits
                    .iter()
                    .any(super::limits_command::PendingUsageFetch::is_finished)
                || self
                    .pending_doctor_probes
                    .iter()
                    .any(super::doctor_overlay::PendingDoctorProbe::is_finished)
                || self
                    .pending_changelog
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_custom_models
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_cursor_models
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_syntax_warmup
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_herdr_graphics
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
                || self
                    .pending_github_pr
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
                || self.prompt_history.load_finished()
                || agent.startup_hydrate_pending();
            self.poll_model_metadata_fetch(agent).await;
            needs_redraw |= self.poll_startup_hydrates(agent).await?;
            needs_redraw |= self.poll_compact(agent).await?;
            needs_redraw |= self.release_pending_held_turn(terminal, agent).await?;
            needs_redraw |= self.start_next_follow_up(terminal, agent).await?;
            if !first_frame {
                needs_redraw |= self.start_startup_prompt(terminal, agent).await?;
            }
            self.poll_update_notice();
            self.poll_custom_provider_models();
            self.poll_cursor_model_refresh().await;
            needs_redraw |= self.poll_syntax_warmup();
            self.poll_herdr_graphics();
            needs_redraw |= self.poll_prompt_history();
            needs_redraw |= self.poll_pending_session_title()?;
            self.poll_pending_interactive_login(terminal, agent).await?;
            needs_redraw |= self.poll_limits_command().await?;
            needs_redraw |= self.poll_doctor_command().await?;
            needs_redraw |= self.poll_side_chat();
            needs_redraw |= self.poll_changelog_command().await?;
            // Runs on every pass because the composer is what decides whether
            // there is anything to ask about, and it changes on key events
            // rather than on a schedule of its own.
            needs_redraw |= self.poll_mcp_argument_completion().await;
            needs_redraw |= self.poll_markdown_images();
            let shell_changed = self.finish_completed_inline_shells().await?;
            if !self.is_ui_busy() {
                self.insert_deferred_inline_shell_context(agent)?;
            }
            needs_redraw |= shell_changed;
            needs_redraw |= background_ready;
            needs_redraw |= self.update_activity_panels(agent)?;
            needs_redraw |= self
                .poll_subagent_questionnaires(agent.session_id())
                .await?;
            // Subscribe before draining so an exit between drain and wait is not lost.
            let process_exit = agent
                .processes()
                .map(crate::tools::process::ProcessManager::notified_owned);
            needs_redraw |= self.poll_subagent_completions(terminal, agent).await?;
            if needs_redraw {
                terminal.draw(|frame| self.draw(frame))?;
                needs_redraw = false;
                if first_frame {
                    first_frame = false;
                    if open_resume_after_draw {
                        self.open_resume_picker()?;
                    }
                    // Only skip the first event wait when startup work still
                    // needs a loop-top poll. A blanket continue changed paste
                    // timing on ordinary launches.
                    if open_resume_after_draw || self.info.session.startup_prompt.is_some() {
                        needs_redraw = true;
                        continue;
                    }
                }
            }
            let subagents_active = agent.subagents().is_some_and(|manager| {
                manager.has_active_or_pending_notification(agent.session_id().as_str())
            }) || agent
                .workflow_tracker()
                .has_active_or_pending_notification(agent.session_id().as_str())
                || agent
                    .processes()
                    .is_some_and(crate::tools::process::ProcessManager::has_pending_notification)
                || self.pending_subagent_questionnaire.is_some()
                || self.subagent_inbox.has_queued_questionnaires()
                || self.subagent_inbox.has_parent_action_requests();
            let idle_timeout = if self.pending_model_metadata.is_some()
                || self.pending_update_notice.is_some()
                || self.pending_custom_models.is_some()
                || self.pending_cursor_models.is_some()
                || self.pending_syntax_warmup.is_some()
                || self.pending_herdr_graphics.is_some()
                || self.pending_github_pr.is_some()
                || self.prompt_history.load_pending()
                || self.pending_session_title.is_some()
                || self.pending_interactive_login.is_some()
                || !self.pending_usage_limits.is_empty()
                || !self.pending_doctor_probes.is_empty()
                || self.pending_changelog.is_some()
                || self.mcp_argument_completions.is_pending()
                || self.exclusive.wants_fast_ticks()
                || !self.pending_inline_shells.is_empty()
                || self.history.images().has_pending()
                || agent.startup_hydrate_pending()
                || self.loading_active()
            {
                Duration::from_millis(100)
            } else if subagents_active || self.process_panel.is_active() {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(3600)
            };
            let redraw_on_timeout = self.animation_active(Instant::now());
            let timeout = self.event_poll_timeout(idle_timeout);
            let media_attach_pending = !self.media_attach_tasks.is_empty();
            tokio::select! {
                biased;
                event = self.terminal_session.as_mut().expect("terminal session initialized").next_event() => {
                    self.handle_terminal_event(event?, terminal, agent).await?;
                    needs_redraw = true;
                    needs_redraw |= self.flush_due_paste_burst();
                }
                () = self.subagent_inbox.recv() => {
                    needs_redraw = true;
                }
                () = async {
                    match process_exit {
                        Some(notified) => notified.await,
                        None => std::future::pending().await,
                    }
                } => {
                    needs_redraw = true;
                }
                outcome = media_attach::next_media_attach_completion(&mut self.media_attach_tasks), if media_attach_pending => {
                    self.finish_media_attach(outcome);
                    needs_redraw = true;
                }
                _ = tokio::time::sleep(timeout) => {
                    needs_redraw |= self.flush_due_paste_burst();
                    needs_redraw |= redraw_on_timeout;
                }
            }
        }
        self.prompt_history.flush();
        self.abort_compact(agent).await;
        self.cancel_limits_command().await;
        self.cancel_doctor_command().await;
        self.cancel_changelog_command().await;
        if let Some(handle) = self.pending_cursor_models.take() {
            handle.abort();
        }
        agent.cancel_startup_hydrates();
        self.mcp_argument_completions.cancel();
        if let Some(mut pending) = self.pending_session_title.take() {
            pending.cancel();
            let _ = (&mut pending).await;
        }
        Ok(self.exit_receipt())
    }

    pub(super) async fn handle_terminal_event(
        &mut self,
        event: Event,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match self.take_exclusive_event(event) {
            Ok(resize) => {
                if resize {
                    self.apply_terminal_resize(terminal)?;
                }
                return Ok(());
            }
            Err(event) => match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    self.clear_selections();
                    self.subagent_panel.clear_pointer_state();
                    self.process_panel.clear_pointer_state();
                    self.handle_key(key, terminal, agent).await?;
                }
                Event::Paste(text) => {
                    self.input_ui.cancel_pointer_click_sequence();
                    self.apply_external_paste(&text);
                }
                Event::Resize(_, _) => {
                    self.apply_terminal_resize(terminal)?;
                }
                Event::Mouse(mouse) => {
                    self.flush_pending_paste_burst();
                    self.handle_mouse_event(mouse.kind, mouse.column, mouse.row, terminal)?;
                }
                Event::FocusGained => self.on_focus_gained(),
                Event::FocusLost => {
                    self.input_ui.cancel_pointer_click_sequence();
                    self.input_ui.finalize_selection();
                    self.subagent_panel.clear_pointer_state();
                    self.process_panel.clear_pointer_state();
                }
                Event::Key(_) => {}
            },
        }
        Ok(())
    }

    pub(super) fn on_focus_gained(&mut self) {
        self.input_ui.cancel_pointer_click_sequence();
        // Some Windows hosts drop application mouse tracking on focus
        // changes; re-assert so wheel scrolling keeps working.
        mouse_capture::reassert();
        self.refresh_workspace_on_focus();
    }

    pub(super) fn event_poll_timeout(&self, idle_timeout: Duration) -> Duration {
        let now = Instant::now();
        let timeout = self.input_ui.paste_burst().poll_timeout(now, idle_timeout);
        let timeout = self
            .history
            .copy_notice()
            .and_then(|notice| notice.visible_until().checked_duration_since(now))
            .map_or(timeout, |remaining| remaining.min(timeout));
        let timeout = self
            .status_overlay
            .as_ref()
            .and_then(|overlay| overlay.visible_until().checked_duration_since(now))
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
            || self.exclusive_should_redraw(now)
            || self.subagent_panel.is_active()
            || self.process_panel.is_active()
            || self
                .history
                .copy_notice()
                .is_some_and(|notice| now < notice.visible_until())
            || self
                .status_overlay
                .as_ref()
                .is_some_and(|overlay| overlay.is_visible(now))
            || matches!(
                self.input_ui.composer(),
                ComposerMode::Limits(overlay) if overlay.is_checking()
            )
            || matches!(
                self.input_ui.composer(),
                ComposerMode::Doctor(overlay) if overlay.is_checking()
            )
            || self.side_chat_busy()
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
        let user_wait = match self.input_ui.composer() {
            ComposerMode::Approval(_) => Some(HerdrUserWait::Approval),
            ComposerMode::Questionnaire(_) => Some(HerdrUserWait::Questionnaire),
            ComposerMode::Input
            | ComposerMode::Picker(_)
            | ComposerMode::Limits(_)
            | ComposerMode::Doctor(_)
            | ComposerMode::Side
            | ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::TextInput(_)
            | ComposerMode::InteractivePending(_)
            | ComposerMode::InlineChoice(_) => None,
        };
        if let Some(wait) = user_wait {
            self.report_herdr_waiting_for_user(wait).await;
            return;
        }
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
        // Keep the activity rail in zen mode. Zen only strips transcript chrome
        // (tools, reasoning, Thinking...); live status still belongs on screen so
        // subagent rows are not an orphaned full-width strip.
        let phase = match self.input_ui.composer() {
            ComposerMode::Approval(_) => ActivityPhase::WaitingForApproval,
            ComposerMode::Questionnaire(_) => ActivityPhase::WaitingForInput,
            _ => self.turn.activity_phase(),
        };
        let retry = match self.input_ui.composer() {
            ComposerMode::Approval(_) | ComposerMode::Questionnaire(_) => None,
            _ => self.turn.provider_retry(),
        };
        ActivityStatus::from_parent_and_background(
            self.loading_active().then_some((phase, retry)),
            BackgroundCounts {
                subagent_count: self.subagent_panel.count(),
                job_count: self.process_panel.live_count(),
            },
            self.subagent_panel.is_active() || self.process_panel.is_active(),
        )
    }

    pub(super) fn apply_terminal_resize(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        self.flush_pending_paste_burst();
        self.clamp_overlay_detail_scroll(terminal);
        self.clamp_limits_overlay_scroll(terminal);
        self.clamp_doctor_overlay_scroll(terminal);
        self.clear_selections();
        self.clear_hovered_copy_buttons();
        self.subagent_panel.clear_pointer_state();
        self.process_panel.clear_pointer_state();
        self.hide_history_scrollbar();
        self.clamp_history_scroll_for_terminal(terminal)
    }

    pub(super) fn update_activity_panels(
        &mut self,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        // Tick the occupant first so a journal read error cannot leave panel
        // costs half-applied.
        let mut changed = self.refresh_exclusive_screen()?;
        let now = Instant::now();
        let panel_changed = self.subagent_panel.update(agent.subagents(), now);
        if panel_changed {
            self.refresh_attach_picker();
        }
        changed |= panel_changed;
        changed |= self.process_panel.update(agent.processes(), now);
        // Fold terminal subagent/advisor costs on every panel refresh path (idle
        // poll, in-turn wait, goal wait). Claiming is idempotent per run/call.
        changed |= self.claim_non_main_costs(agent);
        changed |= self.poll_github_pr();
        self.restore_mcp_hold_activity_if_needed(agent.mcp_connect_pending());
        if self.activity_status().is_some() {
            self.turn.start_loading_if_needed();
        }
        Ok(changed)
    }

    /// Pull finished non-main costs into the parent session total.
    fn claim_non_main_costs(&mut self, agent: &InteractiveRuntime) -> bool {
        let mut changed = false;
        if let Some(manager) = agent.subagents() {
            let claimed = manager.claim_terminal_costs_usd_micros(agent.session_id().as_str());
            if claimed > 0 {
                self.usage.subagent_total_cost_usd_micros = self
                    .usage
                    .subagent_total_cost_usd_micros
                    .saturating_add(claimed);
                changed = true;
            }
        }
        if let Some(advisor) = agent.advisor() {
            let claimed = advisor.claim_cost_usd_micros();
            if claimed > 0 {
                self.usage.advisor_total_cost_usd_micros = self
                    .usage
                    .advisor_total_cost_usd_micros
                    .saturating_add(claimed);
                changed = true;
            }
        }
        changed
    }

    pub(super) fn loading_active(&self) -> bool {
        self.is_ui_busy()
            || self.streams.loading_streams_active()
            || matches!(self.turn.activity_phase(), ActivityPhase::ConnectingMcp)
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
        self.usage.run_usage.clear();
        self.usage.subagent_total_cost_usd_micros = 0;
        self.usage.advisor_total_cost_usd_micros = 0;
        self.usage.latest_usage = None;
        self.usage.model_performance.clear();
        self.usage.cache_stats.reset();
    }

    fn exit_receipt(&self) -> Option<ExitReceipt> {
        self.info.session.session_id.as_ref().map(|session_id| {
            ExitReceipt::capture(
                session_id.clone(),
                self.session_title(),
                self.usage.cumulative_usage.as_ref(),
                self.model_metadata.as_ref(),
                self.usage.extra_cost_usd_micros(),
            )
        })
    }
}
