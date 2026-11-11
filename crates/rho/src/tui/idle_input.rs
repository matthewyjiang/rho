use crate::tui::DefaultTerminal;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    command_actions::CommandSubmission,
    command_palette::{slash_command_args, CommandPaletteKeyOutcome},
    commands,
    composer_buffer::ComposerEditKey,
    send_confirm::{SendAuthorization, SendPayload, SendSubmission},
    skill_actions, ActivityPhase, App, ChatMedia, ComposerMode, InputSubmissionMode,
    InteractiveRuntime, PasteSegment, PendingLoginFinish, QueuedPrompt, TurnPrompt,
};

/// A turn held until MCP servers and OpenAPI sources finish loading.
pub(super) struct HeldTurn {
    pub(super) turn: TurnPrompt,
    pub(super) media: Vec<ChatMedia>,
    /// Kept so `esc` can hand the prompt back exactly as it was typed.
    pub(super) paste_segments: Vec<PasteSegment>,
    pub(super) origin: HeldOrigin,
}

/// Where a held turn came from, which decides how it is sent once its wait
/// is over and what `esc` does with it meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HeldOrigin {
    /// The composer: `esc` hands the prompt back to it.
    Composer,
    /// The composer, for a prompt to a daemon session the person confirmed
    /// for this model before it was held (`send_confirm::daemon`): released,
    /// it is asked about again only if the session's model changed since.
    Confirmed(rho_sdk::model::ModelIdentity),
    /// A command of a daemon session that is no send to confirm, as
    /// `/compact`: `esc` hands it back to the composer.
    Command,
    /// The workflow hub of a daemon session, whose launch is a prompt the
    /// person never typed: `esc` cancels it.
    WorkflowHub,
}

impl App {
    fn take_command_submission(
        &mut self,
        invocation: super::CommandInvocation,
        turn: TurnPrompt,
    ) -> CommandSubmission {
        let media = self
            .input_ui
            .take_ready_media()
            .expect("pending attachments block submission");
        let paste_segments = self.input_ui.paste_segments().to_vec();
        let submission = CommandSubmission::new(invocation, turn, media, paste_segments);
        self.clear_submitted_input();
        submission
    }

