//! Shows a daemon session's [`DaemonView`]s through the TUI's own transcript
//! paths (#1667): history entries and the live tool batch, fed the same
//! view-model events an in-process turn feeds them, and the busy chrome an
//! in-process turn shows, which here follows the job's state. Input
//! requests open and close their prompts through `daemon_answers`.

use std::time::Instant;

use super::{
    activity::ActivityPhase,
    compact_work::ReadyFollowUp,
    compaction_display::CompactionUiOutcome,
    daemon_events::{DaemonView, Rest, TurnState},
    event_adapter::{compact_finished_event, compact_started_event, ViewModelEvent},
    program_status::ProgramStatus,
    settings_screen::SettingsUpdate,
    App, DefaultTerminal, Entry, HerdrState, StreamKind, ToolEntry,
};
use rho_protocol::display::Presentation;

/// A turn of the followed daemon session that a `/compact` prompt started:
/// the turn is the compaction, and shows as the in-process TUI shows one.
#[derive(Default)]
pub(super) struct DaemonCompactTurn {
    /// How its compaction ended, as the status line says it; the turn rests
    /// on it.
    rest_status: Option<&'static str>,
    /// Whether its compaction compacted the conversation.
    compacted: bool,
}

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
                DaemonView::User { text, replayed } => {
                    self.finish_streams();
                    // The hub's own prompt has no row, as in process. A
                    // replayed row is history, whatever it reads.
                    let hub_launch = !replayed
                        && self
                            .daemon_hub_launch
                            .take_if(|sent| *sent == text)
                            .is_some();
                    if !hub_launch {
                        self.insert_entry(&Entry::User(text));
                    }
                }
                // A `/compact` turn's only text is its reply, which says
                // again what the card shows.
                DaemonView::Text { .. } if self.daemon_compact_turn.is_some() => {}
                // A running turn's text goes through the stream an
                // in-process turn's does, which holds, paces and previews
                // it as `output_streaming` says. History, and a turn this
                // TUI is not showing busy, is written as it stands.
                DaemonView::Text {
                    kind,
                    text,
                    opens,
                    replayed: false,
                } if self.turn.is_provider_turn() => {
                    if opens {
                        // A new message starts a new row, after any notice
                        // held for the last one's end, and from a clean
                        // stream, as each in-process step does.
                        self.finish_streams();
                        self.reset_streams();
                    }
                    self.handle_agent_event(
                        match kind {
                            StreamKind::Assistant => ViewModelEvent::OutputDelta(text),
                            StreamKind::Reasoning => ViewModelEvent::ReasoningDelta(text),
                        },
                        terminal,
                    )?;
                }
                DaemonView::Text {
                    kind,
                    text,
                    opens,
                    replayed: _,
                } => {
                    // Replayed into a running turn, as a reattach's tail
                    // or a reopen mid-reply is: what the stream still holds
                    // came before it, and the turn shows the phase the text
                    // put it in live.
                    let streaming = self.turn.is_provider_turn();
                    if streaming {
                        self.finish_streams();
                        self.turn.set_activity_phase(match kind {
                            StreamKind::Assistant => ActivityPhase::Responding,
                            StreamKind::Reasoning => ActivityPhase::Thinking,
                        });
                    }
                    match (opens, streaming) {
                        (true, true) => {
                            self.reset_streams();
                            self.open_transcript_row(kind.entry(text));
                        }
                        (true, false) => self.open_transcript_row(kind.entry(text)),
                        (false, true) => self.continue_stream_with_replayed(kind, &text),
                        (false, false) => self.push_transcript_entry(kind.entry(text)),
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
                DaemonView::CompactionStarted { command } => {
                    if command {
                        self.begin_daemon_compact_turn();
                    }
                    self.handle_agent_event(compact_started_event(), terminal)?;
                    self.set_status("compacting context");
                }
                DaemonView::CompactTurnOver => self.daemon_compact_turn = None,
                DaemonView::CompactionEnded(outcome) => {
                    let status = outcome.status_label();
                    let compacted = matches!(outcome, CompactionUiOutcome::Completed(_));
                    self.handle_agent_event(compact_finished_event(outcome), terminal)?;
                    self.set_status(status);
                    if let Some(turn) = &mut self.daemon_compact_turn {
                        turn.rest_status = Some(status);
                        turn.compacted = compacted;
                    }
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
                    // The daemon's figure includes the response under way,
                    // so the local estimate of its output starts over.
                    self.usage.live_stream.clear();
                    self.usage.cumulative_usage = Some(rho_providers::model::ModelUsage {
                        cost_usd_micros,
                        ..Default::default()
                    });
                }
                DaemonView::Title(title) => self.daemon_title = Some(title),
                DaemonView::Options(options) => self.show_daemon_options(*options),
                DaemonView::McpInventory(result) => self.show_daemon_mcp(result),
                DaemonView::ToolSourcesLoaded => self.show_daemon_tool_sources_loaded(),
                DaemonView::Report { report, result } => self.show_daemon_report(report, result),
                DaemonView::Login(update) => self.show_daemon_login(update, terminal).await?,
                DaemonView::Jobs(jobs) => self.show_daemon_jobs(jobs),
                DaemonView::Processes { asked, result } => {
                    self.show_daemon_processes(asked, result);
                }
                DaemonView::SettingsLoaded(Ok(settings)) => {
                    self.show_daemon_settings(SettingsUpdate::Loaded(settings));
                }
                DaemonView::SettingsLoaded(Err(error)) => {
                    self.show_daemon_settings(SettingsUpdate::Unreadable(error));
                }
                DaemonView::SettingsChanged(changed) => {
                    self.show_daemon_settings(SettingsUpdate::Changed(changed));
                }
                DaemonView::ConfigureFinished => self.finish_daemon_option_request(),
                DaemonView::Turn(state) => self.show_daemon_turn(state).await,
                DaemonView::SteerApplied(id) => {
                    // The user row that follows shows it.
                    let _ = self.take_accepted_steering(std::slice::from_ref(&id));
                }
                DaemonView::SteerDelivered(id) => self.record_delivered_steering(&id),
                DaemonView::InputOpened(request) => {
                    // The reply so far shows above what it asks.
                    self.finish_streams();
                    self.ask_daemon_input(*request).await;
                }
                DaemonView::InputClosed { id, notice } => {
                    self.close_daemon_input(id, notice).await;
                }
                DaemonView::InputHeld(id) => self.daemon_input_held(id),
                DaemonView::Retrying(hint) if self.turn.is_provider_turn() => {
                    self.turn
                        .set_activity_phase(ActivityPhase::RetryingProvider);
                    if let Some(hint) = hint {
                        self.set_status(hint.status_label());
                        self.turn.set_provider_retry(hint);
                    }
                }
                // No turn shows busy to say it retries.
                DaemonView::Retrying(_) => {}
                DaemonView::Status(text) => self.set_status(text),
                // Held until the message under way ends, as in process: a
                // notice such as a new title does not break into a reply.
                DaemonView::Notice(text) => self.insert_entry(&Entry::Notice(text)),
                DaemonView::Card(card) => {
                    // A result reaches the model between its messages, so
                    // what the stream still holds was said before it.
                    self.finish_streams();
                    self.daemon_result_delivered(&card);
                    self.insert_entry(&Entry::Tool(ToolEntry::new(
                        Presentation::Notification(card),
                        /*expanded*/ false,
                        /*image*/ None,
                        /*started_at*/ None,
                    )));
                }
                DaemonView::Error(text) => {
                    self.finish_streams();
                    self.insert_entry(&Entry::Error(text));
                }
            }
        }
        Ok(())
    }

    /// Writes `text`, replayed, as more of the message the running turn's
    /// stream of `kind` shows. It goes through that stream, whole and at
    /// once, so the stream's fence, wrap, and emitted text account for it
    /// when live text continues the message.
    fn continue_stream_with_replayed(&mut self, kind: StreamKind, text: &str) {
        self.streams.stream_mut(kind).push_delta(text);
        if let Some(fragment) = self.streams.stream_mut(kind).finish() {
            self.insert_stream_fragment(fragment, kind);
        }
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

    /// Starts showing the turn under way as the in-process TUI shows
    /// `/compact`, which is no turn there: the card alone, without the
    /// command's row, which the daemon sent just before the compaction
    /// started.
    fn begin_daemon_compact_turn(&mut self) {
        self.daemon_compact_turn = Some(DaemonCompactTurn::default());
        if matches!(self.history.last(), Some(Entry::User(_))) {
            let index = self.history.len() - 1;
            self.history.entries_mut().pop();
            self.history.invalidate_from(index);
            if let Some(start) = self.turn.current_turn_start() {
                self.turn.set_current_turn_start(Some(start.min(index)));
            }
        }
    }

    /// Empties the transcript for a reset replay. A turn that shows busy
    /// keeps its chrome and clock, but its rows are gone: the replay's
    /// running state places its start again, as a first attach does.
    fn clear_daemon_transcript(&mut self) {
        // Before the rows go: what the stream held is replayed with them.
        self.reset_streams();
        self.usage.live_stream.clear();
        self.set_history_entries(Vec::new());
        self.turn.clear_tool_calls();
        self.set_todo_list(None);
        self.drop_daemon_inputs();
        self.turn.set_current_turn_start(None);
        self.daemon_compact_turn = None;
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

    /// Ends the busy chrome as the turn rested `rest`. Cards still running
    /// stay in the transcript as interrupted, as a replay of a turn the
    /// daemon never closed leaves them; a session already at rest only
    /// tidies those.
    ///
    /// Pending work settles as an in-process turn's does: an interrupted
    /// turn hands it back to the composer, unless the person's cancel of
    /// questions ended it; any other keeps steers it never applied as
    /// follow-ups, and a completed one, or a `/compact` whose compaction
    /// failed, arms the next follow-up, which the app loop sends once the
    /// composer is free.
    async fn end_daemon_turn(&mut self, rest: Rest) {
        // What the stream still holds of the reply shows before the turn's
        // closing rows, and nothing of it outlasts the turn.
        self.finish_streams();
        self.reset_streams();
        let compact_turn = self.daemon_compact_turn.take();
        let questions_cancelled = self.take_questions_cancelled_turn();
        // The daemon withdrew the turn's own requests before it said the
        // turn ended. A prompt still open is a subagent's question, which
        // outlives the turn.
        for entry in self.turn.interrupted_tool_entries() {
            self.insert_entry(&Entry::Tool(entry));
        }
        self.turn.clear_tool_calls();
        self.turn.set_activity_phase(ActivityPhase::default());
        if !self.turn.is_provider_turn() {
            return;
        }
        // A hub start whose prompt never came back hides no later prompt.
        // Only a turn's own end says so: a rest that tidies an idle session
        // may show between the hub's send and its prompt coming back.
        self.daemon_hub_launch = None;
        self.daemon_rail.turn_ended();
        self.daemon_turn_rested_for_info();
        // Read before `stop_loading` clears the turn clock.
        let worked = self.turn.elapsed_at(Instant::now());
        if matches!(rest, Rest::Interrupted) && !questions_cancelled {
            self.restore_pending_work_to_input();
        } else {
            self.preserve_unapplied_steering_as_follow_ups();
        }
        self.clear_accepted_steering();
        let (status, settled) = match rest {
            Rest::Ready => {
                self.end_provider_turn_ui();
                // In process `/compact` is no turn, so it shows no footer.
                if let Some(worked) = worked.filter(|_| compact_turn.is_none()) {
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
                let status = if questions_cancelled {
                    "questionnaire cancelled"
                } else {
                    "model interrupted"
                };
                (Some(status.to_owned()), ProgramStatus::Idle)
            }
            Rest::Failed { error } => {
                self.end_busy_ui();
                // As in process, a failed compaction still sends what waited
                // for it, as a prompt "Compact, then send" parked.
                if compact_turn.is_some() && self.pending.has_follow_ups() {
                    self.start_follow_ups = Some(ReadyFollowUp::Queued {
                        allow_auto_compact: false,
                    });
                }
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
        let compacted = compact_turn.as_ref().is_some_and(|turn| turn.compacted);
        // A `/compact` turn rests as its compaction ended.
        let compaction_status = compact_turn
            .and_then(|turn| turn.rest_status)
            .map(str::to_owned);
        if let Some(status) = compaction_status.or(status) {
            self.set_status(status);
        }
        self.settle_daemon_compact_handoff(compacted);
        self.settled_program_status = settled;
        self.notifier.turn_finished();
        self.report_resting_herdr_state().await;
    }
}

#[cfg(test)]
#[path = "daemon_transcript_tests.rs"]
mod tests;
