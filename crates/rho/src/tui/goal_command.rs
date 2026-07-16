use super::*;

const GOAL_RETRY_DELAY: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GoalLoopAction {
    /// Turn finished normally; re-enter the evaluate/work cycle.
    Continue,
    /// Transient failure; wait, then keep working while the goal remains active.
    RetryAfterFailure,
    /// User interrupt/cancel or other terminal stop; leave the goal loop.
    Stop,
}

pub(super) fn should_resume_goal_after_turn(
    outcome: TurnOutcomeKind,
    goal_state: Option<goal::GoalLoopState>,
    should_quit: bool,
) -> bool {
    matches!(goal_state, Some(goal::GoalLoopState::Active))
        && !should_quit
        && matches!(
            outcome,
            TurnOutcomeKind::Completed | TurnOutcomeKind::Failed
        )
}

pub(super) fn should_drain_queued_prompts(outcome: TurnOutcomeKind, resume_goal: bool) -> bool {
    matches!(outcome, TurnOutcomeKind::Completed) || resume_goal
}

fn goal_loop_action_after_turn(outcome: TurnOutcomeKind) -> GoalLoopAction {
    match outcome {
        TurnOutcomeKind::Completed => GoalLoopAction::Continue,
        TurnOutcomeKind::Failed => GoalLoopAction::RetryAfterFailure,
        TurnOutcomeKind::Interrupted | TurnOutcomeKind::Cancelled => GoalLoopAction::Stop,
    }
}