    /// Route keys owned by modal/overlay composers. Returns true when handled.
    fn handle_composer_mode_key<'a>(
        &'a mut self,
        key: KeyEvent,
        terminal: &'a mut DefaultTerminal,
        agent: &'a mut InteractiveRuntime,
    ) -> impl std::future::Future<Output = anyhow::Result<bool>> + 'a {
        // Keep modal futures out of every idle key's poll frame, including
        // ordinary Enter and /exit. Allocate in a synchronous frame so debug
        // construction temporaries are gone before polling the handler.
        Box::pin(self.handle_composer_mode_key_inner(key, terminal, agent))
    }

    async fn handle_composer_mode_key_inner(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        match self.input_ui.composer() {
            ComposerMode::Input => Ok(false),
            ComposerMode::InteractivePending(pending)
                if matches!(pending.finish, PendingLoginFinish::UserConfirms(_))
                    && key.code == KeyCode::Enter
                    && key.modifiers.is_empty() =>
            {
                self.confirm_pending_login(terminal, agent).await?;
                Ok(true)
            }
            ComposerMode::InteractivePending(_) => self.handle_interactive_pending_key(key),
            ComposerMode::InlineChoice(_) => {
                self.handle_inline_choice_key(key, terminal, agent).await
            }
            ComposerMode::Questionnaire(_) => self.handle_questionnaire_key(key),
            ComposerMode::SecretInput(_) => self.handle_secret_key(key, terminal, agent).await,
            ComposerMode::TextInput(_) => self.handle_text_input_key(key),
            ComposerMode::Picker(_) => self.handle_picker_key(key, terminal, agent).await,
            ComposerMode::Panel(_) => Ok(self.handle_panel_overlay_key(key, terminal)),
            ComposerMode::Side => Ok(self.handle_side_chat_key(key, terminal)),
            ComposerMode::TranscriptSearch(_) => Ok(self.handle_transcript_search_key(key)),
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

        if self.handle_questionnaire_transcript_key(key, terminal)? {
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

        if self.handle_favorite_cycle_key(key, agent).await? {
            return Ok(());
        }

        match self.handle_command_palette_key(key) {
            CommandPaletteKeyOutcome::Ignored => {}
            CommandPaletteKeyOutcome::Handled => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                return Ok(());
            }
            CommandPaletteKeyOutcome::Submit => {
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                self.submit(terminal, agent).await?;
                return Ok(());
            }
        }

        if self.handle_file_palette_key(key)? {
            return Ok(());
        }

        if self.handle_vim_key(key) {
            return Ok(());
        }

        if self
            .handle_configurable_composer_key(key, terminal, agent)
            .await?
        {
            return Ok(());
        }

        if let Some(edit) = ComposerEditKey::from_key(key) {
            self.apply_input_edit_key(edit);
            self.ctrl_c_streak = 0;
            self.clamp_command_selection();
            self.clamp_file_selection();
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
                let cancelled_compact = self.cancel_compact(agent).await?;
                if cancelled_compact || (!self.cancel_inline_shells() && !self.exit_shell_mode()) {
                    self.take_back_held_turn();
                }
                self.ctrl_c_streak = 0;
            }
            (_, KeyCode::Enter) => {
                // Reset first: submit runs the whole turn, and a Ctrl-C streak
                // armed during it (leaving attach or peek) must survive.
                self.ctrl_c_streak = 0;
                self.submit(terminal, agent).await?;
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

    pub(super) fn submit<'a>(
        &'a mut self,
        terminal: &'a mut DefaultTerminal,
        agent: &'a mut InteractiveRuntime,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + 'a {
        // Submission can poll an entire turn or command. Its construction
        // frame must not stay on the Windows main stack for that whole call.
        Box::pin(self.submit_inner(terminal, agent))
    }

    async fn submit_inner(
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
            if self.info.session.daemon.is_none() {
                self.ensure_session(agent)?;
            }
            self.start_inline_shell(mode, command)?;
            return Ok(());
        }

        match self.parse_input_command() {
            Ok(Some(invocation)) => {
                // A command the daemon runs as a prompt needs credentials
                // as any prompt does: held for `/login` the same way.
                if self.info.session.daemon.is_some()
                    && super::daemon_gate::command_route(invocation.id)
                        == super::daemon_gate::CommandRoute::Prompt
                    && self.daemon_offers_built_in(invocation.id)
                    && self.daemon_prompt_needs_login()
                {
                    return self.offer_login_instead_of_turn(turn);
                }
                let submission = self.take_command_submission(invocation, turn);
                self.execute_command(submission, terminal, agent).await?;
                return Ok(());
            }
            Ok(None) => {}
            Err(commands::CommandParseError::Unknown(name)) => {
                if self.info.session.daemon.is_some() {
                    // `/skill:<name>` goes as the daemon's `/$<name>`, which
                    // loads the skill or says why the session cannot.
                    if let Some(skill) = name.strip_prefix("skill:") {
                        if !self.daemon_may_load_skill(skill) {
                            self.clear_submitted_input();
                            self.report_unknown_command(&name);
                            return Ok(());
                        }
                        turn = TurnPrompt::command(turn.model, turn.display)
                            .with_skill(skill.to_owned());
                    } else if !self.daemon_prompt_command_available(&name) {
                        self.clear_submitted_input();
                        self.daemon_command_unavailable(&name);
                        return Ok(());
                    }
                    if self.daemon_prompt_needs_login() {
                        // The held text is what Enter sends after login, so
                        // a template is held expanded, as in process. The
                        // daemon expands one it is sent as typed.
                        let trailing = slash_command_args(&turn.model).trim().to_string();
                        if let Some(prompt) = self.resolve_prompt_template(&name, &trailing) {
                            turn = TurnPrompt::standard(prompt.clone(), prompt);
                        }
                        return self.offer_login_instead_of_turn(turn);
                    }
                    let media = self
                        .input_ui
                        .take_ready_media()
                        .expect("pending attachments block submission");
                    let paste_segments = self.input_ui.paste_segments().to_vec();
                    self.clear_submitted_input();
                    return self
                        .submit_daemon_turn(turn, media, paste_segments, terminal)
                        .await;
                }
                let trailing_prompt = slash_command_args(&turn.model).trim().to_string();
                self.clear_submitted_input();
                if let Some(resolved) = crate::app::workflow_cli::discover::resolve_start_command(
                    &self.info.runtime.cwd,
                    &name,
                    &trailing_prompt,
                ) {
                    return self.start_workflow_command(&name, resolved, agent).await;
                }
                if let Some(prompt) = self.resolve_prompt_template(&name, &trailing_prompt) {
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
                        agent.has_tool(crate::skills::mention::SKILL_TOOL),
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

        // A daemon session runs the turn: it loads the skills the prompt
        // mentions and owns tools and compaction. A prompt it could only
        // refuse for want of credentials offers `/login` first.
        if self.info.session.daemon.is_some() {
            if self.daemon_prompt_needs_login() {
                return self.offer_login_instead_of_turn(turn);
            }
            let media = self
                .input_ui
                .take_ready_media()
                .expect("pending attachments block submission");
            let paste_segments = self.input_ui.paste_segments().to_vec();
            self.clear_submitted_input();
            return self
                .submit_daemon_turn(turn, media, paste_segments, terminal)
                .await;
        }

        self.load_mentioned_skills(&mut turn, agent);

        if !self.setup_state().signed_in {
            return self.offer_login_instead_of_turn(turn);
        }

        let media = self
            .input_ui
            .take_ready_media()
            .expect("pending attachments block submission");
        let paste_segments = self.input_ui.paste_segments().to_vec();
        self.clear_submitted_input();

        // MCP connect and OpenAPI loading run after the first frame, so a
        // prompt can land before their tools exist. Hold the turn and start it
        // once loading settles; the alternative is a person watching a status
        // line and pressing enter again for up to the full connect budget.
        if agent.tool_sources_pending() {
            // Queue rather than replace: someone who submits twice while the
            // tools are still loading must not lose the first prompt.
            self.hold_turn(turn, media, paste_segments);
            return Ok(());
        }

        self.run_turn_sequence_held(turn, media, paste_segments, terminal, agent)
            .await
    }

    /// Submit a skill or command-backed model turn through the same tool-loading hold,
    /// compaction and follow-up path as a regular composer prompt, or
    /// to the daemon session, as a composer prompt goes.
    pub(super) async fn submit_interactive_turn(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.info.session.daemon.is_some() {
            return self
                .submit_daemon_turn(turn, media, paste_segments, terminal)
                .await;
        }
        if agent.tool_sources_pending() {
            self.hold_turn(turn, media, paste_segments);
            return Ok(());
        }
        self.run_turn_sequence_held(turn, media, paste_segments, terminal, agent)
            .await
    }

    pub(super) fn hold_turn(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
    ) {
        self.hold(HeldTurn {
            turn,
            media,
            paste_segments,
            origin: HeldOrigin::Composer,
        });
    }

    /// Holds `held` until the session's tool sources loaded.
    pub(super) fn hold(&mut self, held: HeldTurn) {
        self.held_turns.push_back(held);
        self.turn
            .set_activity_phase(ActivityPhase::LoadingToolSources);
        self.turn.start_loading_if_needed();
        self.set_tool_sources_loading_status();
    }

    /// Hand the most recently held turn back to the composer, so `esc` unwinds
    /// prompts newest first instead of leaving them to run. Does nothing unless
    /// the composer is empty, so it cannot overwrite something the person has
    /// started typing since. A workflow the hub started is cancelled instead,
    /// whatever the composer holds: the composer never held its command.
    fn take_back_held_turn(&mut self) {
        let composer_in_use =
            !self.input_ui.text().is_empty() || !self.input_ui.attachments().is_empty();
        match self.held_turns.back().map(|held| &held.origin) {
            Some(HeldOrigin::Composer | HeldOrigin::Confirmed(_) | HeldOrigin::Command)
                if composer_in_use =>
            {
                return
            }
            Some(
                HeldOrigin::Composer
                | HeldOrigin::Confirmed(_)
                | HeldOrigin::Command
                | HeldOrigin::WorkflowHub,
            )
            | None => {}
        }
        let Some(HeldTurn {
            turn,
            media,
            paste_segments,
            origin,
        }) = self.held_turns.pop_back()
        else {
            return;
        };
        match origin {
            HeldOrigin::Composer | HeldOrigin::Confirmed(_) | HeldOrigin::Command => self
                .restore_pending_prompt(QueuedPrompt {
                    prompt: turn.model,
                    display_prompt: turn.display,
                    paste_segments,
                    media,
                }),
            HeldOrigin::WorkflowHub => {}
        }
        if !self.held_turns.is_empty() {
            // Older holds are still waiting. Leave their status alone: writing
            // any status here would hand `status_source` back to `Other`, and
            // `poll_startup_hydrates` would never retire the indicator.
            return;
        }
        self.clear_tool_sources_loading_activity();
        self.set_status_quiet("");
    }

    pub(super) fn clear_tool_sources_loading_activity(&mut self) {
        if matches!(
            self.turn.activity_phase(),
            ActivityPhase::LoadingToolSources
        ) {
            self.turn.set_activity_phase(ActivityPhase::default());
            if !self.turn.is_busy() {
                self.turn.stop_loading();
            }
        }
    }

    pub(super) fn restore_tool_sources_hold_activity_if_needed(
        &mut self,
        tool_sources_pending: bool,
    ) {
        if self.held_turns.is_empty() || !tool_sources_pending || self.turn.is_busy() {
            return;
        }
        self.turn
            .set_activity_phase(ActivityPhase::LoadingToolSources);
        self.turn.start_loading_if_needed();
    }

    fn first_held_turn_is_releasable(&self, tool_sources_pending: bool, compacting: bool) -> bool {
        !self.held_turns.is_empty() && !tool_sources_pending && !compacting
    }

    /// Start the next held turn whose wait is over. One per call, so several
    /// held turns run in submission order. Reports whether anything changed so
    /// the caller can redraw.
    pub(super) async fn release_pending_held_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        // A daemon session may be running a turn another client or a
        // background result started; the held one waits for it to end.
        if self.input_ui.composer().blocks_held_turn_start()
            || (self.info.session.daemon.is_some() && self.is_ui_busy())
        {
            return Ok(false);
        }
        if !self
            .first_held_turn_is_releasable(self.tool_sources_pending(agent), agent.is_compacting())
        {
            return Ok(false);
        }
        let Some(HeldTurn {
            turn,
            media,
            paste_segments,
            origin,
        }) = self.held_turns.pop_front()
        else {
            return Ok(false);
        };
        self.set_status_quiet("");
        if self.held_turns.is_empty() {
            self.clear_tool_sources_loading_activity();
        }
        match origin {
            HeldOrigin::WorkflowHub => self.send_daemon_workflow(turn, terminal).await?,
            HeldOrigin::Command => {
                self.send_daemon_command(turn, media, paste_segments, terminal)
                    .await?;
            }
            HeldOrigin::Confirmed(model) => {
                let submission =
                    SendSubmission::turn(turn, media, paste_segments).approve_for(model);
                self.submit_daemon_submission(submission, terminal).await?;
            }
            HeldOrigin::Composer if self.info.session.daemon.is_some() => {
                self.submit_daemon_turn(turn, media, paste_segments, terminal)
                    .await?;
            }
            HeldOrigin::Composer => {
                self.run_turn_sequence_held(turn, media, paste_segments, terminal, agent)
                    .await?;
            }
        }
        Ok(true)
    }

    /// Start the next queued follow-up once armed and the composer is free.
    /// Compact arms this with auto-compact off; model-switch handoff arms it on.
    pub(super) async fn start_next_follow_up(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if self.start_follow_ups.is_none() {
            return Ok(false);
        }
        // Queued prompts go to a daemon session; a send compaction held
        // still runs in process.
        if self.info.session.daemon.is_some()
            && matches!(
                self.start_follow_ups,
                Some(super::compact_work::ReadyFollowUp::Queued { .. })
            )
        {
            return self.start_next_daemon_follow_up(terminal).await;
        }
        if agent.is_compacting() || self.tool_sources_pending(agent) {
            return Ok(false);
        }
        if self.input_ui.composer().blocks_held_turn_start() {
            return Ok(false);
        }
        if self.is_ui_busy() {
            if !super::compact_work::keep_armed_follow_up_while_busy(
                self.start_follow_ups.as_ref().expect("checked above"),
            ) {
                self.start_follow_ups = None;
            }
            return Ok(false);
        }
        let ready = self.start_follow_ups.take().expect("checked above");
        match ready {
            super::compact_work::ReadyFollowUp::Send(submission) => {
                self.start_approved_submission(*submission, terminal, agent)
                    .await?;
            }
            super::compact_work::ReadyFollowUp::Queued { allow_auto_compact } => {
                let Some(prompt) = self.pending.pop_follow_up() else {
                    return Ok(false);
                };
                self.pending_input_changed();
                self.select_pending_recall_target();
                let submission = self.queued_turn_submission(prompt, agent);
                let Some(submission) = self.gate_send(submission, agent) else {
                    return Ok(true);
                };
                let (payload, authorization, _allow_auto_compact) = submission.into_authorized();
                let SendPayload::Turn {
                    turn,
                    media,
                    paste_segments,
                    ..
                } = payload
                else {
                    unreachable!("queued prompts are turn submissions");
                };
                if allow_auto_compact {
                    self.run_turn_sequence_after_send_gate(
                        turn,
                        media,
                        paste_segments,
                        authorization,
                        terminal,
                        agent,
                    )
                    .await?;
                } else {
                    self.run_turn_sequence_without_auto_compact(
                        turn,
                        media,
                        authorization,
                        terminal,
                        agent,
                    )
                    .await?;
                }
            }
        }
        Ok(true)
    }

    pub(super) async fn run_turn_sequence_held(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(submission) =
            self.gate_send(SendSubmission::turn(turn, media, paste_segments), agent)
        else {
            return Ok(());
        };
        let (payload, authorization, _allow_auto_compact) = submission.into_authorized();
        let SendPayload::Turn {
            turn,
            media,
            paste_segments,
            ..
        } = payload
        else {
            unreachable!("turn sequence received a turn submission");
        };
        self.run_turn_sequence_after_send_gate(
            turn,
            media,
            paste_segments,
            authorization,
            terminal,
            agent,
        )
        .await
    }

    pub(super) async fn run_turn_sequence_after_send_gate(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        authorization: SendAuthorization,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if agent.is_compacting() {
            return self.queue_prompt(turn.model, turn.display, paste_segments, media);
        }
        self.apply_pending_compaction_config(agent)?;
        if agent.should_auto_compact() {
            self.start_compact(
                agent,
                super::compact_work::CompactFollowUp::None,
                /*instructions*/ None,
            )?;
            return self.queue_prompt(turn.model, turn.display, paste_segments, media);
        }
        self.run_turn_sequence_without_auto_compact(turn, media, authorization, terminal, agent)
            .await
    }

    pub(super) async fn run_turn_sequence_without_auto_compact(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        authorization: SendAuthorization,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.run_prompt_turn(turn, media, authorization, terminal, agent)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "idle_input_tests.rs"]
mod tests;
