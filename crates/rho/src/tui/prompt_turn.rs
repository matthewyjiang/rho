use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FailedTurn {
    input: rho_sdk::UserInput,
    display_user: Option<Message>,
    notification_context: Option<String>,
    initial_tool_call: Option<rho_sdk::model::ToolCall>,
    generate_session_title_after_completion: bool,
}

impl FailedTurn {
    fn from_prompt(prompt: TurnPrompt, media: Vec<ChatMedia>) -> Result<Self, rho_sdk::Error> {
        let display = prompt.persisted_display.unwrap_or(prompt.display);
        let mut display_content = Vec::with_capacity(1 + media.len());
        display_content.push(ContentBlock::Text(display));
        display_content.extend(media.iter().map(ChatMedia::display_block));

        let mut model_content = Vec::with_capacity(1 + media.len());
        if !prompt.model.is_empty() {
            model_content.push(ContentBlock::Text(prompt.model));
        }
        model_content.extend(media.into_iter().map(ChatMedia::model_block));

        Ok(Self {
            input: rho_sdk::UserInput::content(model_content)?,
            display_user: Some(Message::User(display_content)),
            notification_context: None,
            initial_tool_call: prompt.initial_tool_call,
            generate_session_title_after_completion: false,
        })
    }

    fn session_title_user_message(&self) -> String {
        let Some(Message::User(blocks)) = &self.display_user else {
            return String::new();
        };
        blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn attach_notification_context(&mut self, notification: String) {
        self.notification_context = Some(crate::tools::agent::merge_notification_context(
            self.notification_context.as_deref(),
            &notification,
        ));
    }

    fn model_input(&self) -> Result<rho_sdk::UserInput, rho_sdk::Error> {
        let Some(notification) = &self.notification_context else {
            return Ok(self.input.clone());
        };
        let mut content = Vec::with_capacity(1 + self.input.blocks().len());
        content.push(ContentBlock::Text(notification.clone()));
        content.extend_from_slice(self.input.blocks());
        rho_sdk::UserInput::content(content)
    }
}

enum PromptTurnRequest {
    New {
        prompt: TurnPrompt,
        media: Vec<ChatMedia>,
        /// Idle completion turns already put boundary content in the prompt.
        /// Hold only the restorable batch until provider start commits.
        pre_drained_batch: Option<super::subagent_questionnaires::TurnBoundaryBatch>,
    },
    Retry(FailedTurn),
}

/// Drained turn-boundary work held until provider start accepts the input.
enum RestorableTurnBoundary {
    /// Folded into a real user/command prompt; show a notice on commit.
    Folded(super::subagent_questionnaires::TurnBoundaryDelivery),
    /// Idle synthetic turn; the prompt already carries model/display text.
    Standalone(super::subagent_questionnaires::TurnBoundaryBatch),
}

impl RestorableTurnBoundary {
    fn into_batch(self) -> super::subagent_questionnaires::TurnBoundaryBatch {
        match self {
            Self::Folded(delivery) => delivery.batch,
            Self::Standalone(batch) => batch,
        }
    }

    fn folded_model(&self) -> Option<&str> {
        match self {
            Self::Folded(delivery) => Some(delivery.model.as_str()),
            Self::Standalone(_) => None,
        }
    }

    fn commit_display(&self) -> Option<&str> {
        match self {
            Self::Folded(delivery) => Some(delivery.display.as_str()),
            Self::Standalone(_) => None,
        }
    }

    fn notice_count(&self) -> usize {
        match self {
            Self::Folded(delivery) => delivery.batch.notice_count(),
            Self::Standalone(batch) => batch.notice_count(),
        }
    }
}

async fn questionnaire_reply(
    pending: &mut Option<(
        rho_sdk::ToolCallId,
        rho_sdk::HostInputId,
        tokio::sync::oneshot::Receiver<QuestionnaireReply>,
    )>,
) -> Option<(
    rho_sdk::ToolCallId,
    rho_sdk::HostInputId,
    QuestionnaireReply,
)> {
    let (call_id, request_id, receiver) = pending.as_mut()?;
    let call_id = call_id.clone();
    let request_id = request_id.clone();
    let reply = receiver.await.ok();
    pending.take();
    reply.map(|reply| (call_id, request_id, reply))
}

impl App {
    fn retain_interrupted_tools(&mut self, entries: Vec<ToolEntry>) {
        for entry in entries {
            self.insert_entry(&Entry::Tool(entry));
        }
    }