impl App {
    pub(super) fn execute_goal_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        if invocation.args.trim().is_empty() {
            self.insert_entry(&Entry::Notice(self.goal_status_message()));
            self.status = self
                .goal
                .as_ref()
                .map(|goal| {
                    if goal.is_blocked() {
                        "goal blocked"
                    } else {
                        "goal active"
                    }
                })
                .unwrap_or("running")
                .into();
        } else if is_goal_clear_alias(&invocation.args) {
            self.clear_goal();
        } else {
            self.insert_entry(&Entry::Notice(
                "/goal can only be inspected or cleared while a model turn is running".into(),
            ));
            self.status = "goal command unavailable while running".into();
        }
        Ok(())
    }

    pub(super) fn goal_status_message(&self) -> String {
        match &self.goal {
            Some(goal) => {
                let state = if goal.is_blocked() {
                    "goal blocked"
                } else {
                    "goal active"
                };
                let reason = goal
                    .last_reason
                    .as_deref()
                    .map(|reason| format!("\nlast evaluation: {reason}"))
                    .unwrap_or_default();
                let pending_steps = if goal.is_blocked() {
                    format!(
                        "\nremaining steps:\n{}\nuse /goal resume after completing them.",
                        format_human_steps(goal.pending_steps())
                    )
                } else {
                    String::new()
                };
                format!(
                    "{state}: {}\n{} turn(s), {} elapsed{reason}{pending_steps}",
                    goal.condition,
                    goal.turns,
                    goal::format_elapsed(goal.elapsed())
                )
            }
            None => "no active goal. use /goal <condition> to start one.".into(),
        }
    }

    pub(super) fn clear_goal(&mut self) {
        if self.goal.take().is_some() {
            self.insert_entry(&Entry::Notice("goal cleared".into()));
            self.status = if self.running {
                "running"
            } else {
                "goal cleared"
            }
            .into();
        } else {
            self.insert_entry(&Entry::Notice("no active goal".into()));
            self.status = if self.running { "running" } else { "ready" }.into();
        }
    }

    pub(super) async fn execute_goal_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let condition = invocation.args.trim();
        if condition.is_empty() {
            self.insert_entry(&Entry::Notice(self.goal_status_message()));
            self.status = self
                .goal
                .as_ref()
                .map(|goal| {
                    if goal.is_blocked() {
                        "goal blocked"
                    } else {
                        "goal active"
                    }
                })
                .unwrap_or("ready")
                .into();
            return Ok(());
        }
        if is_goal_clear_alias(condition) {
            self.clear_goal();
            return Ok(());
        }
        if is_goal_resume_alias(condition) {
            self.resume_goal(terminal, agent).await?;
            return Ok(());
        }
        if condition.chars().count() > goal::MAX_GOAL_CHARS {
            self.insert_entry(&Entry::Error(format!(
                "goal conditions cannot exceed {} characters",
                goal::MAX_GOAL_CHARS
            )));
            self.status = "goal not set".into();
            return Ok(());
        }

        self.goal = Some(GoalState::new(condition.to_string()));
        self.insert_entry(&Entry::Notice(format!(
            "goal set: {condition}\nrho will keep working until the goal is met. use /goal clear to cancel."
        )));
        self.status = "goal active".into();
        let images = std::mem::take(&mut self.pending_images);
        let outcome = self
            .run_prompt_turn(
                TurnPrompt::command(initial_goal_prompt(condition), format!("/goal {condition}")),
                images,
                terminal,
                agent,
            )
            .await?;
        let outcome_kind = outcome.kind();
        let pending_retries = match outcome {
            TurnOutcome::Failed(failed_turn) => VecDeque::from([failed_turn]),
            TurnOutcome::Completed | TurnOutcome::Interrupted | TurnOutcome::Cancelled => {
                VecDeque::new()
            }
        };
        if should_resume_goal_after_turn(
            outcome_kind,
            self.goal.as_ref().map(GoalState::loop_state),
            self.should_quit,
        ) {
            self.continue_goal(terminal, agent, pending_retries).await?;
        }
        Ok(())
    }

    async fn resume_goal(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(goal) = self.goal.as_mut() else {
            self.insert_entry(&Entry::Notice("no active goal to resume".into()));
            self.status = "ready".into();
            return Ok(());
        };

        if goal.is_blocked() {
            let condition = goal.condition.clone();
            let pending_steps = goal.pending_steps().to_vec();
            goal.begin_verification();
            let prompt = blocked_goal_resumption_prompt(&condition, &pending_steps, None);
            self.insert_entry(&Entry::Notice(
                "goal resumed; verifying the previously blocked steps".into(),
            ));
            self.status = "goal active".into();
            let outcome = self
                .run_prompt_turn(
                    TurnPrompt::command(prompt, "/goal resume".into()),
                    Vec::new(),
                    terminal,
                    agent,
                )
                .await?;
            let outcome_kind = outcome.kind();
            self.finish_goal_resumption_turn(outcome_kind);
            let pending_retries = match outcome {
                TurnOutcome::Failed(failed_turn) => VecDeque::from([failed_turn]),
                TurnOutcome::Completed | TurnOutcome::Interrupted | TurnOutcome::Cancelled => {
                    VecDeque::new()
                }
            };
            if should_resume_goal_after_turn(
                outcome_kind,
                self.goal.as_ref().map(GoalState::loop_state),
                self.should_quit,
            ) {
                self.continue_goal(terminal, agent, pending_retries).await?;
            }
        } else {
            self.insert_entry(&Entry::Notice("goal is already active".into()));
            self.continue_goal(terminal, agent, VecDeque::new()).await?;
        }
        Ok(())
    }

    pub(super) fn prepare_goal_resumption_turn(
        &mut self,
        prompt: String,
        display_prompt: String,
    ) -> TurnPrompt {
        let Some(goal) = self.goal.as_mut() else {
            return TurnPrompt::standard(prompt, display_prompt);
        };
        if !goal.is_blocked() {
            return TurnPrompt::standard(prompt, display_prompt);
        }

        let condition = goal.condition.clone();
        let pending_steps = goal.pending_steps().to_vec();
        goal.begin_verification();
        self.insert_entry(&Entry::Notice(
            "goal resumed by user message; verifying the previously blocked steps".into(),
        ));
        self.status = "goal active".into();
        TurnPrompt {
            model: blocked_goal_resumption_prompt(&condition, &pending_steps, Some(&prompt)),
            history: prompt,
            display: display_prompt.clone(),
            persisted_display: Some(display_prompt),
        }
    }

    pub(super) fn finish_goal_resumption_turn(&mut self, outcome: TurnOutcomeKind) {
        let Some(goal) = self.goal.as_mut() else {
            return;
        };
        match outcome {
            TurnOutcomeKind::Completed | TurnOutcomeKind::Failed => {
                goal.complete_verification();
            }
            TurnOutcomeKind::Interrupted | TurnOutcomeKind::Cancelled => {
                goal.interrupt_verification();
            }
        }
    }

    pub(super) async fn continue_goal(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
        mut pending_retries: VecDeque<FailedTurn>,
    ) -> anyhow::Result<()> {
        while !self.should_quit && self.goal.as_ref().is_some_and(|goal| !goal.is_blocked()) {
            while let Some(failed_turn) = pending_retries.pop_front() {
                if !self
                    .retry_failed_goal_turn(failed_turn, terminal, agent)
                    .await?
                {
                    self.report_resting_herdr_state().await;
                    terminal.draw(|frame| self.draw(frame))?;
                    return Ok(());
                }
            }
            self.status = "evaluating goal".into();
            self.loading_spinner.start();
            terminal.draw(|frame| self.draw(frame))?;

            let (condition, provider, model) = {
                let goal = self.goal.as_ref().expect("goal checked above");
                (
                    goal.condition.clone(),
                    self.info
                        .title_provider
                        .clone()
                        .unwrap_or_else(|| self.info.provider.clone()),
                    self.info
                        .title_model
                        .clone()
                        .unwrap_or_else(|| self.info.model.clone()),
                )
            };
            let history = agent.history();
            let evaluation = {
                let interrupt_requested = AtomicBool::new(false);
                let tool_call_active = AtomicBool::new(false);
                let mut evaluation =
                    Box::pin(goal::evaluate(&provider, &model, &condition, &history));
                let deadline = tokio::time::Instant::now() + goal::EVALUATION_TIMEOUT;
                loop {
                    tokio::select! {
                        result = &mut evaluation => break Some(result),
                        _ = tokio::time::sleep_until(deadline) => {
                            break Some(Err(anyhow::anyhow!("goal evaluation timed out")));
                        }
                        terminal_event = self.terminal_events.as_mut().expect("terminal events initialized").next() => {
                            let control = self.handle_running_terminal_events(
                                terminal_event?,
                                terminal,
                                &interrupt_requested,
                                &tool_call_active,
                                RunningInputMode::Turn,
                            )?;
                            if matches!(control, StreamControl::Interrupt) {
                                self.clear_goal();
                                break None;
                            }
                            if self.goal.is_none() {
                                break None;
                            }
                            terminal.draw(|frame| self.draw(frame))?;
                        }
                        _ = tokio::time::sleep(LoadingSpinner::FRAME_INTERVAL) => {
                            self.flush_due_paste_burst();
                            terminal.draw(|frame| self.draw(frame))?;
                        }
                    }
                    if self.finish_completed_inline_shells().await? {
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                }
            };
            self.finish_all_inline_shells().await?;
            self.insert_deferred_inline_shell_context(agent)?;
            self.loading_spinner.stop();
            let Some(evaluation) = evaluation else {
                break;
            };

            let evaluation = match evaluation {
                Ok(evaluation) => evaluation,
                Err(err) => {
                    self.insert_entry(&Entry::Error(format!(
                        "goal evaluation failed; retrying while goal remains active: {err}"
                    )));
                    self.status = "goal retrying".into();
                    if !self.wait_for_goal_retry(terminal, agent).await? {
                        break;
                    }
                    continue;
                }
            };
            let Some(goal) = self.goal.as_mut() else {
                break;
            };
            let disposition = goal.record_evaluation(&evaluation);
            match disposition {
                goal::GoalDisposition::Complete => {
                    let elapsed = goal::format_elapsed(goal.elapsed());
                    let turns = goal.turns;
                    self.goal = None;
                    self.insert_entry(&Entry::Notice(format!(
                        "goal achieved after {turns} turn(s) and {elapsed}: {}",
                        evaluation.reason()
                    )));
                    self.status = "goal achieved".into();
                    break;
                }
                goal::GoalDisposition::Pause => {
                    self.insert_entry(&Entry::Notice(format!(
                        "goal blocked: remaining steps need you\n{}\nremaining steps:\n{}\nuse /goal resume or send a message after completing them.",
                        evaluation.reason(),
                        format_human_steps(evaluation.pending_steps())
                    )));
                    self.status = "goal blocked".into();
                    break;
                }
                goal::GoalDisposition::Continue => {}
            }

            self.insert_entry(&Entry::Notice(format!(
                "goal not yet met: {}",
                evaluation.reason()
            )));
            let continuation = format!(
                "Continue working toward this goal:\n\n{condition}\n\nThe goal evaluator says it is not yet met: {}\n\nMake concrete progress and verify the completion condition before stopping.",
                evaluation.reason()
            );
            let outcome = self
                .run_prompt_turn(
                    TurnPrompt::standard(continuation, "continuing active goal".into()),
                    Vec::new(),
                    terminal,
                    agent,
                )
                .await?;
            match outcome {
                TurnOutcome::Completed => {}
                TurnOutcome::Failed(failed_turn) => {
                    if !self
                        .retry_failed_goal_turn(failed_turn, terminal, agent)
                        .await?
                    {
                        break;
                    }
                }
                TurnOutcome::Interrupted | TurnOutcome::Cancelled => break,
            }
        }
        self.report_resting_herdr_state().await;
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }

    async fn retry_failed_goal_turn(
        &mut self,
        mut failed_turn: FailedTurn,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        loop {
            self.insert_entry(&Entry::Notice(
                "goal still active; retrying after the run stopped before the goal was met".into(),
            ));
            self.status = "goal retrying".into();
            if !self.wait_for_goal_retry(terminal, agent).await? {
                return Ok(false);
            }

            let outcome = self
                .retry_failed_prompt_turn(failed_turn, terminal, agent)
                .await?;
            match goal_loop_action_after_turn(outcome.kind()) {
                GoalLoopAction::Continue => return Ok(true),
                GoalLoopAction::RetryAfterFailure => {
                    let TurnOutcome::Failed(next_failed_turn) = outcome else {
                        unreachable!("retry action requires a failed turn")
                    };
                    failed_turn = next_failed_turn;
                }
                GoalLoopAction::Stop => return Ok(false),
            }
        }
    }

    async fn wait_for_goal_retry(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if self.should_quit || self.goal.is_none() {
            return Ok(false);
        }

        self.loading_spinner.start();
        terminal.draw(|frame| self.draw(frame))?;
        let interrupt_requested = AtomicBool::new(false);
        let tool_call_active = AtomicBool::new(false);
        let deadline = tokio::time::Instant::now() + GOAL_RETRY_DELAY;
        let should_retry = loop {
            if self.should_quit || self.goal.is_none() {
                break false;
            }
            if tokio::time::Instant::now() >= deadline {
                break true;
            }
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break true,
                terminal_event = self.terminal_events.as_mut().expect("terminal events initialized").next() => {
                    let control = self.handle_running_terminal_events(
                        terminal_event?,
                        terminal,
                        &interrupt_requested,
                        &tool_call_active,
                        RunningInputMode::Turn,
                    )?;
                    if matches!(control, StreamControl::Interrupt) {
                        self.clear_goal();
                        break false;
                    }
                    if self.goal.is_none() || self.should_quit {
                        break false;
                    }
                    terminal.draw(|frame| self.draw(frame))?;
                }
                _ = tokio::time::sleep(LoadingSpinner::FRAME_INTERVAL) => {
                    self.flush_due_paste_burst();
                    terminal.draw(|frame| self.draw(frame))?;
                }
            }
            if self.finish_completed_inline_shells().await? {
                terminal.draw(|frame| self.draw(frame))?;
            }
        };
        self.finish_all_inline_shells().await?;
        self.insert_deferred_inline_shell_context(agent)?;
        self.loading_spinner.stop();
        Ok(should_retry && self.goal.is_some() && !self.should_quit)
    }
}

