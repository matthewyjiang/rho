//! Runs a daemon session's turns from the TUI (#1667): sends composer
//! prompts, follows the turn while it runs, and cancels or steers it.
//!
//! The daemon owns the turn. A prompt only asks it to start one: the user
//! row is the daemon's `user_message` echo, and the busy chrome follows the
//! job's state ([`super::daemon_transcript`]). While the job runs, the app
//! loop hands the terminal to [`App::follow_daemon_turn`], which routes keys
//! as an in-process turn does (`during_turn`), so Esc cancels and Enter
//! queues a steer, which this sends to the daemon. Quitting leaves the turn
//! running in the daemon, which keeps the session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::{
    daemon_prompt, daemon_session,
    during_turn::RunningTerminalError,
    pending_input::{AcceptedSteering, PendingInputAction},
    App, ChatMedia, DaemonSession, DefaultTerminal, Entry, FrameScheduler, InteractiveRuntime,
    PasteSegment, QueuedPrompt, StreamControl, TurnPrompt,
};

impl App {
    /// Whether the followed daemon session runs a turn.
    pub(super) fn daemon_turn_runs(&self) -> bool {
        self.info.session.daemon.is_some() && self.turn.is_provider_turn()
    }

    /// Asks the daemon to run `turn` with `media`. On a refusal the
    /// transcript says why and the composer gets the prompt back, so the
    /// caller clears the composer before calling.
    pub(super) async fn submit_daemon_turn(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<PasteSegment>,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if let Err(error) = self.request_daemon_turn(&turn, &media).await {
            self.insert_entry(&Entry::Error(format!(
                "could not send the prompt: {error:#}"
            )));
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

    /// Sends the next queued follow-up once a daemon turn completed and the
    /// composer is free; the in-process `start_next_follow_up` for a daemon
    /// session. Reports whether anything changed.
    pub(super) async fn start_next_daemon_follow_up(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        // Another client's turn may run meanwhile; its end arms this again.
        if self.input_ui.composer().blocks_held_turn_start() || self.is_ui_busy() {
            return Ok(false);
        }
        self.start_follow_ups = None;
        let Some(prompt) = self.pending.pop_follow_up() else {
            return Ok(false);
        };
        self.pending_input_changed();
        self.select_pending_recall_target();
        let turn = TurnPrompt::standard(prompt.prompt.clone(), prompt.display_prompt.clone());
        if let Err(error) = self.request_daemon_turn(&turn, &prompt.media).await {
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
    /// for [`Self::show_taken_daemon_views`].
    async fn request_daemon_turn(
        &self,
        turn: &TurnPrompt,
        media: &[ChatMedia],
    ) -> anyhow::Result<()> {
        let Some(session) = self.info.session.daemon.as_ref() else {
            anyhow::bail!("no daemon session is open");
        };
        session
            .prompt(daemon_prompt::prompt_content(turn, media))
            .await
    }

    /// Shows what the daemon sent up to now without waiting, so a turn the
    /// TUI just started shows busy before the next key is read.
    async fn show_taken_daemon_views(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let views = self
            .info
            .session
            .daemon
            .as_mut()
            .map(DaemonSession::take_views)
            .unwrap_or_default();
        self.show_daemon_views(views, terminal).await
    }

    /// Follows the daemon session's running turn until it rests, or the
    /// person quits, with the keys an in-process turn takes: Esc cancels it,
    /// and Enter steers it.
    pub(super) async fn follow_daemon_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let interrupt_requested = AtomicBool::new(false);
        let tool_call_active = AtomicBool::new(self.turn.tool_calls().is_running());
        let mut frame_scheduler = FrameScheduler::new(Instant::now());
        self.draw_running_frame(terminal, &mut frame_scheduler)?;
        while self.daemon_turn_runs() && !self.should_quit {
            let mut needs_redraw = self.update_activity_panels(agent)?;
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
                        .handle_running_terminal_events(
                            event?,
                            terminal,
                            &interrupt_requested,
                            &tool_call_active,
                        )
                        .await;
                    match control {
                        // Quitting leaves the turn to the daemon.
                        Ok(StreamControl::Interrupt) if self.should_quit => {}
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
                views = daemon_session::next_views(&mut self.info.session.daemon) => {
                    self.show_daemon_views(views, terminal).await?;
                    tool_call_active.store(self.turn.tool_calls().is_running(), Ordering::SeqCst);
                    if frame_scheduler.request_background_frame(Instant::now()) {
                        self.draw_running_frame(terminal, &mut frame_scheduler)?;
                    }
                }
                () = self.subagent_inbox.recv() => {
                    self.draw_running_frame(terminal, &mut frame_scheduler)?;
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

    /// Asks the daemon to stop its turn; the turn's end shows that it did.
    async fn cancel_daemon_turn(&mut self) {
        let Some(session) = self.info.session.daemon.as_ref() else {
            return;
        };
        if let Err(error) = session.cancel().await {
            self.insert_entry(&Entry::Error(format!(
                "could not cancel the turn: {error:#}"
            )));
        }
    }

    /// Sends what the pending panel asked for since: a steer to withdraw,
    /// then the steers queued meanwhile. A steer the daemon refuses, and
    /// every one queued after it, becomes a follow-up, as in process.
    async fn send_daemon_pending_input(&mut self) {
        if let Some(action) = self.pending.take_input_action() {
            let id = match &action {
                PendingInputAction::EditAccepted { id, .. }
                | PendingInputAction::DiscardAccepted { id } => id.clone(),
            };
            let Some(session) = self.info.session.daemon.as_ref() else {
                return;
            };
            let retraction = match session.unsteer(&id).await {
                Ok(outcome) => steering_retraction(outcome),
                Err(error) => Err(format!("{error:#}")),
            };
            self.finish_steering_retraction(action, retraction);
        }
        while let Some(prompt) = self.pending.steering_prompts_mut().pop_front() {
            let Some(session) = self.info.session.daemon.as_ref() else {
                self.pending.steering_prompts_mut().push_front(prompt);
                return;
            };
            match session.steer(prompt.prompt.clone()).await {
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