    pub(super) async fn run_prompt_turn(
        &mut self,
        prompt: TurnPrompt,
        media: Vec<ChatMedia>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(
            PromptTurnRequest::New {
                prompt,
                media,
                pre_drained_batch: None,
            },
            terminal,
            agent,
        )
        .await
    }

    /// Runs a turn whose prompt body is already a drained turn-boundary batch
    /// (idle completion delivery). The batch is restored only if provider start
    /// never accepts the input.
    pub(super) async fn run_turn_boundary_prompt_turn(
        &mut self,
        prompt: TurnPrompt,
        batch: super::subagent_questionnaires::TurnBoundaryBatch,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(
            PromptTurnRequest::New {
                prompt,
                media: Vec::new(),
                pre_drained_batch: Some(batch),
            },
            terminal,
            agent,
        )
        .await
    }

    pub(super) async fn retry_failed_prompt_turn(
        &mut self,
        failed_turn: FailedTurn,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(PromptTurnRequest::Retry(failed_turn), terminal, agent)
            .await
    }

    async fn run_prompt_turn_request(
        &mut self,
        request: PromptTurnRequest,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        let (mut failed_turn, pre_drained_batch) = match request {
            PromptTurnRequest::New {
                prompt,
                media,
                pre_drained_batch,
            } => {
                if !prompt.history.is_empty() {
                    self.push_input_history(&prompt.history);
                }
                self.reset_input_history_navigation();
                self.ensure_session(agent)?;
                self.info
                    .services
                    .herdr
                    .report_session(self.info.session.session_id.as_deref())
                    .await;
                let generate_session_title_after_completion = !agent
                    .history()
                    .iter()
                    .any(|message| matches!(message, Message::User(_)));
                self.insert_entry(&Entry::User(super::message_history::render_user_entry(
                    &prompt.display,
                    &media,
                )));
                let mut failed_turn = FailedTurn::from_prompt(prompt, media)?;
                failed_turn.generate_session_title_after_completion =
                    generate_session_title_after_completion;
                (failed_turn, pre_drained_batch)
            }
            PromptTurnRequest::Retry(failed_turn) => {
                self.ensure_session(agent)?;
                self.info
                    .services
                    .herdr
                    .report_session(self.info.session.session_id.as_deref())
                    .await;
                self.insert_entry(&Entry::Notice(
                    "retrying the previous goal turn without duplicating the prompt".into(),
                ));
                (failed_turn, None)
            }
        };

        // Background completions pending at this turn boundary ride in the
        // same model request. This runs after retry delays too, while the
        // persisted display remains the real user-visible prompt. The drained
        // batch stays restorable until provider start commits delivery.
        // Idle completion turns pass a pre-drained batch whose content is already
        // the prompt body; do not collect or fold again.
        let mut pending_boundary = match pre_drained_batch {
            Some(batch) => Some(RestorableTurnBoundary::Standalone(batch)),
            None => self
                .collect_turn_boundary_prompts(agent)
                .map(RestorableTurnBoundary::Folded),
        };
        if let Some(model) = pending_boundary
            .as_ref()
            .and_then(RestorableTurnBoundary::folded_model)
        {
            failed_turn.attach_notification_context(model.to_owned());
        }
        let model_input = match failed_turn.model_input() {
            Ok(input) => input,
            Err(error) => {
                if let Some(boundary) = pending_boundary.take() {
                    self.restore_turn_boundary_batch(agent, boundary.into_batch());
                }
                return Err(error.into());
            }
        };
        self.turn.set_current_turn_start(Some(self.history.len()));
        self.reset_streams();
        self.turn.reasoning_phase_mut().begin_step();
        self.set_status("running");
        self.begin_provider_turn_ui();
        self.turn.set_activity_phase(ActivityPhase::Starting);
        self.report_herdr_working().await;
        self.turn.start_loading();
        if let Err(error) = self.clamp_history_scroll_for_terminal(terminal) {
            self.abandon_provider_turn_start(agent, &mut pending_boundary);
            return Err(error.into());
        }
        if let Err(error) = terminal.draw(|frame| self.draw(frame)) {
            self.abandon_provider_turn_start(agent, &mut pending_boundary);
            return Err(error.into());
        }

        self.turn.clear_tool_calls();
        let start_result = match failed_turn.initial_tool_call.clone() {
            Some(call) => {
                agent
                    .start_with_tool_call(model_input, failed_turn.display_user.clone(), call)
                    .await
            }
            None => {
                agent
                    .start(model_input, failed_turn.display_user.clone())
                    .await
            }
        };
        if let Err(error) = start_result {
            self.abandon_provider_turn_start(agent, &mut pending_boundary);
            return Err(error.into());
        }
        // Provider start accepted the input; commit drained boundary delivery.
        // Folded deliveries surface a notice; standalone idle completions already
        // used the batch as the user entry, so committing is just dropping it.
        // Either way, free end-to-end notice budget for delivered child notices.
        if let Some(boundary) = pending_boundary.take() {
            let delivered_notices = boundary.notice_count();
            if let Some(display) = boundary.commit_display() {
                self.insert_entry(&Entry::Notice(format!(
                    "delivered with this message:\n{display}"
                )));
            }
            self.subagent_inbox
                .commit_delivered_notices(delivered_notices);
        }
        self.debug_assert_provider_turn_sync(agent);
        self.insert_runtime_notices(agent);
        if let Some(context) = agent.take_context_usage() {
            self.handle_queued_agent_event(ViewModelEvent::ContextUsage(context), terminal)?;
        }

        let interrupt_requested = AtomicBool::new(false);
        let tool_call_active = AtomicBool::new(false);
        let mut adapter = SdkEventAdapter::new(self.info.runtime.cwd.clone());
        let mut frame_scheduler = FrameScheduler::new(Instant::now());
        let mut pending_questionnaire: Option<(
            rho_sdk::ToolCallId,
            rho_sdk::HostInputId,
            oneshot::Receiver<QuestionnaireReply>,
        )> = None;
        let mut queued_interactions = RunningInteractionQueue::default();
        let mut pending_input_request = None;
        let mut approval_receiver_open = agent.approval_receiver().is_some();
        let mut terminal_event = false;
        let mut sdk_failure = None;
        let mut questionnaire_cancelled_by_user = false;
        while !terminal_event {
            if self
                .poll_running_subagent_questionnaire_state(agent.session_id())
                .await?
            {
                self.draw_running_frame(terminal, &mut frame_scheduler)?;
            }
            queued_interactions
                .extend_subagent_questionnaires(self.subagent_inbox.take_questionnaires());
            let panel_changed = self.update_activity_panels(agent);
            let attach_changed = self.poll_pending_subagent_attaches(Instant::now());
            if panel_changed || attach_changed {
                self.draw_running_frame(terminal, &mut frame_scheduler)?;
            }
            if self.poll_limits_command().await? {
                self.draw_running_frame(terminal, &mut frame_scheduler)?;
            }
            if self.poll_changelog_command().await? {
                self.draw_running_frame(terminal, &mut frame_scheduler)?;
            }
            let frame_deadline =
                self.next_running_frame_deadline(frame_scheduler.deferred_deadline());
            let approval_ready = approval_receiver_open;
            tokio::select! {
                biased;
                terminal_event = self.terminal_session.as_mut().expect("terminal session initialized").next_event() => {
                    match self.handle_running_terminal_events(
                        terminal_event?,
                        terminal,
                        &interrupt_requested,
                        &tool_call_active,
                        RunningInputMode::Turn,
                    ).await {
                        Ok(StreamControl::Interrupt) => agent.cancel(),
                        Ok(StreamControl::ApprovalResolved) => {
                            self.report_herdr_working().await;
                        }
                        Ok(StreamControl::Continue | StreamControl::Resize) => {}
                        Err(error) => {
                            agent.cancel();
                            sdk_failure = Some(sdk_failure_from_running_terminal_error(error)?);
                        }
                    }
                    if pending_input_request.is_none() && sdk_failure.is_none() {
                        pending_input_request = self.start_pending_input_request(agent);
                    }
                    self.pending_input_changed();
                    self.draw_running_frame(terminal, &mut frame_scheduler)?;
                }
                completion = pending_input::pending_input_completion(&mut pending_input_request), if pending_input_request.is_some() => {
                    let completion = completion.expect("pending request checked above");
                    let request = pending_input_request
                        .take()
                        .expect("completed pending request exists");
                    if let Some(error) = self.finish_pending_input_request(request, completion) {
                        sdk_failure = Some(error);
                        agent.cancel();
                    }
                    if pending_input_request.is_none() && sdk_failure.is_none() {
                        pending_input_request = self.start_pending_input_request(agent);
                    }
                    self.pending_input_changed();
                    self.draw_running_frame(terminal, &mut frame_scheduler)?;
                }
                reply = questionnaire_reply(&mut pending_questionnaire), if pending_questionnaire.is_some() => {
                    let Some((_call_id, request_id, reply)) = reply else {
                        agent.cancel();
                        continue;
                    };
                    match reply {
                        QuestionnaireReply::Answer(response) => {
                            self.report_herdr_working().await;
                            if let Err(error) = agent
                                .respond(request_id, event_adapter::host_response(response))
                                .await
                            {
                                sdk_failure = Some(error.to_string());
                                agent.cancel();
                            }
                        }
                        QuestionnaireReply::Cancelled(
                            QuestionnaireCancelReason::UserCancelled,
                        ) => {
                            questionnaire_cancelled_by_user = true;
                            agent.cancel();
                        }
                        QuestionnaireReply::Cancelled(QuestionnaireCancelReason::UiUnavailable) => {
                            agent.cancel();
                        }
                    }
                }
                () = self.subagent_inbox.recv() => {
                    self.draw_running_frame(terminal, &mut frame_scheduler)?;
                }
                _ = tokio::time::sleep_until(frame_deadline) => {
                    self.drain_stream_tick(terminal)?;
                    self.flush_due_paste_burst();
                    self.draw_running_frame(terminal, &mut frame_scheduler)?;
                }
                event = next_runtime_event(agent, approval_ready) => {
                    let event = match event {
                        RuntimeEvent::Approval(pending) => {
                            queued_interactions
                                .push(QueuedRunningInteraction::Approval(pending));
                            continue;
                        }
                        RuntimeEvent::ApprovalReceiverClosed => {
                            approval_receiver_open = false;
                            continue;
                        }
                        RuntimeEvent::Agent(Some(event)) => event,
                        RuntimeEvent::Agent(None) => break,
                    };
                    let mut changed = false;
                    if let Some(context) = agent.take_context_usage() {
                        changed |= self.handle_queued_agent_event(
                            ViewModelEvent::ContextUsage(context),
                            terminal,
                        )?;
                    }
                    let view_events = adapter.translate(event);
                    for view_event in view_events {
                        match view_event {
                            ViewEvent::Update(event) => {
                                changed |= self.handle_queued_agent_event(event, terminal)?;
                                tool_call_active.store(
                                    self.turn.tool_calls().is_running(),
                                    Ordering::SeqCst,
                                );
                            }
                            ViewEvent::Questionnaire { call_id, request } => {
                                queued_interactions.push(
                                    QueuedRunningInteraction::ParentQuestionnaire {
                                        call_id,
                                        request,
                                    },
                                );
                                changed = true;
                            }
                            ViewEvent::Notice(notice) => {
                                self.insert_entry(&Entry::Notice(notice));
                                changed = true;
                            }
                            ViewEvent::Completed => terminal_event = true,
                            ViewEvent::Cancelled => terminal_event = true,
                            ViewEvent::Failed(message) => {
                                sdk_failure = Some(message);
                                terminal_event = true;
                            }
                        }
                    }
                    if changed && frame_scheduler.request_background_frame(Instant::now()) {
                        self.draw_running_frame(terminal, &mut frame_scheduler)?;
                    }
                }
            }
            if !terminal_event
                && !questionnaire_cancelled_by_user
                && sdk_failure.is_none()
                && interaction_slot_available(
                    /*approval_active*/
                    matches!(self.input_ui.composer(), ComposerMode::Approval(_)),
                    /*questionnaire_active*/
                    pending_questionnaire.is_some()
                        || self.pending_subagent_questionnaire.is_some(),
                )
            {
                while let Some(interaction) = queued_interactions.pop() {
                    let presented = match interaction {
                        QueuedRunningInteraction::Approval(pending) => {
                            self.finish_streams();
                            self.open_approval(pending).await;
                            true
                        }
                        QueuedRunningInteraction::ParentQuestionnaire { call_id, request } => {
                            pending_questionnaire =
                                Some(self.begin_pending_questionnaire(call_id, request).await?);
                            true
                        }
                        QueuedRunningInteraction::SubagentQuestionnaire(request) => {
                            if request.parent_session_id != *agent.session_id() {
                                let _ = request.response.send(Err(rho_sdk::Error::Interrupted {
                                    message: "parent session changed before the delegated questionnaire was shown".into(),
                                }));
                                false
                            } else {
                                self.present_subagent_questionnaire(request).await?
                            }
                        }
                    };
                    if presented {
                        self.draw_running_frame(terminal, &mut frame_scheduler)?;
                        break;
                    }
                }
            }
            if pending_input_request.is_none()
                && sdk_failure.is_none()
                && !self.pending.steering_prompts().is_empty()
            {
                pending_input_request = self.start_pending_input_request(agent);
                self.pending_input_changed();
            }
            if self.finish_completed_inline_shells().await? {
                self.clamp_history_scroll_for_terminal(terminal)?;
                terminal.draw(|frame| self.draw(frame))?;
            }
        }

        self.subagent_inbox
            .return_questionnaires(queued_interactions.into_subagent_questionnaires());

        if pending_input_request.is_some() {
            let completion = pending_input::pending_input_completion(&mut pending_input_request)
                .await
                .expect("pending request checked above");
            let request = pending_input_request
                .take()
                .expect("completed pending request exists");
            if let Some(error) = self.finish_pending_input_request(request, completion) {
                sdk_failure = Some(error);
            }
        }

        self.cancel_approval();
        let interrupted_tool_entries = self.turn.interrupted_tool_entries();
        self.turn.clear_tool_calls();
        tool_call_active.store(false, Ordering::SeqCst);
        let result = agent.finish_run().await;
        let inline_shell_error = match self.finish_all_inline_shells().await {
            Ok(()) => self.insert_deferred_inline_shell_context(agent).err(),
            Err(error) => Some(error),
        };
        if let Some(context) = agent.take_context_usage() {
            self.handle_queued_agent_event(ViewModelEvent::ContextUsage(context), terminal)?;
        }
        let outcome = match result {
            _ if inline_shell_error.is_some() => {
                let outcome = self.finalize_failed_turn(
                    inline_shell_error
                        .expect("inline shell error checked above")
                        .to_string(),
                    failed_turn,
                    interrupted_tool_entries,
                );
                self.debug_assert_provider_turn_sync(agent);
                outcome
            }
            Ok(outcome) if sdk_failure.is_none() => {
                self.end_busy_ui();
                self.debug_assert_provider_turn_sync(agent);
                self.turn.stop_loading();
                self.finish_streams();
                self.insert_final_answer_suffix(outcome.text());
                if failed_turn.generate_session_title_after_completion {
                    self.start_session_title_generation(
                        &failed_turn.session_title_user_message(),
                        outcome.text(),
                        agent,
                    );
                }
                self.reset_streams();
                self.turn.set_current_turn_start(None);
                self.set_status(if !self.pending.has_follow_ups() {
                    "ready".to_string()
                } else {
                    format!(
                        "running next queued message ({})",
                        self.pending.follow_up_len()
                    )
                });
                TurnOutcome::Completed
            }
            _ if questionnaire_cancelled_by_user => {
                self.end_busy_ui();
                self.debug_assert_provider_turn_sync(agent);
                self.turn.stop_loading();
                self.finish_streams();
                let notice = if self.goal.is_some() {
                    "questionnaire cancelled; goal left active"
                } else {
                    "questionnaire cancelled"
                };
                self.retain_interrupted_tools(interrupted_tool_entries);
                self.reset_streams();
                self.turn.set_current_turn_start(None);
                self.set_status(notice);
                TurnOutcome::Cancelled
            }
            Err(error)
                if matches!(
                    error.downcast_ref::<rho_sdk::Error>(),
                    Some(rho_sdk::Error::Cancelled | rho_sdk::Error::Interrupted { .. })
                ) =>
            {
                self.restore_pending_work_to_input();
                self.end_busy_ui();
                self.debug_assert_provider_turn_sync(agent);
                self.turn.stop_loading();
                self.finish_streams();
                self.retain_interrupted_tools(interrupted_tool_entries);
                self.reset_streams();
                self.turn.set_current_turn_start(None);
                self.set_status("model interrupted");
                TurnOutcome::Interrupted
            }
            result => {
                let message = sdk_failure.unwrap_or_else(|| match result {
                    Ok(_) => "model run failed".into(),
                    Err(error) => error.to_string(),
                });
                let outcome =
                    self.finalize_failed_turn(message, failed_turn, interrupted_tool_entries);
                self.debug_assert_provider_turn_sync(agent);
                outcome
            }
        };
        let completed = matches!(outcome, TurnOutcome::Completed);
        if completed {
            agent.mark_live_context_warm();
        }
        if matches!(&outcome, TurnOutcome::Failed(_) | TurnOutcome::Cancelled) {
            self.preserve_unapplied_steering_as_follow_ups();
        }
        self.clear_accepted_steering();
        self.apply_pending_model_selection(agent, completed).await?;
        if self.pending_subagent_questionnaire.is_some() {
            self.set_status(HerdrUserWait::Questionnaire.message());
        }
        self.report_resting_herdr_state().await;
        terminal.draw(|frame| self.draw(frame))?;
        Ok(outcome)
    }

