//! A loaded session's goal controller. One daemon turn owns the work loop,
//! including judging and retry waits, so disconnecting a client changes nothing.
//! The runtime's boundary delivery already waits for children and services their
//! parent-action requests. Goal changes bypass the runtime lock; clear disarms
//! continuation without cancelling a model run already in progress.

use std::sync::{Arc, MutexGuard, PoisonError};

use rho_protocol::{self as wire, ErrorCode, RpcError};
use rho_sdk::{CancellationToken, UserInput};

use super::{
    open::OpenWith,
    turn::{self, DaemonView, Ran},
    Entered, LoadedSession, Sessions, TurnTask,
};
use crate::{
    app::session_runtime::{SessionRuntime, Turn, TurnStart},
    goal::{
        blocked_goal_resumption_prompt, format_human_steps, initial_goal_prompt, GoalDisposition,
        GoalEvaluation, GoalState, EVALUATION_TIMEOUT, GOAL_RETRY_DELAY, MAX_GOAL_CHARS,
    },
};

#[derive(Default)]
pub(super) struct GoalControl {
    state: Option<GoalState>,
    cancel: CancellationToken,
    /// Published after the transcript cut, so a reset also sees the outcome.
    terminal_notice: Option<String>,
}

impl GoalControl {
    fn publish(&self, session: &LoadedSession) {
        session.feed.update_goal(self.state.as_ref().map(|goal| {
            Box::new(wire::SessionGoal {
                condition: goal.condition.clone(),
                phase: if goal.is_blocked() {
                    wire::SessionGoalPhase::Blocked
                } else {
                    wire::SessionGoalPhase::Active
                },
                evaluated_turns: goal.turns,
                set_at: goal.set_at_unix_secs(),
                last_reason: goal.last_reason.clone(),
                human_steps: goal
                    .pending_steps()
                    .iter()
                    .map(|step| wire::SessionGoalStep {
                        action: step.action.clone(),
                        reason: step.reason.clone(),
                    })
                    .collect(),
            })
        }));
    }
}

impl LoadedSession {
    fn goal_control(&self) -> MutexGuard<'_, GoalControl> {
        self.goal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn has_goal(&self) -> bool {
        self.goal_control().state.is_some()
    }

    /// Cancel disarms active goals, but interrupted blocked verification must
    /// retain the human-only steps for a later resume.
    pub(super) fn cancel_goal(&self) {
        let mut goal = self.goal_control();
        goal.cancel.cancel();
        if let Some(state) = &mut goal.state {
            if state.is_blocked() {
                state.interrupt_verification();
            } else {
                goal.state = None;
            }
            goal.publish(self);
        }
    }

    pub(super) fn report_goal_outcome(&self) {
        if let Some(text) = self.goal_control().terminal_notice.take() {
            notice(self, text);
        }
    }
}

