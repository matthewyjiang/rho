//! The models a client may pick outside a session, as a settings screen
//! offers them for a model setting such as the conversation model, and the
//! provider model lists the daemon keeps.
//!
//! The daemon owns those lists: it fetches a provider's list once a
//! credential for it is saved ([`super::AUTH_KEY`], [`super::AUTH_LOGIN`])
//! and again on [`MODEL_REFRESH`], so every client reads the same lists.
//! A session's own pickers read its [`super::ModelOptions`], which the
//! same lists build.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ClientEnvironment, ModelChoice};

/// List the models a client may pick for a model setting, and subscribe
/// this connection to [`MODEL_CHANGED`].
pub const MODEL_LIST: &str = "rho/model/list";
/// Fetch the signed-in providers' model lists again, and the models.dev
/// catalog of model names and capabilities.
pub const MODEL_REFRESH: &str = "rho/model/refresh";
/// Notification: the models [`MODEL_LIST`] answered this connection with
/// changed.
pub const MODEL_CHANGED: &str = "rho/model/changed";

/// Params of [`MODEL_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelListParams {
    /// The asking client's environment, whose credential variables and
    /// `RHO_CREDENTIAL_STORE` decide which providers it is signed in to,
    /// as for [`super::AUTH_LIST`].
    pub environment: ClientEnvironment,
}

/// Result of [`MODEL_LIST`], and the params of [`MODEL_CHANGED`], which
/// carries the whole list again whenever it changes: after a credential
/// write, a refreshed model list, or [`super::SessionChange::ModelCatalog`].
/// Listing again replaces the connection's subscription, so the
/// notification then follows the latest `environment`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelList {
    /// The models rho runs that the client's credentials reach
    /// ([`super::ModelRuntime::Rho`]), the user's favorites first, labelled
    /// as a session's [`super::ModelOptions::available`] labels them. A
    /// change to the favorites alone sends no [`MODEL_CHANGED`]; the order
    /// follows at the next change above.
    pub models: Vec<ModelChoice>,
    /// The models a [`super::ModelRuntime::ClaudeCli`] setting may take:
    /// the CLI's own default first, then the aliases it resolves to its
    /// latest models. The CLI also takes full model ids this list does
    /// not carry.
    pub claude_cli: Vec<ClaudeCliModel>,
}

/// One model the Claude Code CLI runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaudeCliModel {
    /// What [`super::ModelSelection::ClaudeCli`]'s `model` holds for it;
    /// absent for the CLI's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub label: String,
    /// What the model is, for a picker row.
    pub detail: String,
}

/// Params of [`MODEL_REFRESH`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRefreshParams {
    /// The asking client's environment, as for [`ModelListParams`]: its
    /// credentials fetch the lists.
    pub environment: ClientEnvironment,
    /// Only this provider's lists, without the models.dev catalog, as
    /// after a sign-in the client made without the daemon. Absent fetches
    /// every signed-in provider's lists, then the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<ModelRefreshTarget>,
}

/// The one provider a [`MODEL_REFRESH`] fetches the lists of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRefreshTarget {
    pub provider: String,
    /// The auth profile whose credential fetches them, as `--auth` names
    /// one, such as `none` for a host that takes no key. One the provider
    /// does not take is [`crate::ErrorCode::InvalidParams`], as is a host
    /// logged in through its endpoint (Ollama) before that login.
    pub auth: String,
}

/// Result of [`MODEL_REFRESH`], once every fetch ended. Every loaded
/// session's options and every [`MODEL_LIST`] subscriber follow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRefreshResponse {
    /// Each list fetched, by provider: one for each signed-in provider
    /// that lists its models, none for one that lists none.
    pub lists: Vec<ModelListRefresh>,
    /// The models.dev catalog, which names models and says what each can
    /// do; absent when the refresh asked for [`ModelRefreshParams::only`]
    /// one provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<ModelRefreshOutcome>,
}

/// One provider model list the daemon fetched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelListRefresh {
    pub provider: String,
    pub list: ModelListKind,
    pub outcome: ModelRefreshOutcome,
}

/// Which of a provider's model lists. Protocol minors may add kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelListKind {
    /// The models a session can talk to.
    Chat,
    /// The models a host serves for decisions, such as the permission
    /// classifier's.
    Decision,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How one fetch went. Protocol minors may add outcomes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelRefreshOutcome {
    /// The fetch saved `models` entries.
    Refreshed { models: u32 },
    /// The fetch failed; `message` says why, for a person. The list the
    /// daemon had stays.
    Failed { message: String },
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