    fn draw_running_frame(
        &mut self,
        terminal: &mut DefaultTerminal,
        frame_scheduler: &mut FrameScheduler,
    ) -> anyhow::Result<()> {
        self.clamp_history_scroll_for_terminal(terminal)?;
        terminal.draw(|frame| self.draw(frame))?;
        frame_scheduler.rendered(Instant::now());
        Ok(())
    }

    async fn begin_pending_questionnaire(
        &mut self,
        call_id: rho_sdk::ToolCallId,
        request: rho_sdk::HostInputRequest,
    ) -> anyhow::Result<(
        rho_sdk::ToolCallId,
        rho_sdk::HostInputId,
        oneshot::Receiver<QuestionnaireReply>,
    )> {
        let request_id = request.id().clone();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.open_questionnaire(QuestionAnswerRequest {
            request: event_adapter::questionnaire_request(&request),
            response: QuestionnaireResponseChannel::new(reply_tx),
            notice: None,
        })
        .await?;
        Ok((call_id, request_id, reply_rx))
    }

    fn finalize_failed_turn(
        &mut self,
        message: String,
        failed_turn: FailedTurn,
        interrupted_tool_entries: Vec<ToolEntry>,
    ) -> TurnOutcome {
        self.finish_streams();
        self.retain_interrupted_tools(interrupted_tool_entries);
        self.reset_streams();
        self.turn.set_current_turn_start(None);
        self.end_busy_ui();
        self.turn.stop_loading();
        self.insert_entry(&Entry::Error(message));
        self.set_status("error");
        TurnOutcome::Failed(Box::new(failed_turn))
    }

