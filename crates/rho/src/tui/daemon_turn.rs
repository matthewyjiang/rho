//! Runs a daemon session's turns from the TUI (#1667): sends composer
//! prompts, follows the turn while it runs, and cancels or steers it.
//!
//! The daemon owns the turn. A prompt only asks it to start one: the user
//! row is the daemon's `user_message` echo, and the busy chrome follows the
//! job's state ([`super::daemon_transcript`]). While the job runs, or waits
//! for input, the app loop hands the terminal to [`App::follow_daemon_turn`],
//! which routes keys through `during_turn`, so Esc cancels, Enter queues a
//! steer, which this sends to the daemon, and an open input request's prompt
//! takes its keys (`daemon_answers`). Quitting leaves the turn running in the
//! daemon, which keeps the session.

use std::time::Instant;

use rho_protocol::{PromptAuthor, TurnErrorKind, TurnOutcome};

use super::{
    daemon_prompt,
    during_turn::RunningTerminalError,
    idle_input::{HeldOrigin, HeldTurn},
    pending_input::{AcceptedSteering, PendingInputAction},
    send_confirm::SendSubmission,
    App, ChatMedia, DefaultTerminal, Entry, FrameScheduler, PasteSegment, QueuedPrompt,
    StreamControl, TurnPrompt,
};

impl App {
    /// Whether the followed daemon session runs a turn.
    pub(super) fn daemon_turn_runs(&self) -> bool {
        self.turn.is_provider_turn()
    }

    /// Sends `turn` with `media` to the daemon session as
    /// [`Self::send_daemon_turn`] does, once the person confirmed it where
    /// the session's model cannot be sent some of the conversation
    /// (`send_confirm::daemon`): every prompt that starts a model request
    /// goes through here. One held while the session's MCP servers connect
    /// is confirmed when it is released.
    ///
    /// The daemon starts a turn only once those servers connected, and the
    /// TUI would wait for it meanwhile, so every path to
    /// [`Self::send_daemon_turn`] holds the turn here until they have.
    pub(super) async fn submit_daemon_turn(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.daemon_tool_sources_loading() {
            self.hold_turn(turn, media, paste_segments);
            return Ok(());
        }
        let submission = SendSubmission::turn(turn, media, paste_segments);
        self.submit_daemon_submission(submission, terminal).await
    }

    /// Sends a command that is no send to confirm, as `/compact`: held like
    /// a prompt while the session's MCP servers connect, and released the
    /// same way.
    pub(super) async fn send_daemon_command(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.daemon_tool_sources_loading() {
            self.hold(HeldTurn {
                turn,
                media,
                paste_segments,
                origin: HeldOrigin::Command,
            });
            return Ok(());
        }
        self.send_daemon_turn(turn, media, paste_segments, terminal)
            .await
    }

    pub(super) fn daemon_tool_sources_loading(&self) -> bool {
        self.info.session.daemon.tool_sources_loading()
    }

    /// Asks the daemon to run `turn` with `media`, now and unconfirmed: the
    /// caller already held it while the session's MCP servers connected and
    /// passed the send gate where one applies. On a refusal the transcript
    /// says why and the composer gets the prompt back, so the caller clears
    /// the composer before calling. A turn that ended before
    /// it saved its prompt is not a refusal: the daemon's own rows say how
    /// it ended, so the prompt comes back without another. A command the
    /// daemon refused, as `/init` in plan mode, does not come back either:
    /// it goes to the input history as a sent one does.
    pub(super) async fn send_daemon_turn(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if let Err(error) = self.request_daemon_turn(&turn, &media).await {
            // Why the prompt found no attachment, such as a failed reattach
            // after a daemon restart, shows first.
            self.show_taken_daemon_views(terminal).await?;
            let ended = error
                .downcast_ref::<rho_protocol::ClientError>()
                .and_then(rho_protocol::ClientError::turn_ended);
            match ended {
                // The daemon refused the command itself, and its row says
                // why: sent, as far as the composer goes.
                Some(TurnOutcome::Failed { error })
                    if error.kind == TurnErrorKind::Configuration =>
                {
                    if !turn.history.is_empty() {
                        self.push_input_history(&turn.history);
                    }
                    self.reset_input_history_navigation();
                    return Ok(());
                }
                // The daemon's rows say how the turn ended.
                Some(
                    TurnOutcome::Failed { .. }
                    | TurnOutcome::Cancelled
                    | TurnOutcome::TimedOut { .. },
                ) => {}
                // Not sent, or ended in a way this rho cannot show.
                Some(_) | None => self.insert_entry(&Entry::Error(format!(
                    "could not send the prompt: {error:#}"
                ))),
            }
            self.restore_pending_prompt(QueuedPrompt {
                prompt: turn.model,
                display_prompt: turn.display,
                paste_segments,
                media,
            });
            return Ok(());
        }
        if !turn.history.is_empty() {
            self.push_input_history(&turn.history);
        }
        self.reset_input_history_navigation();
        self.show_taken_daemon_views(terminal).await
    }

