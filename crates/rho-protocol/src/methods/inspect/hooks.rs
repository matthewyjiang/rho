//! [`super::SessionReport::Hooks`]: the session's hooks, read again from
//! their files, and what they did recently.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The session's hooks and their recent activity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionHooksReport {
    /// Whether the session runs hooks: false when it started without them,
    /// because none were configured, they failed to load, or a hook started
    /// the session. Its report still lists an untrusted project file.
    pub enabled: bool,
    /// The hooks files the session loaded, in load order: the user's, then
    /// the project's.
    pub files: Vec<PathBuf>,
    /// The project's hooks file, when the session skipped it because it
    /// does not trust the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_untrusted: Option<SkippedHooksFile>,
    /// Every hook the files define: the active ones in the order they run,
    /// the user's file then the project's, each in file order; then those of
    /// the skipped file.
    pub hooks: Vec<HookContract>,
    /// The latest hook invocations, oldest first. rho keeps a bounded
    /// number of them.
    pub recent_activity: Vec<HookActivityRecord>,
}

/// A hooks file the session did not load because it does not trust the
/// project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkippedHooksFile {
    pub path: PathBuf,
    /// Why the file could not be read or parsed, when it could not. Its
    /// hooks are then not listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What one hook executes, shown so a person can judge it before trusting
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookContract {
    /// The hook's id, qualified by its file, such as `project:fmt`.
    pub id: String,
    pub status: HookStatus,
    /// The event that runs it.
    pub event: HookEvent,
    /// The tools whose events run it; only tool events use it.
    pub tools: HookTools,
    /// The program and its arguments, resolved.
    pub command: Vec<String>,
    pub working_directory: PathBuf,
    /// How long it may run before rho stops it, in milliseconds.
    pub timeout_ms: u64,
    /// The names of the environment variables the program receives.
    pub environment: Vec<String>,
    /// Problems found at load that do not stop it from running.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<HookWarning>,
}

/// Whether a hook runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookStatus {
    /// It runs on its event.
    Active,
    /// It belongs to a project file the session does not trust, so it does
    /// not run.
    Untrusted,
    /// Its file loads, but another hooks file a session would load is
    /// invalid, so a session runs no hooks at all. Only
    /// [`crate::HOOKS_LIST`] sends it.
    Disabled,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// An event a hook runs on, by the name a hooks file gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookEvent {
    SessionStarted,
    /// A tool call is about to be authorized. The only event whose hooks
    /// can stop what follows.
    BeforeToolUse,
    AfterToolUse,
    RunCompleted,
    RunFailed,
    /// A run waits for a person: an approval, a plan approval, or a
    /// question.
    InputRequested,
    SessionCompleted,
    SessionFailed,
    WorkflowStarted,
    WorkflowNodeStarted,
    WorkflowNodeFinished,
    WorkflowCompleted,
    WorkflowFailed,
    WorkflowCancelled,
    /// An event this crate does not know, including one the daemon's own
    /// rho does not name.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The tools whose events run a hook.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookTools {
    /// Every tool: the hooks file names none.
    All,
    /// The tools the hooks file names, in its order. Each pattern is a tool
    /// name, or a name prefix ending in one `*`.
    Matching { patterns: Vec<String> },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A problem with a hook that does not stop it from running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookWarning {
    /// It names some edit tools but not these. A session registers one edit
    /// tool, chosen by its provider and configuration, so the hook stops
    /// running when that choice changes.
    MissingEditTools { tools: Vec<String> },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One recorded hook invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookActivityRecord {
    /// The id of the hook that ran; absent when none did, as for an event
    /// the queue of observing hooks dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<String>,
    pub event: HookEvent,
    pub outcome: HookOutcome,
    /// How long the hook ran, in milliseconds, when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Whether rho cut the hook's output short.
    pub output_truncated: bool,
}

/// What one hook invocation resulted in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookOutcome {
    /// A blocking hook let the action go ahead.
    Continued,
    /// A blocking hook stopped the action.
    Denied { reason: String },
    /// An observing hook ran.
    Observed,
    /// The hook itself failed, or the event could not be delivered to the
    /// observing hooks (then no hook is named). A blocking hook's failure
    /// denies the action.
    Failed { reason: String },
    /// The queue of observing hooks was full, so the event was dropped.
    Dropped,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