    /// Clears start-time busy chrome and returns drained turn-boundary work
    /// when provider start never accepted the input.
    fn abandon_provider_turn_start(
        &mut self,
        agent: &mut InteractiveRuntime,
        pending_boundary: &mut Option<RestorableTurnBoundary>,
    ) {
        self.end_busy_ui();
        self.turn.stop_loading();
        // begin_step() already opened the stretch; drop it so idle draws
        // do not keep a stale Thinking... line after a failed provider start.
        self.turn.reasoning_phase_mut().reset();
        self.turn.set_current_turn_start(None);
        self.turn.set_activity_phase(ActivityPhase::default());
        self.set_status("ready");
        if let Some(boundary) = pending_boundary.take() {
            self.restore_turn_boundary_batch(agent, boundary.into_batch());
        }
    }
}

fn interaction_slot_available(approval_active: bool, questionnaire_active: bool) -> bool {
    !approval_active && !questionnaire_active
}

enum QueuedRunningInteraction {
    Approval(rho_sdk::PendingApproval),
    ParentQuestionnaire {
        call_id: rho_sdk::ToolCallId,
        request: rho_sdk::HostInputRequest,
    },
    SubagentQuestionnaire(crate::app::subagent_host_input::SubagentHostInputRequest),
}

#[derive(Default)]
struct RunningInteractionQueue {
    pending: VecDeque<QueuedRunningInteraction>,
}

impl RunningInteractionQueue {
    fn push(&mut self, interaction: QueuedRunningInteraction) {
        self.pending.push_back(interaction);
    }

