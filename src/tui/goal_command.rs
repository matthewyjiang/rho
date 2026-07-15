use super::*;

impl App {
    pub(super) fn execute_goal_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        if invocation.args.trim().is_empty() {
            self.insert_entry(&Entry::Notice(self.goal_status_message()));
            self.status = if self.goal.is_some() {
                "goal active"
            } else {
                "running"
            }
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
                let reason = goal
                    .last_reason
                    .as_deref()
                    .map(|reason| format!("\nlast evaluation: {reason}"))
                    .unwrap_or_default();
                format!(
                    "goal active: {}\n{} turn(s), {} elapsed{reason}",
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
            self.status = if self.goal.is_some() {
                "goal active"
            } else {
                "ready"
            }
            .into();
            return Ok(());
        }
        if is_goal_clear_alias(condition) {
            self.clear_goal();
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
        if matches!(outcome, TurnOutcome::Completed) {
            self.continue_goal(terminal, agent).await?;
        }
        Ok(())
    }

    pub(super) async fn continue_goal(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        while !self.should_quit && self.goal.is_some() {
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
                        _ = tokio::time::sleep(LoadingSpinner::FRAME_INTERVAL) => {
                            let control = self.handle_running_terminal_events(
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
                    }
                }
            };
            self.loading_spinner.stop();
            let Some(evaluation) = evaluation else {
                break;
            };

            let evaluation = match evaluation {
                Ok(evaluation) => evaluation,
                Err(err) => {
                    self.insert_entry(&Entry::Error(format!(
                        "goal evaluation failed; goal remains active: {err}"
                    )));
                    self.status = "goal evaluation failed".into();
                    break;
                }
            };
            let Some(goal) = self.goal.as_mut() else {
                break;
            };
            goal.turns += 1;
            goal.last_reason = Some(evaluation.reason.clone());
            if evaluation.met {
                let elapsed = goal::format_elapsed(goal.elapsed());
                let turns = goal.turns;
                self.goal = None;
                self.insert_entry(&Entry::Notice(format!(
                    "goal achieved after {turns} turn(s) and {elapsed}: {}",
                    evaluation.reason
                )));
                self.status = "goal achieved".into();
                break;
            }

            self.insert_entry(&Entry::Notice(format!(
                "goal not yet met: {}",
                evaluation.reason
            )));
            let continuation = format!(
                "Continue working toward this goal:\n\n{condition}\n\nThe goal evaluator says it is not yet met: {}\n\nMake concrete progress and verify the completion condition before stopping.",
                evaluation.reason
            );
            let outcome = self
                .run_prompt_turn(
                    TurnPrompt::standard(continuation, "continuing active goal".into()),
                    Vec::new(),
                    terminal,
                    agent,
                )
                .await?;
            if !matches!(outcome, TurnOutcome::Completed) {
                break;
            }
        }
        self.report_resting_herdr_state().await;
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }
}

fn initial_goal_prompt(condition: &str) -> String {
    format!(
        "The user invoked Rho's `/goal` command to set the following completion goal. Treat this as a goal-setting action, not as an ordinary conversational message or a claim that the goal is already complete.\n\nGoal:\n{condition}\n\nBegin working toward the goal now. Make concrete progress, use tools as needed, and verify the completion condition before stopping."
    )
}

pub(super) fn is_goal_clear_alias(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "clear" | "stop" | "off" | "reset" | "none" | "cancel"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::tests::test_app;

    #[test]
    fn initial_prompt_identifies_goal_setting_action() {
        assert_eq!(
            initial_goal_prompt("all tests pass"),
            "The user invoked Rho's `/goal` command to set the following completion goal. Treat this as a goal-setting action, not as an ordinary conversational message or a claim that the goal is already complete.\n\nGoal:\nall tests pass\n\nBegin working toward the goal now. Make concrete progress, use tools as needed, and verify the completion condition before stopping."
        );
    }

    #[test]
    fn goal_turn_preserves_command_for_display_history_and_persistence() {
        let turn = TurnPrompt::command(
            initial_goal_prompt("all tests pass"),
            "/goal all tests pass".into(),
        );

        assert_eq!(turn.display, "/goal all tests pass");
        assert_eq!(turn.history, "/goal all tests pass");
        assert_eq!(
            turn.persisted_display.as_deref(),
            Some("/goal all tests pass")
        );
        assert!(turn
            .model
            .starts_with("The user invoked Rho's `/goal` command"));
    }

    #[test]
    fn clear_aliases_are_case_insensitive() {
        for alias in ["clear", "STOP", "off", "reset", "none", "cancel"] {
            assert!(is_goal_clear_alias(alias), "{alias}");
        }
        assert!(!is_goal_clear_alias("finish the work"));
    }

    #[test]
    fn clearing_goal_removes_active_indicator() {
        let mut app = test_app();
        app.goal = Some(GoalState::new("tests pass".into()));

        app.clear_goal();

        assert!(app.goal.is_none());
        assert_eq!(app.status, "goal cleared");
        assert!(matches!(
            app.transcript.last(),
            Some(Entry::Notice(message)) if message == "goal cleared"
        ));
    }

    #[test]
    fn status_reports_condition_and_progress() {
        let mut app = test_app();
        let mut goal = GoalState::new("tests pass".into());
        goal.turns = 3;
        goal.last_reason = Some("one test still fails".into());
        app.goal = Some(goal);

        let status = app.goal_status_message();

        assert!(status.contains("goal active: tests pass"), "{status}");
        assert!(status.contains("3 turn(s)"), "{status}");
        assert!(
            status.contains("last evaluation: one test still fails"),
            "{status}"
        );
    }
}
