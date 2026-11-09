//! `/goal` in a daemon session (#1667). The daemon owns the goal's loop, so
//! it keeps working while no TUI is open; the TUI shows the session's goal
//! from its options and sends the user's changes.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rho_protocol::{SessionGoal, SessionGoalAction, SessionGoalPhase, SessionGoalStep};

use super::{
    goal::format_elapsed,
    goal_command::{is_goal_clear_alias, is_goal_resume_alias},
    statusline::GoalStatus,
    App, Entry,
};

impl App {
    /// The followed session's goal, as the daemon last reported it.
    pub(super) fn daemon_goal(&self) -> Option<&SessionGoal> {
        self.daemon_options.as_ref()?.goal.as_deref()
    }

    /// `/goal [condition | clear | resume]`: no arguments show the goal, the
    /// rest go to the daemon, which refuses a set or resume during a turn.
    pub(super) fn execute_daemon_goal_command(&mut self, args: &str) {
        let args = args.trim();
        let action = if args.is_empty() {
            // Multi-line status belongs in history for scroll-back.
            self.insert_entry(&Entry::Notice(goal_status_text(self.daemon_goal())));
            return;
        } else if is_goal_clear_alias(args) {
            SessionGoalAction::Clear
        } else if is_goal_resume_alias(args) {
            SessionGoalAction::Resume
        } else {
            SessionGoalAction::Set {
                condition: args.to_owned(),
            }
        };
        let session = self
            .info
            .session
            .daemon
            .as_ref()
            .expect("a daemon goal command requires a session");
        if let Err(error) = session.goal(action) {
            self.insert_entry(&Entry::Error(format!("could not change the goal: {error}")));
        }
    }
}

/// What the status line shows of a daemon goal.
pub(super) fn daemon_goal_status(goal: &SessionGoal) -> GoalStatus {
    GoalStatus {
        turns: goal.evaluated_turns,
        elapsed: elapsed_since(goal.set_at),
        blocked: goal.phase == SessionGoalPhase::Blocked,
    }
}

/// `/goal` with no arguments, as the in-process goal says it.
fn goal_status_text(goal: Option<&SessionGoal>) -> String {
    let Some(goal) = goal else {
        return "no active goal. use /goal <condition> to start one.".into();
    };
    let blocked = goal.phase == SessionGoalPhase::Blocked;
    let state = if blocked {
        "goal blocked"
    } else {
        "goal active"
    };
    let reason = goal
        .last_reason
        .as_deref()
        .map(|reason| format!("\nlast evaluation: {reason}"))
        .unwrap_or_default();
    let pending_steps = if blocked {
        format!(
            "\nremaining steps:\n{}\nuse /goal resume after completing them.",
            human_steps(&goal.human_steps)
        )
    } else {
        String::new()
    };
    format!(
        "{state}: {}\n{} turn(s), {} elapsed{reason}{pending_steps}",
        goal.condition,
        goal.evaluated_turns,
        format_elapsed(elapsed_since(goal.set_at))
    )
}

fn human_steps(steps: &[SessionGoalStep]) -> String {
    steps
        .iter()
        .map(|step| format!("- {}: {}", step.action, step.reason))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Time since Unix second `set_at`; the daemon runs on this machine, so the
/// clocks agree.
fn elapsed_since(set_at: u64) -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH + Duration::from_secs(set_at))
        .unwrap_or_default()
}