    fn extend_subagent_questionnaires(
        &mut self,
        requests: impl IntoIterator<Item = crate::app::subagent_host_input::SubagentHostInputRequest>,
    ) {
        self.pending.extend(
            requests
                .into_iter()
                .map(QueuedRunningInteraction::SubagentQuestionnaire),
        );
    }

    fn pop(&mut self) -> Option<QueuedRunningInteraction> {
        self.pending.pop_front()
    }

    fn into_subagent_questionnaires(
        self,
    ) -> impl Iterator<Item = crate::app::subagent_host_input::SubagentHostInputRequest> {
        self.pending.into_iter().filter_map(|interaction| {
            if let QueuedRunningInteraction::SubagentQuestionnaire(request) = interaction {
                Some(request)
            } else {
                None
            }
        })
    }
}

enum RuntimeEvent {
    Approval(rho_sdk::PendingApproval),
    ApprovalReceiverClosed,
    Agent(Option<rho_sdk::RunEvent>),
}

fn sdk_failure_from_running_terminal_error(
    error: super::during_turn::RunningTerminalError,
) -> anyhow::Result<String> {
    match error {
        super::during_turn::RunningTerminalError::Recoverable(error) => Ok(error.to_string()),
        super::during_turn::RunningTerminalError::Terminal(error) => Err(error),
    }
}

async fn next_runtime_event(
    agent: &mut InteractiveRuntime,
    receive_approval: bool,
) -> RuntimeEvent {
    std::future::poll_fn(|context| {
        if receive_approval {
            if let Some(receiver) = agent.approval_receiver() {
                let approval = receiver.recv();
                tokio::pin!(approval);
                if let std::task::Poll::Ready(approval) = approval.poll(context) {
                    return std::task::Poll::Ready(match approval {
                        Some(pending) => RuntimeEvent::Approval(pending),
                        None => RuntimeEvent::ApprovalReceiverClosed,
                    });
                }
            }
        }

        let event = agent.next_event();
        tokio::pin!(event);
        event.poll(context).map(RuntimeEvent::Agent)
    })
    .await
}

#[cfg(test)]
#[path = "prompt_turn_tests.rs"]
mod tests;
