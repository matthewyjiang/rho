//! Shows a daemon session's [`DaemonView`]s through the TUI's own transcript
//! paths (#1667): history entries and the live tool batch, fed the same
//! view-model events an in-process turn feeds them, and the busy chrome an
//! in-process turn shows, which here follows the job's state. Input
//! requests open and close their prompts through `daemon_answers`.

use std::time::Instant;

use super::{
    activity::ActivityPhase,
    compact_work::ReadyFollowUp,
    daemon_events::{DaemonView, Rest, TurnState},
    event_adapter::ViewModelEvent,
    program_status::ProgramStatus,
    settings_screen::SettingsUpdate,
    App, DefaultTerminal, Entry, HerdrState, StreamKind,
};

impl App {
    /// Applies what the followed daemon session changed.
    pub(super) async fn show_daemon_views(
        &mut self,
        views: Vec<DaemonView>,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        for view in views {
            match view {
                DaemonView::Clear => self.clear_daemon_transcript(),
                DaemonView::InputsReset => self.drop_daemon_inputs(),
                DaemonView::User(text) => self.insert_entry(&Entry::User(text)),
                DaemonView::Text { kind, text, opens } => {
                    if self.turn.is_provider_turn() {
                        self.turn.set_activity_phase(match kind {
                            StreamKind::Assistant => ActivityPhase::Responding,
                            StreamKind::Reasoning => ActivityPhase::Thinking,
                        });
                    }
                    if opens {
                        self.open_transcript_row(kind.entry(text));
                    } else {
                        self.push_transcript_entry(kind.entry(text));
                    }
                }
                DaemonView::ToolRunning { call, card } => {
                    self.handle_agent_event(
                        ViewModelEvent::ToolStarted {
                            call_id: call,
                            card,
                        },
                        terminal,
                    )?;
                }
                DaemonView::ToolFinished { call, presentation } => {
                    self.handle_agent_event(
                        ViewModelEvent::ToolFinished {
                            call_id: call,
                            presentation,
                            // Images a call returned stay in its wire
                            // content, which cards do not show yet.
                            image_asset: None,
                        },
                        terminal,
                    )?;
                }
                DaemonView::Plan(list) => {
                    self.set_todo_list(list);
                }
                DaemonView::Context {
                    used_tokens,
                    window_tokens,
                } => {
                    self.handle_agent_event(
                        ViewModelEvent::ContextUsage(
                            rho_sdk::model::ContextUsage::provider_reported(
                                used_tokens,
                                window_tokens,
                            ),
                        ),
                        terminal,
                    )?;
                }
                DaemonView::Spend(cost_usd_micros) => {
                    self.usage.cumulative_usage = Some(rho_providers::model::ModelUsage {
                        cost_usd_micros,
                        ..Default::default()
                    });
                }
                DaemonView::Title(title) => self.daemon_title = Some(title),
                DaemonView::Options(options) => self.show_daemon_options(*options),
                DaemonView::McpInventory(result) => self.show_daemon_mcp(result),
                DaemonView::Report { report, result } => self.show_daemon_report(report, result),
                DaemonView::Login(update) => self.show_daemon_login(update, terminal).await?,
                DaemonView::Jobs(jobs) => self.show_daemon_jobs(jobs),
                DaemonView::SettingsLoaded(Ok(settings)) => {
                    self.show_daemon_settings(SettingsUpdate::Loaded(settings));
                }
                DaemonView::SettingsLoaded(Err(error)) => {
                    self.show_daemon_settings(SettingsUpdate::Unreadable(error));
                }
                DaemonView::SettingsChanged(changed) => {
                    self.show_daemon_settings(SettingsUpdate::Changed(changed));
                }
                DaemonView::ConfigureFinished => {
                    self.daemon_option_requests.pop_front();
                }
                DaemonView::Turn(state) => self.show_daemon_turn(state).await,
                DaemonView::SteerApplied(id) => {
                    // The user row that follows shows it.
                    let _ = self.take_accepted_steering(std::slice::from_ref(&id));
                }
                DaemonView::SteerDelivered(id) => self.record_delivered_steering(&id),
                DaemonView::InputOpened(request) => self.ask_daemon_input(*request).await,
                DaemonView::InputClosed { id, notice } => {
                    self.close_daemon_input(id, notice).await;
                }
                DaemonView::InputHeld(id) => self.daemon_input_held(id),
                DaemonView::Status(text) => self.set_status(text),
                DaemonView::Notice(text) => self.insert_entry(&Entry::Notice(text)),
                DaemonView::Error(text) => self.insert_entry(&Entry::Error(text)),
            }
        }
        Ok(())
    }

    /// Appends `entry` as a row of its own, even after a row of its kind
    /// that it would otherwise extend: a new message starts a new row.
    fn open_transcript_row(&mut self, entry: Entry) {
        let index = self.history.len();
        self.history.lines_mut().invalidate_from(index);
        self.history.push(entry);
        self.mark_markdown_images_dirty_from(index);
    }