    /// Sends the next queued follow-up once a turn completed armed
    /// `start_follow_ups` and the composer is free. Reports whether anything
    /// changed.
    pub(super) async fn start_next_follow_up(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        // Another client's turn may run meanwhile; its end arms this again.
        if !self.start_follow_ups
            || self.input_ui.composer().blocks_held_turn_start()
            || self.is_ui_busy()
        {
            return Ok(false);
        }
        self.start_follow_ups = false;
        let Some((prompt, consent)) = self.pending.pop_follow_up_with_consent() else {
            return Ok(false);
        };
        self.pending_input_changed();
        self.select_pending_recall_target();
        let turn = TurnPrompt::standard(prompt.prompt, prompt.display_prompt);
        let submission = SendSubmission::turn(turn, prompt.media, prompt.paste_segments)
            .with_compact_consent(consent);
        // A confirmation takes the prompt, which it sends or returns to the
        // composer; the rest of the queue stays parked.
        let Some(submission) = self.gate_daemon_send(submission).await else {
            return Ok(true);
        };
        let (turn, prompt) = submission.into_daemon_turn();
        if let Err(error) = self.request_daemon_turn(&turn, &prompt.media).await {
            self.show_taken_daemon_views(terminal).await?;
            self.pending.push_follow_up_front(prompt);
            self.pending_input_changed();
            self.insert_entry(&Entry::Error(format!(
                "could not send the queued message: {error:#}"
            )));
            return Ok(true);
        }
        self.show_taken_daemon_views(terminal).await?;
        Ok(true)
    }

    /// Starts a daemon turn of `turn` with `media`. Once this returns, what
    /// the daemon sent before it answered, such as the turn's start, waits
    /// for [`Self::show_taken_daemon_views`], as does, on an error, what
    /// ended the attachment, such as a failed reattach.
    pub(super) async fn request_daemon_turn(
        &mut self,
        turn: &TurnPrompt,
        media: &[ChatMedia],
    ) -> anyhow::Result<()> {
        self.request_daemon_prompt(turn, media, PromptAuthor::Person)
            .await
    }

    /// [`Self::request_daemon_turn`] for a prompt `author` sends: the
    /// daemon keeps no user message for one the TUI sends on the person's
    /// behalf, as the workflow hub's start.
    pub(super) async fn request_daemon_prompt(
        &mut self,
        turn: &TurnPrompt,
        media: &[ChatMedia],
        author: PromptAuthor,
    ) -> anyhow::Result<()> {
        // The person moved on from a report still on its way.
        self.awaited_daemon_report = None;
        self.info
            .session
            .daemon
            .prompt(daemon_prompt::prompt_content(turn, media), author)
            .await
    }

    /// Shows what the daemon sent up to now without waiting, so a turn the
    /// TUI just started shows busy before the next key is read.
    pub(super) async fn show_taken_daemon_views(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let views = self.info.session.daemon.take_views();
        self.show_daemon_views(views, terminal).await
    }