fn initial_goal_prompt(condition: &str) -> String {
    format!(
        "The user invoked Rho's `/goal` command to set the following completion goal. Treat this as a goal-setting action, not as an ordinary conversational message or a claim that the goal is already complete.\n\nGoal:\n{condition}\n\nBegin working toward the goal now. Make concrete progress, use tools as needed, and verify the completion condition before stopping."
    )
}

fn blocked_goal_resumption_prompt(
    condition: &str,
    pending_steps: &[goal::HumanStep],
    user_message: Option<&str>,
) -> String {
    let user_message = user_message
        .map(|message| format!("\n\nThe user's new message is:\n{message}"))
        .unwrap_or_default();
    format!(
        "Resume the following goal after it was blocked on steps requiring the user:\n\n{condition}\n\nPreviously blocked steps:\n{}\n\nFirst verify whether each relevant external condition has changed. Do not repeat implementation work unless verification shows that more agent-actionable work is needed.{user_message}",
        format_human_steps(pending_steps)
    )
}

fn format_human_steps(steps: &[goal::HumanStep]) -> String {
    steps
        .iter()
        .map(|step| format!("- {}: {}", step.action, step.reason))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn is_goal_clear_alias(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "clear" | "stop" | "off" | "reset" | "none" | "cancel"
    )
}

fn is_goal_resume_alias(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("resume")
}

#[cfg(test)]
#[path = "goal_command_tests.rs"]
mod tests;
