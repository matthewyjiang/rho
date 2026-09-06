use super::subagent_delivery::TurnBoundaryDelivery;
use super::*;
use crate::app::interactive_runtime::DisplayCommit;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FailedTurn {
    input: rho_sdk::UserInput,
    display_user: Vec<Message>,
    display_commit: DisplayCommit,
    notification_context: Option<String>,
    boundary_recovery: Vec<BoundaryRecovery>,
    initial_tool_call: Option<rho_sdk::model::ToolCall>,
    generate_session_title_after_completion: bool,
    session_title_user: Option<String>,
    /// Keep action-request retries runnable even after delivery consumed the notice.
    parent_action_required: bool,
}

/// Accepted in-turn input is replayed only if its durable checkpoint was lost.
#[derive(Clone, Debug, PartialEq)]
struct BoundaryRecovery {
    model: String,
    display: Message,
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
            display_user: vec![Message::User(display_content)],
            display_commit: DisplayCommit::Unsaved,
            notification_context: None,
            boundary_recovery: Vec::new(),
            initial_tool_call: prompt.initial_tool_call,
            generate_session_title_after_completion: false,
            session_title_user: None,
            parent_action_required: false,
        })
    }

    /// Newer failed deliveries can precede an action retry. Keep the queue
    /// runnable until every action retry has been consumed.
    pub(super) fn retries_need_parent_action(retries: &VecDeque<Self>) -> bool {
        retries.iter().any(|retry| retry.parent_action_required)
    }

    fn session_title_user_message(&self) -> String {
        let Some(Message::User(blocks)) = self
            .display_user
            .iter()
            .find(|message| matches!(message, Message::User(_)))
        else {
            return self.session_title_user.clone().unwrap_or_default();
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

    /// Retry model input even when its display is already saved. Unsaved display
    /// must travel with the next attempt so persistence can recover it.
    fn prepare_retry(&mut self) {
        if self.generate_session_title_after_completion
            && !matches!(self.display_commit, DisplayCommit::Unsaved)
        {
            self.session_title_user = Some(self.session_title_user_message());
        }
        match std::mem::take(&mut self.display_commit) {
            DisplayCommit::Unsaved => {}
            DisplayCommit::Complete => {
                self.display_user.clear();
                self.boundary_recovery.clear();
            }
            DisplayCommit::Checkpoint(saved) => {
                for message in saved {
                    if let Some(index) = self
                        .display_user
                        .iter()
                        .position(|display| display == &message)
                    {
                        self.display_user.remove(index);
                    }
                    if let Some(index) = self
                        .boundary_recovery
                        .iter()
                        .position(|recovery| recovery.display == message)
                    {
                        self.boundary_recovery.remove(index);
                    }
                }
            }
        }
    }

    fn attach_notification_context(&mut self, notification: String) {
        self.notification_context = Some(crate::tools::agent::merge_notification_context(
            self.notification_context.as_deref(),
            &notification,
        ));
    }

    fn model_input(&self) -> Result<rho_sdk::UserInput, rho_sdk::Error> {
        let mut notification = self.notification_context.clone();
        for recovery in &self.boundary_recovery {
            notification = Some(crate::tools::agent::merge_notification_context(
                notification.as_deref(),
                &recovery.model,
            ));
        }
        let Some(notification) = notification else {
            return Ok(self.input.clone());
        };
        let mut content = Vec::with_capacity(1 + self.input.blocks().len());
        content.push(ContentBlock::Text(notification));
        content.extend_from_slice(self.input.blocks());
        rho_sdk::UserInput::content(content)
    }
}

enum PromptTurnRequest {
    New {
        prompt: TurnPrompt,
        media: Vec<ChatMedia>,
    },
    Boundary(TurnBoundaryDelivery),
    Retry(FailedTurn),
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
        authorization: super::send_confirm::SendAuthorization,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(
            PromptTurnRequest::New { prompt, media },
            authorization,
            terminal,
            agent,
        )
        .await
    }

    /// Runs a prepared idle delivery, restoring its batch if provider start fails.
    pub(super) async fn run_turn_boundary_prompt_turn(
        &mut self,
        delivery: TurnBoundaryDelivery,
        authorization: super::send_confirm::SendAuthorization,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(
            PromptTurnRequest::Boundary(delivery),
            authorization,
            terminal,
            agent,
        )
        .await
    }

    pub(super) async fn retry_failed_prompt_turn(
        &mut self,
        failed_turn: FailedTurn,
        authorization: super::send_confirm::SendAuthorization,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(
            PromptTurnRequest::Retry(failed_turn),
            authorization,
            terminal,
            agent,
        )
        .await
    }

    async fn run_prompt_turn_request(
        &mut self,
        request: PromptTurnRequest,
        authorization: super::send_confirm::SendAuthorization,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        if !authorization.matches(&agent.provider_identity()) {
            if let PromptTurnRequest::Boundary(delivery) = request {
                self.restore_turn_boundary_batch(agent, delivery.batch);
            }
            anyhow::bail!("send authorization became stale before provider start");
        }
        let (mut failed_turn, prepared_boundary) = match request {
            PromptTurnRequest::New { prompt, media } => {
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
                (failed_turn, None)
            }
            PromptTurnRequest::Boundary(delivery) => {
                if let Err(error) = self.ensure_session(agent) {
                    self.restore_turn_boundary_batch(agent, delivery.batch);
                    return Err(error);
                }
                self.info
                    .services
                    .herdr
                    .report_session(self.info.session.session_id.as_deref())
                    .await;
                let failed_turn = FailedTurn {
                    input: rho_sdk::UserInput::text(delivery.model.clone()),
                    display_user: vec![delivery.transcript.display_message()],
                    display_commit: DisplayCommit::Unsaved,
                    notification_context: None,
                    boundary_recovery: Vec::new(),
                    initial_tool_call: None,
                    generate_session_title_after_completion: false,
                    session_title_user: None,
                    parent_action_required: false,
                };
                (failed_turn, Some(delivery))
            }
            PromptTurnRequest::Retry(mut failed_turn) => {
                self.ensure_session(agent)?;
                self.info
                    .services
                    .herdr
                    .report_session(self.info.session.session_id.as_deref())
                    .await;
                self.insert_entry(&Entry::Notice(
                    "retrying the previous goal turn without duplicating the prompt".into(),
                ));
                failed_turn.prepare_retry();
                (failed_turn, None)
            }
        };

        // A prepared standalone turn already owns its model and display input.
        // Human turns and retries fold new arrivals in without replacing the human.
        let mut pending_boundary = prepared_boundary.or_else(|| {
            let delivery = self.collect_turn_boundary_prompts(agent)?;
            failed_turn.attach_notification_context(delivery.model.clone());
            failed_turn
                .display_user
                .push(delivery.transcript.display_message());
            Some(delivery)
        });
        failed_turn.parent_action_required |= pending_boundary
            .as_ref()
            .is_some_and(|boundary| boundary.batch.requires_parent_action());
        let model_input = match failed_turn.model_input() {
            Ok(input) => input,
            Err(error) => {
                if let Some(boundary) = pending_boundary.take() {
                    self.restore_turn_boundary_batch(agent, boundary.batch);
                }
                return Err(error.into());
            }
        };
        if agent.take_tool_list_changed() {
            self.usage.cache_stats.note_tool_list_changed();
        }
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
        let mut boundary_requests = match agent
            .start_with_boundary_inputs(
                model_input,
                Some(failed_turn.display_user.clone()),
                failed_turn.initial_tool_call.clone(),
            )
            .await
        {
            Ok(receiver) => receiver,
            Err(error) => {
                self.abandon_provider_turn_start(agent, &mut pending_boundary);
                return Err(error.into());
            }
        };
        // Provider start accepted the input; commit drained boundary delivery.
        // Both folded and standalone deliveries use message cards, never a
        // synthetic human turn. Free notice budget only after acceptance.
        if let Some(boundary) = pending_boundary.take() {
            let delivered_notices = boundary.batch.notice_count();
            for entry in super::message_history::transcript_entries(boundary.transcript) {
                self.insert_entry(&entry);
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
        let mut pending_boundary_display = VecDeque::new();
        let mut sdk_failure = None;
        let mut questionnaire_cancelled_by_user = false;
        while !terminal_event {
            let mut needs_redraw = self
                .poll_running_subagent_questionnaire_state(agent.session_id())
                .await?;
            queued_interactions
                .extend_subagent_questionnaires(self.subagent_inbox.take_questionnaires());
            needs_redraw |= self.update_activity_panels(agent)?;
            needs_redraw |= self.poll_overlay_tasks().await?;
            if needs_redraw {
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
                                .respond(request_id, response)
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
                Some(request) = boundary_requests.recv() => {
                    if let Some((model, transcript)) = self.deliver_running_boundary(request, agent).await {
                        // Acceptance is not a durable save. Retain findings and
                        // display receipts if this run later rolls back and retries.
                        let display = transcript.display_message();
                        failed_turn.display_user.push(display.clone());
                        failed_turn.boundary_recovery.push(BoundaryRecovery { model, display });
                        pending_boundary_display.push_back(transcript);
                    }
                }
                _ = tokio::time::sleep_until(frame_deadline) => {
                    self.tick_questionnaire_timeout();
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
                    if matches!(&event, rho_sdk::RunEvent::BoundaryInputApplied { .. }) {
                        if let Some(transcript) = pending_boundary_display.pop_front() {
                            self.finish_streams();
                            for entry in super::message_history::transcript_entries(transcript) {
                                self.insert_entry(&entry);
                            }
                            changed = true;
                        }
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
        failed_turn.display_commit = agent.take_last_turn_display_commit();
        let inline_shell_error = match self.finish_all_inline_shells().await {
            Ok(()) => self.insert_deferred_inline_shell_context(agent).err(),
            Err(error) => Some(error),
        };
        if let Some(context) = agent.take_context_usage() {
            self.handle_queued_agent_event(ViewModelEvent::ContextUsage(context), terminal)?;
        }
        // Capture before `stop_loading` clears the turn clock.
        let turn_elapsed = self.turn.elapsed_at(Instant::now());
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
                self.end_provider_turn_ui();
                self.debug_assert_provider_turn_sync(agent);
                self.turn.stop_loading();
                self.finish_streams();
                self.insert_final_answer_suffix(outcome.text());
                self.insert_assistant_images(outcome.content());
                if let Some(elapsed) = turn_elapsed {
                    self.attach_turn_worked(elapsed);
                }
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
        // Cancellation can acknowledge a boundary without applying it. Flush
        // leftovers only after final assistant text and interrupted tools.
        for transcript in pending_boundary_display {
            for entry in super::message_history::transcript_entries(transcript) {
                self.insert_entry(&entry);
            }
        }
        let completed = matches!(outcome, TurnOutcome::Completed);
        if completed {
            agent.mark_live_context_warm();
        }
        self.insert_cache_miss_notices(completed);
        if matches!(&outcome, TurnOutcome::Failed(_) | TurnOutcome::Cancelled) {
            self.preserve_unapplied_steering_as_follow_ups();
        }
        self.clear_accepted_steering();
        self.apply_pending_model_selection(agent).await?;
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
            request,
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
        pending_boundary: &mut Option<TurnBoundaryDelivery>,
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
            self.restore_turn_boundary_batch(agent, boundary.batch);
        }
    }

    /// Drain significant misses detected during this turn into the transcript.
    ///
    /// Always drains so notices cannot leak into a later turn, and inserts only
    /// on completed turns when the user opted in.
    fn insert_cache_miss_notices(&mut self, turn_completed: bool) {
        let notices = self.usage.cache_stats.take_turn_notices();
        if !turn_completed || !self.info.runtime.cache_miss_notices {
            return;
        }
        for notice in notices {
            self.insert_entry(&Entry::Notice(super::cache_stats::notice_text(&notice)));
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