    /// Follows the daemon session's running turn until it rests, or the
    /// person quits: Esc cancels it, and Enter steers it.
    pub(super) async fn follow_daemon_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let mut frame_scheduler = FrameScheduler::new(Instant::now());
        self.draw_running_frame(terminal, &mut frame_scheduler)?;
        while self.daemon_turn_runs() && !self.should_quit {
            let mut needs_redraw = self.update_activity_panels()?;
            needs_redraw |= self.reconcile_overlays().await?;
            needs_redraw |= self.poll_sessions_task()?;
            needs_redraw |= self.finish_completed_inline_shells().await?;
            if needs_redraw {
                self.draw_running_frame(terminal, &mut frame_scheduler)?;
            }
            let frame_deadline =
                self.next_running_frame_deadline(frame_scheduler.deferred_deadline());
            tokio::select! {
                biased;
                event = self.terminal_session.as_mut().expect("terminal session initialized").next_event() => {
                    let control = self
                        .handle_running_terminal_events(event?, terminal)
                        .await;
                    // Before any cancel: Esc at an approval denies it, then
                    // cancels the turn, in that order.
                    let cancelling = matches!(control, Ok(StreamControl::Interrupt));
                    self.answer_daemon_input(terminal, cancelling).await?;
                    match control {
                        // Quitting leaves the turn to the daemon, unless
                        // nothing could follow it again: a session with no
                        // file cannot be resumed, so its turn ends here.
                        Ok(StreamControl::Interrupt)
                            if self.should_quit && !self.info.session.no_save => {}
                        Ok(StreamControl::Interrupt) => self.cancel_daemon_turn().await,
                        Ok(
                            StreamControl::Continue
                            | StreamControl::Resize
                            | StreamControl::ApprovalResolved,
                        ) => {}
                        Err(RunningTerminalError::Recoverable(error)) => {
                            self.insert_entry(&Entry::Error(format!(
                                "could not handle input: {error}"
                            )));
                        }
                        Err(RunningTerminalError::Terminal(error)) => return Err(error),
                    }
                    self.send_daemon_pending_input().await;
                    self.pending_input_changed();
                    self.draw_running_frame(terminal, &mut frame_scheduler)?;
                }
                views = self.info.session.daemon.next_views() => {
                    self.show_daemon_views(views, terminal).await?;
                    if frame_scheduler.request_background_frame(Instant::now()) {
                        self.draw_running_frame(terminal, &mut frame_scheduler)?;
                    }
                }
                _ = tokio::time::sleep_until(frame_deadline) => {
                    self.tick_questionnaire_timeout();
                    self.drain_stream_tick(terminal)?;
                    self.flush_due_paste_burst();
                    self.draw_running_frame(terminal, &mut frame_scheduler)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn draw_running_frame(
        &mut self,
        terminal: &mut DefaultTerminal,
        frame_scheduler: &mut FrameScheduler,
    ) -> anyhow::Result<()> {
        terminal.draw(|frame| self.draw(frame))?;
        frame_scheduler.rendered(Instant::now());
        Ok(())
    }

    /// Asks the daemon to stop its turn; the turn's end shows that it did.
    pub(super) async fn cancel_daemon_turn(&mut self) {
        if let Err(error) = self.info.session.daemon.cancel().await {
            self.insert_entry(&Entry::Error(format!(
                "could not cancel the turn: {error:#}"
            )));
        }
    }

    /// Sends what the pending panel asked for since: a steer to withdraw,
    /// then the steers queued meanwhile. A steer the daemon refuses, and
    /// every one queued after it, becomes a follow-up.
    async fn send_daemon_pending_input(&mut self) {
        if let Some(action) = self.pending.take_input_action() {
            let id = match &action {
                PendingInputAction::EditAccepted { id, .. }
                | PendingInputAction::DiscardAccepted { id } => id.clone(),
            };
            let retraction = match self.info.session.daemon.unsteer(&id).await {
                Ok(outcome) => steering_retraction(outcome),
                Err(error) => Err(format!("{error:#}")),
            };
            self.finish_steering_retraction(action, retraction);
        }
        while let Some(prompt) = self.pending.steering_prompts_mut().pop_front() {
            match self.info.session.daemon.steer(prompt.prompt.clone()).await {
                Ok(id) => {
                    self.pending
                        .accepted_steering_mut()
                        .push_back(AcceptedSteering {
                            id,
                            prompt,
                            delivered: false,
                        });
                    self.select_pending_recall_target();
                }
                Err(error) => {
                    let unsent: Vec<_> = std::iter::once(prompt)
                        .chain(self.pending.steering_prompts_mut().drain(..))
                        .collect();
                    for prompt in unsent.into_iter().rev() {
                        self.pending.push_follow_up_front(prompt);
                    }
                    self.select_pending_recall_target();
                    self.notify_status(format!("steer queued as follow-up: {error:#}"));
                    return;
                }
            }
        }
    }
}

/// What withdrawing a steer did, as the pending panel shows it.
fn steering_retraction(
    outcome: rho_protocol::UnsteerOutcome,
) -> Result<rho_sdk::SteeringRetraction, String> {
    match outcome {
        rho_protocol::UnsteerOutcome::Withdrawn => Ok(rho_sdk::SteeringRetraction::Retracted),
        rho_protocol::UnsteerOutcome::Delivered => Ok(rho_sdk::SteeringRetraction::Delivered),
        rho_protocol::UnsteerOutcome::Applied => Ok(rho_sdk::SteeringRetraction::AlreadyApplied),
        rho_protocol::UnsteerOutcome::NotFound => Ok(rho_sdk::SteeringRetraction::NotFound),
        // `Unknown`, or an outcome a newer daemon sends.
        _ => Err("the rho daemon answered with an outcome this client does not know".into()),
    }
}
