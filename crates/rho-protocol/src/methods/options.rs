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

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PermissionMode, SessionId};

/// Change one setting of a loaded session between its turns. The session
/// keeps the change with its file, so it outlasts the daemon unloading the
/// session. A session the daemon has not loaded is
/// [`crate::ErrorCode::SessionNotLoaded`]; one with a turn under way, or
/// another change applying, is [`crate::ErrorCode::SessionBusy`]. A change
/// that fails as it applies is [`crate::ErrorCode::InternalError`].
pub const SESSION_CONFIGURE: &str = "rho/session/configure";

/// Params of [`SESSION_CONFIGURE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfigureParams {
    pub session: SessionId,
    /// One change per request, so an error names the change it refuses.
    pub change: SessionChange,
}

/// One setting to change. The value must be one the session's
/// [`SessionOptions`] offers; any other is [`crate::ErrorCode::InvalidParams`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionChange {
    /// Switch to `model`, one of [`ModelOptions::available`]. The session
    /// keeps its reasoning level, or refuses a model that cannot.
    Model { model: ModelRef },
    /// Spend `level`, one of [`ReasoningOptions::available`], on later turns.
    Reasoning { level: ReasoningLevel },
    /// Turn fast mode on or off. Turning it on needs a model that has it
    /// ([`SessionOptions::fast_mode`] present); off always applies.
    FastMode { enabled: bool },
    /// Gate later turns' tool calls with `mode`.
    PermissionMode { mode: PermissionMode },
}

/// Result of [`SESSION_CONFIGURE`]: the change applies from the next turn,
/// and these are the session's options with it. Every attached client also
/// receives them as [`super::SessionEvent::OptionsChanged`] when they
/// changed. A session with a turn running or waiting for input refuses with
/// [`crate::ErrorCode::SessionBusy`]; send the change again once it ends.
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
