//! A session's options: the settings a client may change between turns,
//! the choices it offers for each, and the commands a prompt may start
//! with. Frontends render them as the session's model picker, reasoning
//! and fast-mode controls, permission mode, and command menu: ACP's config
//! options and `available_commands_update` (#1670), and the TUI's `/model`,
//! reasoning, `/fast`, and `/permissions` (#1667).
//!
//! [`super::SessionNewResponse`] and [`super::SessionAttachResponse`] carry
//! them, and [`super::SessionEvent::OptionsChanged`] replaces them whenever
//! they change: after [`SESSION_CONFIGURE`], a turn's end, which may leave
//! new commands behind, a plan approval's mode switch, or a credential write
//! that changes the models a session may switch to.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PermissionMode, SessionGoal, SessionGoalAction, SessionId};

/// Make one change to a loaded session between its turns: a change sent
/// during a turn, waiting for input included, waits for the turn to end,
/// then applies (#1685). Changes apply in the order the daemon took them,
/// each before any turn a later prompt starts. The session keeps the change
/// with its file, so it outlasts the daemon unloading the session. A
/// session the daemon has not loaded is
/// [`crate::ErrorCode::SessionNotLoaded`]; one being deleted is
/// [`crate::ErrorCode::SessionBusy`]. A change that fails as it applies is
/// [`crate::ErrorCode::InternalError`].
pub const SESSION_CONFIGURE: &str = "rho/session/configure";

/// Params of [`SESSION_CONFIGURE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfigureParams {
    pub session: SessionId,
    /// One change per request, so an error names the change it refuses.
    pub change: SessionChange,
}

/// One change to a loaded session, applied between its turns: a setting, or
/// an edit to its context or to where its conversation continues from. A setting's value must be one the session's
/// [`SessionOptions`] offers; any other is [`crate::ErrorCode::InvalidParams`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionChange {
    /// Switch to `model`, one of [`ModelOptions::available`]. The session
    /// keeps its reasoning level, or refuses a model that cannot.
    Model {
        model: ModelRef,
        /// The auth profile to run `model` on, as `--auth` names one: a
        /// profile `model`'s provider takes. When absent, the session keeps
        /// its auth if the provider takes it, else picks one that has
        /// credentials.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<String>,
        /// Whether the change sets the session up as it opens, as a launch
        /// flag or a host's setup does, rather than switching its model.
        /// A switch that moves the session to another model tells its
        /// clients which prompt file it now runs on, in a
        /// [`super::SessionEvent::Notice`]; a setup change does not.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        setup: bool,
    },
    /// Spend `level`, one of [`ReasoningOptions::available`], on later turns.
    Reasoning { level: ReasoningLevel },
    /// Turn fast mode on or off. Turning it on needs a model that has it
    /// ([`SessionOptions::fast_mode`] present); off always applies.
    FastMode { enabled: bool },
    /// Gate later turns' tool calls with `mode`.
    PermissionMode { mode: PermissionMode },
    /// Control the loaded session's goal without waiting for its turn.
    Goal { action: SessionGoalAction },
    /// Add `text` to what the model reads from its next turn on, without a
    /// turn of its own, and save it with the session as a user row that
    /// reads `display`. Clients attached now see no event for it: the one
    /// that sent it shows its own notice, and a later reset replays the
    /// row. An empty `text` is [`crate::ErrorCode::InvalidParams`].
    Context { text: String, display: String },
    /// Read the workspace's agent definitions again and tell the model, from
    /// its next turn on, which agents it can now delegate to. A client sends
    /// it after it wrote or removed a definition. Clients see a
    /// [`super::SessionEvent::Notice`] when the model had to be told.
    AgentCatalog,
    /// Let the session's tools reach the directory at `path`, absolute, and
    /// tell the model so from its next turn on, with a saved row. A path
    /// that is not a directory, or one the session already reaches, is
    /// [`crate::ErrorCode::InvalidParams`].
    AddDirectory { path: PathBuf },
    /// Read the model lists again: a client sends it after it refreshed
    /// the lists the daemon shares on disk. Every loaded session's options
    /// follow.
    ModelCatalog,
    /// Continue the conversation from `node`, a node of the session's saved
    /// tree, as it stood there: later turns branch from it, and the nodes
    /// after it stay in the tree. The daemon then detaches every attached
    /// client as [`super::DetachReason::Lagged`]; attaching again replays
    /// the conversation up to `node`. A node the tree does not hold, or a
    /// session that has saved nothing, is
    /// [`crate::ErrorCode::InvalidParams`].
    SelectNode { node: String },
    /// Put back the files the turn of workspace checkpoint `checkpoint`
    /// changed, as far as they still hold what that turn left, and, when
    /// every one was put back, continue the conversation from before that
    /// turn, as [`Self::SelectNode`] does. Clients see what became of each
    /// file as a [`super::SessionEvent::Notice`]. A checkpoint the session
    /// does not have, a session in plan mode, or one with nothing saved is
    /// [`crate::ErrorCode::InvalidParams`].
    RewindWorkspace { checkpoint: String },
}

