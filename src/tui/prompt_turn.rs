use super::*;

enum PromptTurnRequest {
    New {
        prompt: TurnPrompt,
        images: Vec<ImageContent>,
    },
    /// Retry the previous failed model turn without appending another user message.
    RetryFailed,
}

impl App {
    pub(super) async fn run_prompt_turn(
        &mut self,
        prompt: TurnPrompt,
        images: Vec<ImageContent>,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(PromptTurnRequest::New { prompt, images }, terminal, agent)
            .await
    }

    pub(super) async fn retry_failed_prompt_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<TurnOutcome> {
        self.run_prompt_turn_request(PromptTurnRequest::RetryFailed, terminal, agent)
            .await
    }

    async fn run_prompt_turn_request(
        &mut self,
        request: PromptTurnRequest,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<TurnOutcome> {
        let user_content = match &request {
            PromptTurnRequest::New { prompt, images } => {
                if !prompt.history.is_empty() {
                    self.push_input_history(&prompt.history);
                }
                self.reset_input_history_navigation();
                self.ensure_session(agent)?;
                self.info
                    .herdr
                    .report_session(self.info.session_id.as_deref())
                    .await;
                if !agent
                    .messages()
                    .iter()
                    .any(|message| matches!(message, Message::User(_)))
                {
                    self.start_session_title_generation(prompt.history.clone());
                }
                self.insert_entry(&Entry::User(render_user_entry(&prompt.display, images)));
                let mut content = Vec::with_capacity(1 + images.len());
                if !prompt.model.is_empty() {
                    content.push(ContentBlock::Text(prompt.model.clone()));
                }
                let display_content = prompt.persisted_display.as_ref().map(|display| {
                    let mut display_content = Vec::with_capacity(1 + images.len());
                    display_content.push(ContentBlock::Text(display.clone()));
                    display_content.extend(images.iter().cloned().map(ContentBlock::Image));
                    display_content
                });
                content.extend(images.iter().cloned().map(ContentBlock::Image));
                Some(match display_content {
                    Some(display) => ModelAndDisplayContent::Separate {
                        model: content,
                        display,
                    },
                    None => ModelAndDisplayContent::Same(content),
                })
            }
            PromptTurnRequest::RetryFailed => {
                self.ensure_session(agent)?;
                self.info
                    .herdr
                    .report_session(self.info.session_id.as_deref())
                    .await;
                self.insert_entry(&Entry::Notice(
                    "retrying the previous goal turn without duplicating the prompt".into(),
                ));
                None
            }
        };
        self.current_turn_start = Some(self.transcript.len());
        self.active_turn_show_reasoning_output = self.info.show_reasoning_output;
        self.reset_streams();
        self.hidden_reasoning_active = !self.active_turn_show_reasoning_output;
        self.status = "running".into();
        self.running = true;
        self.info
            .herdr
            .report_state(HerdrState::Working, None, self.info.session_id.as_deref())
            .await;
        self.loading_spinner.start();
        self.clamp_history_scroll_for_terminal(terminal)?;
        terminal.draw(|frame| self.draw(frame))?;

        if let Ok(config) = self.info.config_repository.load() {
            agent.set_compaction_config((&config).into());
        }
        self.active_tool_call = false;
        self.pending_tool_call = None;
        let interrupt_requested = Arc::new(AtomicBool::new(false));
        let cancellation = crate::cancellation::RunCancellation::default();
        let tool_call_active = Arc::new(AtomicBool::new(false));
        let steering_prompts = Arc::new(Mutex::new(VecDeque::new()));
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let result = {
            let callback_interrupt_requested = Arc::clone(&interrupt_requested);
            let run_interrupt_requested = Arc::clone(&interrupt_requested);
            let run_cancellation = cancellation.clone();
            let callback_tool_call_active = Arc::clone(&tool_call_active);
            let run_steering_prompts = Arc::clone(&steering_prompts);
            let (question_tx, mut question_rx) = mpsc::unbounded_channel::<QuestionAnswerRequest>();
            let question_request_tx = question_tx.clone();
            let mut ask_questionnaire =
                move |request: QuestionnaireRequest| -> crate::agent::QuestionnaireFuture {
                    let question_request_tx = question_request_tx.clone();
                    let (reply_tx, reply_rx) = oneshot::channel();
                    Box::pin(async move {
                        question_request_tx
                            .send(QuestionAnswerRequest {
                                request,
                                response: QuestionnaireResponseChannel::new(reply_tx),
                            })
                            .map_err(|_| {
                                crate::agent::AgentError::Questionnaire(
                                    "questionnaire UI is unavailable".into(),
                                )
                            })?;
                        match reply_rx.await {
                            Ok(QuestionnaireReply::Answer(response)) => Ok(response),
                            Ok(QuestionnaireReply::Cancelled(
                                QuestionnaireCancelReason::UserCancelled,
                            )) => Err(crate::agent::AgentError::Questionnaire(
                                "questionnaire answer was cancelled".into(),
                            )),
                            Ok(QuestionnaireReply::Cancelled(
                                QuestionnaireCancelReason::UiUnavailable,
                            ))
                            | Err(_) => Err(crate::agent::AgentError::Questionnaire(
                                "questionnaire UI is unavailable".into(),
                            )),
                        }
                    })
                };
            let questionnaire_handler = self
                .info
                .questionnaire_enabled
                .then_some(&mut ask_questionnaire as crate::agent::QuestionnaireHandler<'_>);
            let on_event = move |event: AgentEvent| {
                match &event {
                    AgentEvent::ToolStarted { .. } => {
                        callback_tool_call_active.store(true, Ordering::SeqCst)
                    }
                    AgentEvent::ToolFinished { .. } => {
                        callback_tool_call_active.store(false, Ordering::SeqCst)
                    }
                    AgentEvent::StepStarted(_)
                    | AgentEvent::OutputDelta(_)
                    | AgentEvent::ReasoningDelta(_)
                    | AgentEvent::ContextUsage(_)
                    | AgentEvent::Usage(_)
                    | AgentEvent::ToolUpdated { .. }
                    | AgentEvent::ToolCallUpdated { .. }
                    | AgentEvent::QuestionnaireStarted(_)
                    | AgentEvent::QuestionnaireFinished(_) => {}
                }
                let _ = event_tx.send(event);
                if callback_interrupt_requested.load(Ordering::SeqCst) {
                    return Err(crate::model::ModelError::Interrupted);
                }
                Ok(())
            };
            let mut run_future = Box::pin(agent.run_turn_events_questionnaire_and_steering(
                user_content,
                on_event,
                questionnaire_handler,
                run_cancellation,
                move || run_interrupt_requested.load(Ordering::SeqCst),
                move || Ok(run_steering_prompts.lock().unwrap().pop_front()),
            ));
            loop {
                tokio::select! {
                    result = &mut run_future => {
                        let mut result = result;
                        while let Ok(event) = event_rx.try_recv() {
                            if let Err(err) = self.handle_queued_agent_event(event, terminal) {
                                result = Err(crate::agent::AgentError::Provider(err));
                                break;
                            }
                        }
                        terminal.draw(|frame| self.draw(frame))?;
                        break result;
                    }
                    Some(request) = question_rx.recv() => {
                        self.open_questionnaire(request, terminal)?;
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                    Some(event) = event_rx.recv() => {
                        event_batch::handle_batch(event, &mut event_rx, |event| self.handle_queued_agent_event(event, terminal)).map_err(crate::agent::AgentError::Provider)?;
                        match self.handle_running_terminal_events(
                            terminal,
                            &interrupt_requested,
                            &tool_call_active,
                            RunningInputMode::Turn,
                        ) {
                            Ok(StreamControl::Interrupt) if !tool_call_active.load(Ordering::SeqCst) => {
                                cancellation.cancel();
                            }
                            Ok(StreamControl::Interrupt | StreamControl::Continue | StreamControl::Resize) => {}
                            Err(err) => break Err(crate::agent::AgentError::Provider(err)),
                        }
                        self.drain_steering_prompts_to(&steering_prompts);
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                    _ = tokio::time::sleep_until(self.stream_sleep_deadline()) => {
                        self.drain_stream_preview(terminal)?;
                        match self.handle_running_terminal_events(
                            terminal,
                            &interrupt_requested,
                            &tool_call_active,
                            RunningInputMode::Turn,
                        ) {
                            Ok(StreamControl::Interrupt) if !tool_call_active.load(Ordering::SeqCst) => {
                                cancellation.cancel();
                            }
                            Ok(StreamControl::Interrupt | StreamControl::Continue | StreamControl::Resize) => {}
                            Err(err) => break Err(crate::agent::AgentError::Provider(err)),
                        }
                        self.drain_steering_prompts_to(&steering_prompts);
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                }
            }
        };

        while let Ok(event) = event_rx.try_recv() {
            self.handle_agent_event(event, terminal)?;
        }
        self.active_tool_call = false;
        self.pending_tool_call = None;
        tool_call_active.store(false, Ordering::SeqCst);
        let outcome = match result {
            Ok(answer) => {
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams(terminal)?;
                self.insert_final_answer_suffix(terminal, &answer)?;
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
            Err(crate::agent::AgentError::Provider(crate::model::ModelError::Interrupted)) => {
                self.restore_pending_work_to_input(&steering_prompts);
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams(terminal)?;
                self.insert_entry(&Entry::Notice("model interrupted".into()));
                self.reset_streams();
                self.current_turn_start = None;
                self.status = "interrupted".into();
                TurnOutcome::Interrupted
            }
            Err(crate::agent::AgentError::Questionnaire(message))
                if message == "questionnaire answer was cancelled" =>
            {
                self.running = false;
                self.loading_spinner.stop();
                self.finish_streams(terminal)?;
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
            Err(err) => {
                self.reset_streams();
                self.current_turn_start = None;
                self.running = false;
                self.loading_spinner.stop();
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "error".into();
                TurnOutcome::Failed
            }
        };
        self.apply_pending_model_selection(agent)?;
        self.report_resting_herdr_state().await;
        terminal.draw(|frame| self.draw(frame))?;
        Ok(outcome)
    }
}
