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

use super::{PermissionMode, SessionId};

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
    /// Switch to `model` as [`Self::Model`] does, only if the session's
    /// current or last prompted turn is a `/compact` that reduced the
    /// conversation. Queued behind that turn like any change, it applies once
    /// the turn ends having compacted and is dropped otherwise, leaving the
    /// session's options as they were. A host sends it once a `/compact` it
    /// asked for started, to return to the model the session compacted from,
    /// so the switch back outlives the host and a change queued after it
    /// still applies last.
    ModelAfterCompaction {
        model: ModelRef,
        /// As [`Self::Model`]'s `auth`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<String>,
    },
    /// Spend `level`, one of [`ReasoningOptions::available`], on later turns.
    Reasoning { level: ReasoningLevel },
    /// Turn fast mode on or off. Turning it on needs a model that has it
    /// ([`SessionOptions::fast_mode`] present); off always applies.
    FastMode { enabled: bool },
    /// Gate later turns' tool calls with `mode`.
    PermissionMode { mode: PermissionMode },
    /// Open the sessions this session starts through session control (the
    /// `sessions` tool's `new`, which names it as
    /// [`super::SessionNewParams::parent`]) in `mode`, as if a person had
    /// set it on each before its first turn; absent, they take the
    /// config's. A person's client sets it; no tool rho gives the model
    /// does. The sessions this session's subagents and workflow nodes start
    /// take it too, and each session started under it keeps a copy as its
    /// own choice, so the sessions that one starts open in `mode` as well,
    /// until a person changes it there: changing it here later does not
    /// change what sessions already started hand to the sessions they
    /// start. `Unknown`, or `auto` while the config has
    /// no permission-classifier model, is
    /// [`crate::ErrorCode::InvalidParams`]. The session reports its choice as
    /// [`super::SessionEvent::StartedSessionDefaults`]. It is kept with the
    /// session apart from the session's own settings: setting it leaves the
    /// session following the config, or keeping its settings, as before.
    /// Sent while the session's turn runs, it waits for that turn's end, as
    /// every change does, so the sessions that turn starts meanwhile open
    /// with the choice as it was.
    StartedSessionPermissionMode {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<PermissionMode>,
    },
    /// Open the sessions this session starts through session control on
    /// `model`, one of [`ModelOptions::available`], as
    /// [`Self::StartedSessionPermissionMode`] sets their mode, and carried
    /// down and kept as that one is; absent, they take the config's. A
    /// model the `new` call names itself still wins for that session, and
    /// a session whose agent names its own model, preferred or required,
    /// runs that one, and does not need `model` to resolve. Each
    /// started session runs `model` on an auth that has credentials, as a
    /// [`Self::Model`] with no `auth` picks; one it no longer resolves to
    /// fails the `new`. While the session keeps a saved choice this daemon
    /// cannot read, it is [`crate::ErrorCode::InvalidParams`]: a
    /// [`Self::StartedSessionPermissionMode`] replaces that choice first.
    /// Sent while the session's turn runs, it waits for that turn's end, as
    /// [`Self::StartedSessionPermissionMode`] does.
    StartedSessionModel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelRef>,
    },
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
    /// follow, and so does every [`super::MODEL_LIST`] subscriber.
    ///
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-protocol): remove `SessionChange::ModelCatalog`; clients have the daemon fetch the lists with `rho/model/refresh`, which needs no session.
    ///
    /// The daemon now fetches the lists itself, so nothing in rho sends it;
    /// it stays so a client of this major that writes the lists still
    /// reaches the daemon.
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

/// What a person chose for the sessions a session starts through session
/// control ([`SessionChange::StartedSessionPermissionMode`] and
/// [`SessionChange::StartedSessionModel`]): the permission mode and the
/// model they open with. A part left `None` is not chosen, and those
/// sessions take the config's. [`super::SessionEvent::StartedSessionDefaults`]
/// carries it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartedSessionDefaults {
    /// `Unknown` for a mode a newer rho saved, or a saved choice this daemon
    /// cannot read, under which it refuses to open a session. A client that
    /// sees it should offer to set the mode again, which replaces the saved
    /// choice. While the choice does not read, `model` is absent even when
    /// the saved choice names one, which the daemon keeps as saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
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