    /// Shows the session's turn as the job's state says: the busy chrome,
    /// program status, and Herdr state an in-process turn shows while it
    /// runs, and how it ended once it rests.
    async fn show_daemon_turn(&mut self, state: TurnState) {
        match state {
            TurnState::Running => {
                self.begin_daemon_turn();
                if self.turn.activity_phase() == ActivityPhase::WaitingForInput {
                    self.turn.set_activity_phase(ActivityPhase::Starting);
                }
                self.report_working().await;
            }
            TurnState::WaitingForInput => {
                self.begin_daemon_turn();
                self.turn.set_activity_phase(ActivityPhase::WaitingForInput);
                self.report_herdr_state(HerdrState::Blocked, Some("waiting for input"))
                    .await;
            }
            TurnState::Resting(rest) => self.end_daemon_turn(rest).await,
        }
    }

    /// Empties the transcript for a reset replay. A turn that shows busy
    /// keeps its chrome and clock, but its rows are gone: the replay's
    /// running state places its start again, as a first attach does.
    fn clear_daemon_transcript(&mut self) {
        self.set_history_entries(Vec::new());
        self.turn.clear_tool_calls();
        self.set_todo_list(None);
        self.drop_daemon_inputs();
        self.turn.set_current_turn_start(None);
    }

    /// The busy chrome of a turn that started, unless it shows already. A
    /// turn already showing whose rows a reset replay rebuilt starts after
    /// them.
    fn begin_daemon_turn(&mut self) {
        if self.turn.is_provider_turn() {
            if self.turn.current_turn_start().is_none() {
                self.turn.set_current_turn_start(Some(self.history.len()));
            }
            return;
        }
        // After the turn's user row, which the daemon sent first.
        self.turn.set_current_turn_start(Some(self.history.len()));
        self.set_status("running");
        self.begin_provider_turn_ui();
        self.turn.set_activity_phase(ActivityPhase::Starting);
        self.turn.start_loading();
    }

    /// Ends the busy chrome as the turn rested `rest`, closing any input
    /// request's prompt still open. Cards still running
    /// stay in the transcript as interrupted, as a replay of a turn the
    /// daemon never closed leaves them; a session already at rest only
    /// tidies those.
    ///
    /// Pending work settles as an in-process turn's does: an interrupted
    /// turn hands it back to the composer, any other keeps steers it never
    /// applied as follow-ups, and a completed one arms the next follow-up,
    /// which the app loop sends once the composer is free.
    async fn end_daemon_turn(&mut self, rest: Rest) {
        // A turn withdraws its requests before it ends, so any prompt still
        // open is one the TUI can no longer follow.
        self.drop_daemon_inputs();
        for entry in self.turn.interrupted_tool_entries() {
            self.insert_entry(&Entry::Tool(entry));
        }
        self.turn.clear_tool_calls();
        self.turn.set_activity_phase(ActivityPhase::default());
        if !self.turn.is_provider_turn() {
            return;
        }
        // Read before `stop_loading` clears the turn clock.
        let worked = self.turn.elapsed_at(Instant::now());
        if matches!(rest, Rest::Interrupted) {
            self.restore_pending_work_to_input();
        } else {
            self.preserve_unapplied_steering_as_follow_ups();
        }
        self.clear_accepted_steering();
        let (status, settled) = match rest {
            Rest::Ready => {
                self.end_provider_turn_ui();
                if let Some(worked) = worked {
                    self.attach_turn_worked(worked);
                }
                let status = if self.pending.has_follow_ups() {
                    self.start_follow_ups = Some(ReadyFollowUp::Queued {
                        allow_auto_compact: false,
                    });
                    format!(
                        "running next queued message ({})",
                        self.pending.follow_up_len()
                    )
                } else {
                    "ready".to_owned()
                };
                (Some(status), ProgramStatus::Done)
            }
            Rest::Interrupted => {
                self.end_busy_ui();
                (Some("model interrupted".to_owned()), ProgramStatus::Idle)
            }
            Rest::Failed { error } => {
                self.end_busy_ui();
                (
                    Some("error".to_owned()),
                    ProgramStatus::Error { message: error },
                )
            }
            // The error row says why; the turn may run on in the daemon.
            Rest::Unfollowed => {
                self.end_busy_ui();
                (None, ProgramStatus::Idle)
            }
        };
        self.turn.stop_loading();
        self.turn.set_current_turn_start(None);
        if let Some(status) = status {
            self.set_status(status);
        }
        self.settled_program_status = settled;
        self.notifier.turn_finished();
        self.report_resting_herdr_state().await;
    }
}

#[cfg(test)]
#[path = "daemon_transcript_tests.rs"]
mod tests;