impl Sessions {
    /// Goal actions are session control, not queued runtime settings. Set and
    /// resume reserve a daemon turn before answering; clear never waits for it.
    pub(super) async fn configure_goal(
        &self,
        session_id: wire::SessionId,
        action: wire::SessionGoalAction,
    ) -> Result<wire::SessionConfigureResponse, RpcError> {
        let session = self
            .loaded_session(&session_id)
            .await?
            .ok_or_else(not_loaded)?;
        let start = {
            let mut slot = session.enter().ok_or_else(not_loaded)?;
            let mut goal = session.goal_control();
            let (model, display, replacement) = match action {
                wire::SessionGoalAction::Clear => {
                    goal.cancel.cancel();
                    let had = goal.state.take().is_some();
                    goal.terminal_notice = None;
                    goal.publish(&session);
                    let text = if had {
                        "goal cleared"
                    } else {
                        "no active goal"
                    };
                    notice(&session, text.into());
                    return Ok(wire::SessionConfigureResponse {
                        options: session.feed.options(),
                    });
                }
                wire::SessionGoalAction::Set { condition } => {
                    let condition = condition.trim();
                    let asked = condition.chars().count();
                    if asked == 0 || asked > MAX_GOAL_CHARS {
                        return Err(RpcError::new(ErrorCode::InvalidParams, format!(
                            "goal condition length must be 1..={MAX_GOAL_CHARS} characters; asked {asked}"
                        )));
                    }
                    (
                        initial_goal_prompt(condition),
                        format!("/goal {condition}"),
                        Some(GoalState::new(condition.to_owned())),
                    )
                }
                wire::SessionGoalAction::Resume => {
                    let state = goal.state.as_ref().ok_or_else(|| {
                        RpcError::new(ErrorCode::InvalidParams, "no active goal to resume")
                    })?;
                    let model = if state.is_blocked() {
                        blocked_goal_resumption_prompt(
                            &state.condition,
                            state.pending_steps(),
                            None,
                        )
                    } else {
                        format!("Continue working toward this goal:\n\n{}", state.condition)
                    };
                    (model, "/goal resume".into(), None)
                }
                _ => {
                    return Err(RpcError::new(
                        ErrorCode::InvalidParams,
                        "unknown goal action",
                    ))
                }
            };
            // Unlike ordinary prompts, goal actions do not cancel a wake wait.
            // Reserve the turn before mutating the goal, so a busy refusal
            // leaves the existing goal and verification untouched.
            if matches!(
                self.jobs.session_job(&session.id),
                super::SessionJob::Busy(_)
            ) {
                return Err(RpcError::new(
                    ErrorCode::SessionBusy,
                    "session already has a turn in progress",
                ));
            }
            let with = OpenWith {
                // The environment the last prompt brought; configure brings none.
                environment: session.environment.current(),
                setup: wire::SessionSetup::default(),
            };
            let Entered::Started(cancel, runtime) = self.start_turn(&session, &mut slot, &with)?
            else {
                unreachable!("an idle session never queues behind a wake wait");
            };
            goal.cancel = CancellationToken::new();
            if let Some(state) = replacement {
                goal.state = Some(state);
            } else if let Some(state) = &mut goal.state {
                state.begin_verification();
            }
            goal.publish(&session);
            (
                cancel,
                runtime,
                Turn {
                    model: UserInput::text(model),
                    display: UserInput::text(display),
                    calls: Vec::new(),
                },
            )
        };
        let (cancel, runtime, input) = start;
        let _started = TurnTask {
            session: Arc::clone(&session),
            runtime,
            stopping: self.stopping.clone(),
            cancel,
            limit: None,
        }
        .spawn_goal(input);
        Ok(wire::SessionConfigureResponse {
            options: session.feed.options(),
        })
    }
}

fn not_loaded() -> RpcError {
    RpcError::new(
        ErrorCode::SessionNotLoaded,
        "the daemon has not loaded the session; attach to it first",
    )
}

/// A user message resumes blocked verification without replacing the shown
/// prompt with host instructions. Attach/replay uses the ordinary display rows.
pub(super) fn prepare_turn(session: &LoadedSession, turn: &mut Turn) {
    let mut goal = session.goal_control();
    let Some(state) = &mut goal.state else { return };
    if state.begin_verification() {
        // The instructions take the message's text; attachments stay.
        let mut blocks = turn.model.blocks().to_vec();
        let message = crate::app::session_commands::leading_text(&turn.model).map(str::to_owned);
        let prompt = rho_sdk::model::ContentBlock::Text(blocked_goal_resumption_prompt(
            &state.condition,
            state.pending_steps(),
            message.as_deref(),
        ));
        if message.is_some() {
            blocks[0] = prompt;
        } else {
            blocks.insert(0, prompt);
        }
        turn.model = UserInput::content(blocks).expect("a text block keeps the input valid");
        goal.cancel = CancellationToken::new();
        goal.publish(session);
    }
}

