use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FailedTurn {
    input: rho_sdk::UserInput,
    display_user: Option<Message>,
}

enum PromptTurnRequest {
    New {
        prompt: TurnPrompt,
        images: Vec<ImageContent>,
    },
    Retry(FailedTurn),
}

impl App {
    pub(super) async fn run_prompt_turn(
        &mut self,
        prompt: TurnPrompt,
        images: Vec<ImageContent>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(PromptTurnRequest::New { prompt, images }, terminal, agent)
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
        let failed_turn = match request {
            PromptTurnRequest::New { prompt, images } => {
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
                if !agent
                    .history()
                    .iter()
                    .any(|message| matches!(message, Message::User(_)))
                {
                    self.start_session_title_generation(prompt.history.clone(), agent);
                }
                self.insert_entry(&Entry::User(render_user_entry(&prompt.display, &images)));

                let mut content = Vec::with_capacity(1 + images.len());
                if !prompt.model.is_empty() {
                    content.push(ContentBlock::Text(prompt.model));
                }
                content.extend(images.iter().cloned().map(ContentBlock::Image));
                let input = rho_sdk::UserInput::content(content)?;
                let display_user = prompt.persisted_display.map(|display| {
                    let mut content = Vec::with_capacity(1 + images.len());
                    content.push(ContentBlock::Text(display));
                    content.extend(images.into_iter().map(ContentBlock::Image));
                    Message::User(content)
                });
                FailedTurn {
                    input,
                    display_user,
                }
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
                failed_turn
            }
        };
        self.current_turn_start = Some(self.transcript.len());
        self.active_turn_show_reasoning_output = self.info.runtime.show_reasoning_output;
        self.reset_streams();
        self.hidden_reasoning_active = !self.active_turn_show_reasoning_output;
        self.status = "running".into();
        self.running = true;
        self.activity_phase = ActivityPhase::Starting;
        self.info
            .services
            .herdr
            .report_state(
                HerdrState::Working,
                None,
                self.info.session.session_id.as_deref(),
            )
            .await;
        self.loading_spinner.start();
        self.clamp_history_scroll_for_terminal(terminal)?;
        terminal.draw(|frame| self.draw(frame))?;

        self.active_tool_call = false;
        self.pending_tool_call = None;
        agent
            .start(failed_turn.input.clone(), failed_turn.display_user.clone())
            .await?;
        self.insert_runtime_notices(agent);
        if let Some(context) = agent.take_context_usage() {
            self.handle_queued_agent_event(ViewModelEvent::ContextUsage(context), terminal)?;
        }

        let interrupt_requested = AtomicBool::new(false);
        let tool_call_active = AtomicBool::new(false);
        let mut adapter = SdkEventAdapter::new(self.info.runtime.cwd.clone());
        let mut frame_scheduler = FrameScheduler::new(Instant::now());
        let mut pending_questionnaire: Option<(
            rho_sdk::HostInputId,
            oneshot::Receiver<QuestionnaireReply>,
        )> = None;
        let mut pending_input_request = None;
        let mut approval_receiver_open = agent.approval_receiver().is_some();
        let mut terminal_event = false;
        let mut sdk_failure = None;
        let mut questionnaire_cancelled_by_user = false;
        while !terminal_event {
            if self.update_subagent_panel(agent) {
                self.draw_running_frame(terminal, &mut frame_scheduler)?;
            }
            if self.poll_limits_command().await? {
                self.draw_running_frame(terminal, &mut frame_scheduler)?;
            }
            let frame_deadline =
                self.next_running_frame_deadline(frame_scheduler.deferred_deadline());
            let approval_ready =
                approval_receiver_open && !matches!(self.composer, ComposerMode::Approval(_));
            tokio::select! {
                biased;
                terminal_event = self.terminal_events.as_mut().expect("terminal events initialized").next() => {
                    match self.handle_running_terminal_events(
                        terminal_event?,
                        terminal,
                        &interrupt_requested,
                        &tool_call_active,
                        RunningInputMode::Turn,
                    ) {
                        Ok(StreamControl::Interrupt) => agent.cancel(),
                        Ok(StreamControl::Continue | StreamControl::Resize) => {}
                        Err(error) => {
                            sdk_failure = Some(error.to_string());
                            agent.cancel();
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
                    let Some((request_id, reply)) = reply else {
                        agent.cancel();
                        continue;
                    };
                    match reply {
                        QuestionnaireReply::Answer(response) => {
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
                _ = tokio::time::sleep_until(frame_deadline) => {
                    self.drain_stream_preview(terminal)?;
                    self.flush_due_paste_burst();
                    self.draw_running_frame(terminal, &mut frame_scheduler)?;
                }
                event = next_runtime_event(agent, approval_ready) => {
                    let event = match event {
                        RuntimeEvent::Approval(pending) => {
                            self.finish_streams();
                            self.open_approval(pending);
                            self.draw_running_frame(terminal, &mut frame_scheduler)?;
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
                    let mut interaction_ready = false;
                    if let Some(context) = agent.take_context_usage() {
                        changed |= self.handle_queued_agent_event(
                            ViewModelEvent::ContextUsage(context),
                            terminal,
                        )?;
                    }
                    match adapter.translate(event) {
                        ViewEvent::Update(event) => {
                            changed |= self.handle_queued_agent_event(event, terminal)?;
                            tool_call_active.store(self.active_tool_call, Ordering::SeqCst);
                        }
                        ViewEvent::Questionnaire(request) => {
                            let request_id = request.id().clone();
                            let (reply_tx, reply_rx) = oneshot::channel();
                            self.open_questionnaire(
                                QuestionAnswerRequest {
                                    request: event_adapter::questionnaire_request(&request),
                                    response: QuestionnaireResponseChannel::new(reply_tx),
                                },
                                terminal,
                            )?;
                            pending_questionnaire = Some((request_id, reply_rx));
                            changed = true;
                            interaction_ready = true;
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
                        ViewEvent::Ignored => {}
                    }
                    let render_now = interaction_ready
                        || (changed && frame_scheduler.request_background_frame(Instant::now()));
                    if render_now {
                        self.draw_running_frame(terminal, &mut frame_scheduler)?;
                    }
                }
            }
            if pending_input_request.is_none()
                && sdk_failure.is_none()
                && !self.steering_prompts.is_empty()
            {
                pending_input_request = self.start_pending_input_request(agent);
                self.pending_input_changed();
            }
            if self.finish_completed_inline_shells().await? {
                self.clamp_history_scroll_for_terminal(terminal)?;
                terminal.draw(|frame| self.draw(frame))?;
            }
        }

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
        self.active_tool_call = false;
        self.pending_tool_call = None;
        tool_call_active.store(false, Ordering::SeqCst);
        let result = agent.finish_run().await;
        let inline_shell_error = match self.finish_all_inline_shells().await {
            Ok(()) => self.insert_deferred_inline_shell_context(agent).err(),
            Err(error) => Some(error),
        };
        let outcome = match result {
            _ if inline_shell_error.is_some() => self.finalize_failed_turn(
                inline_shell_error
                    .expect("inline shell error checked above")
                    .to_string(),
                failed_turn,
            ),
            Ok(outcome) if sdk_failure.is_none() => {
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams();
                self.insert_final_answer_suffix(outcome.text());
                self.reset_streams();
                self.current_turn_start = None;
                self.status = if self.queued_prompts.is_empty() {
                    "ready".into()
                } else {
                    format!(
                        "running next queued message ({})",
                        self.queued_prompts.len()
                    )
                };
                TurnOutcome::Completed
            }
            _ if questionnaire_cancelled_by_user => {
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams();
                let notice = if self.goal.is_some() {
                    "questionnaire cancelled; goal left active"
                } else {
                    "questionnaire cancelled"
                };
                self.insert_entry(&Entry::Notice(notice.into()));
                self.reset_streams();
                self.current_turn_start = None;
                self.status = "questionnaire cancelled".into();
                TurnOutcome::Cancelled
            }
            Err(error)
                if matches!(
                    error.downcast_ref::<rho_sdk::Error>(),
                    Some(rho_sdk::Error::Cancelled | rho_sdk::Error::Interrupted { .. })
                ) =>
            {
                self.restore_pending_work_to_input();
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams();
                self.insert_entry(&Entry::Notice("model interrupted".into()));
                self.reset_streams();
                self.current_turn_start = None;
                self.status = "interrupted".into();
                TurnOutcome::Interrupted
            }
            result => {
                let message = sdk_failure.unwrap_or_else(|| match result {
                    Ok(_) => "model run failed".into(),
                    Err(error) => error.to_string(),
                });
                self.finalize_failed_turn(message, failed_turn)
            }
        };
        if matches!(&outcome, TurnOutcome::Failed(_) | TurnOutcome::Cancelled) {
            self.preserve_unapplied_steering_as_follow_ups();
        }
        self.clear_accepted_steering();
        self.apply_pending_model_selection(agent)?;
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

    fn finalize_failed_turn(&mut self, message: String, failed_turn: FailedTurn) -> TurnOutcome {
        self.finish_streams();
        self.reset_streams();
        self.current_turn_start = None;
        self.running = false;
        self.loading_spinner.stop();
        self.insert_entry(&Entry::Error(message));
        self.status = "error".into();
        TurnOutcome::Failed(failed_turn)
    }
}

enum RuntimeEvent {
    Approval(rho_sdk::PendingApproval),
    ApprovalReceiverClosed,
    Agent(Option<rho_sdk::RunEvent>),
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
