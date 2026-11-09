//! Daemon-owned goal snapshots and lifecycle actions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A goal the loaded session keeps working on, independently of clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionGoal {
    /// The user's completion condition.
    pub condition: String,
    /// Whether the goal is working or waiting for the user.
    pub phase: SessionGoalPhase,
    /// Evaluations completed, not provider calls or SDK steps.
    pub evaluated_turns: usize,
    /// Unix time, in seconds, when the goal was set. Clients count the
    /// goal's elapsed time from it.
    pub set_at: u64,
    /// Why the last evaluation found the goal unmet or blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reason: Option<String>,
    /// Remaining human-only work; empty while active.
    pub human_steps: Vec<SessionGoalStep>,
}

/// Whether the goal is working or waiting for external human-only steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionGoalPhase {
    Active,
    /// Resume or a new user prompt verifies the previously blocked steps.
    Blocked,
    /// A phase a newer daemon sends that this client does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One step the evaluator found outside the session's authority or capabilities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionGoalStep {
    /// What the user has to do.
    pub action: String,
    /// Why the session cannot do it itself.
    pub reason: String,
}

/// A goal lifecycle change, sent through `rho/session/configure`. Unlike
/// model settings, these actions do not queue behind a running goal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionGoalAction {
    /// Set a nonempty condition and start working. Refused while a turn runs.
    Set { condition: String },
    /// Disarm the loop immediately, cancelling evaluation or retry waits,
    /// but not a model turn already running. Idempotent when no goal exists.
    Clear,
    /// Verify a blocked goal's human steps, or continue an active goal.
    /// Refused while a turn runs or when there is no goal.
    Resume,
}