/// Result of [`SESSION_CONFIGURE`], once the change applied, after the turn
/// under way, if any, ended: the change applies from the next turn, and
/// these are the session's options with it. Every attached client also
/// receives them as [`super::SessionEvent::OptionsChanged`] when they
/// changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfigureResponse {
    pub options: SessionOptions,
}

/// What a session runs with now, and what a client may change it to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOptions {
    pub model: ModelOptions,
    /// Absent when the current model has no reasoning to configure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningOptions>,
    /// Absent when the current model has no fast mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<FastModeOptions>,
    pub permission_mode: PermissionModeOptions,
    /// The commands a prompt may start with or mention, in menu order.
    pub commands: Vec<SessionCommand>,
    /// The daemon-owned goal, absent after completion, clear, or cancel.
    /// Kept while the session is loaded, not across daemon restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<Box<SessionGoal>>,
    /// The directories the session's tools reach besides its workspace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_dirs: Vec<PathBuf>,
}

/// A model, by its provider and the provider's model id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

/// The session's model and the models it may switch to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelOptions {
    pub current: ModelRef,
    /// The auth profile `current` runs on, which names how its use is
    /// billed.
    pub auth: String,
    /// The user's favorites first, then the rest; always includes `current`.
    pub available: Vec<ModelChoice>,
}

/// One model a session may switch to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelChoice {
    pub model: ModelRef,
    /// A short name for pickers, unique within the list: the catalog name,
    /// with the provider or model id added where two would read alike.
    pub label: String,
}

/// How much the model reasons on each turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReasoningLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The session's reasoning level and the levels its model takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningOptions {
    pub current: ReasoningLevel,
    /// Lowest first; always includes `current`.
    pub available: Vec<ReasoningLevel>,
}

/// Fast mode, which the current model has: faster responses at a higher
/// credit rate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FastModeOptions {
    pub enabled: bool,
}

/// The session's permission mode and the modes it may switch to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionModeOptions {
    pub current: PermissionMode,
    pub available: Vec<PermissionMode>,
}

/// One command a prompt may run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCommand {
    /// The command's name, without the `/` or `$` its [`CommandKind`] puts
    /// before it: `init`, `prompt:review`, `mcp:docs:search`, `pdf`.
    pub name: String,
    pub kind: CommandKind,
    pub description: String,
    /// What to type after the name, such as `<query>` or `[instructions]`;
    /// absent for a command that takes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hint: Option<String>,
}

/// How a prompt runs a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CommandKind {
    /// The prompt starts with `/<name>`, then its input.
    Slash,
    /// `$<name>` anywhere in the prompt loads that skill before the turn's
    /// first model request.
    Skill,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