/// Continue the goal after a prompt, retrying failed history rather than
/// duplicating its prompt or dropping child notices it already consumed.
pub(super) async fn drive(
    runtime: &mut SessionRuntime,
    view: &mut DaemonView,
    mut ended: anyhow::Result<Ran>,
    turn_cancel: &CancellationToken,
) -> anyhow::Result<Ran> {
    let session = Arc::clone(&view.session);
    loop {
        let current = {
            let mut goal = session.goal_control();
            if let Some(state) = &mut goal.state {
                if matches!(ended, Ok(Ran::Cancelled)) || turn_cancel.is_cancelled() {
                    state.interrupt_verification();
                } else {
                    state.complete_verification();
                }
            }
            goal.publish(&session);
            goal.state
                .as_ref()
                .filter(|state| !state.is_blocked())
                .map(|state| (state.condition.clone(), goal.cancel.clone()))
        };
        let Some((condition, cancel)) = current else {
            return ended;
        };
        if turn_cancel.is_cancelled() || matches!(ended, Ok(Ran::Cancelled)) {
            session.cancel_goal();
            return ended;
        }
        if cancel.is_cancelled() {
            return ended;
        }
        let next = if let Err(error) = &ended {
            notice(
                &session,
                format!(
                    "could not complete goal turn: {}; retrying while goal remains active",
                    turn::client_error(runtime, error).message
                ),
            );
            if !retry_wait(&cancel, turn_cancel).await {
                return ended;
            }
            TurnStart::Continue
        } else {
            notice(&session, "evaluating goal".into());
            let evaluation_cancel = CancellationToken::new();
            let evaluation =
                runtime.evaluate_goal(&condition, &session.config, evaluation_cancel.clone());
            tokio::pin!(evaluation);
            let judged = tokio::select! {
                biased;
                () = cancel.cancelled() => Judged::Stopped,
                () = turn_cancel.cancelled() => Judged::Stopped,
                result = &mut evaluation => Judged::Done(result),
                () = tokio::time::sleep(EVALUATION_TIMEOUT) => Judged::TimedOut,
            };
            let result = match judged {
                Judged::Done(result) => result,
                // Drain cancellation, just as the in-process evaluator does, so
                // no judge request keeps running after clear, cancel, or timeout.
                unfinished => {
                    evaluation_cancel.cancel();
                    let _ = evaluation.await;
                    match unfinished {
                        Judged::TimedOut => Err(anyhow::anyhow!("goal evaluation timed out")),
                        Judged::Stopped | Judged::Done(_) => return ended,
                    }
                }
            };
            let evaluation = match result {
                Ok(evaluation) => evaluation,
                Err(error) => {
                    notice(
                        &session,
                        format!(
                            "could not evaluate goal: {error}; retrying while goal remains active"
                        ),
                    );
                    if !retry_wait(&cancel, turn_cancel).await {
                        return ended;
                    }
                    continue;
                }
            };
            {
                let mut goal = session.goal_control();
                // A clear/cancel won while the evaluator was finishing.
                if cancel.is_cancelled() {
                    return ended;
                }
                let Some(state) = &mut goal.state else {
                    return ended;
                };
                match state.record_evaluation(&evaluation) {
                    GoalDisposition::Complete => {
                        goal.terminal_notice = Some(format!(
                            "goal achieved after {} turn(s) and {}: {}",
                            state.turns,
                            crate::subagent::format_elapsed_secs(state.elapsed().as_secs()),
                            evaluation.reason()
                        ));
                        goal.state = None;
                        goal.publish(&session);
                        return ended;
                    }
                    GoalDisposition::Pause => {
                        goal.terminal_notice = Some(format!("goal blocked: remaining steps need you\n{}\nremaining steps:\n{}\nuse /goal resume or send a message after completing them.",
                            evaluation.reason(), format_human_steps(evaluation.pending_steps())));
                        goal.publish(&session);
                        return ended;
                    }
                    GoalDisposition::Continue => goal.publish(&session),
                }
            }
            notice(
                &session,
                format!("goal not yet met: {}", evaluation.reason()),
            );
            let model = format!("Continue working toward this goal:\n\n{condition}\n\nThe goal evaluator says it is not yet met: {}\n\nMake concrete progress and verify the completion condition before stopping.", evaluation.reason());
            TurnStart::Prompt(Turn {
                model: UserInput::text(model),
                display: UserInput::text("continuing active goal"),
                calls: Vec::new(),
            })
        };
        // Clear only disarms the next turn; it never cancels a run in progress.
        if cancel.is_cancelled() || turn_cancel.is_cancelled() {
            return ended;
        }
        ended = turn::run_goal_turn(runtime, view, next, &session.config, turn_cancel).await;
    }
}

/// How waiting for the goal's evaluation ended.
enum Judged {
    Done(anyhow::Result<GoalEvaluation>),
    /// The goal was cleared or the turn cancelled.
    Stopped,
    TimedOut,
}

async fn retry_wait(cancel: &CancellationToken, turn_cancel: &CancellationToken) -> bool {
    tokio::select! {
        biased;
        () = cancel.cancelled() => false,
        () = turn_cancel.cancelled() => false,
        () = tokio::time::sleep(GOAL_RETRY_DELAY) => true,
    }
}

fn notice(session: &LoadedSession, text: String) {
    session.feed.publish([wire::SessionEvent::Notice { text }]);
}
