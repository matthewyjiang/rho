//! How a turn ended (#1669), as [`super::SessionEvent::TurnEnded`] tells
//! every attached client once, before the [`super::SessionEvent::State`]
//! that records the job's end.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TurnOutcome {
    /// The model gave its final answer. `text` is the answer whole, as the
    /// turn saved it, which a client that missed chunks or saw a provider
    /// retry restart a reply can show instead of what it gathered.
    Answered { text: String, stop: AnswerStop },
    /// The turn used its model-step limit before the model answered; `text`
    /// is the reply text of its last step, possibly empty.
    MaxSteps { text: String },
    /// A client cancelled it.
    Cancelled,
    /// The daemon cancelled it at the time limit its prompt set,
    /// [`super::SessionPromptParams::timeout_ms`], repeated here.
    TimedOut { timeout_ms: u64 },
    /// It failed.
    Failed { error: TurnError },
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Why the model stopped at its final answer. A daemon that does not know
/// why, as for a reason a newer SDK adds, sends `unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnswerStop {
    /// It finished its answer.
    EndTurn,
    /// The provider cut the answer off at its output token limit.
    MaxTokens,
    /// The model refused.
    Refusal,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Why a turn failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnError {
    pub kind: TurnErrorKind,
    /// What went wrong, for a person. An authentication failure names only
    /// the provider and auth and how to log in, never the provider's own
    /// text, which may echo credentials.
    pub message: String,
}

/// What kind of failure ended a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TurnErrorKind {
    /// The provider rejected the credentials, or there were none.
    Authentication,
    /// The provider failed the request.
    Provider,
    /// A tool's host failed, not just one tool call.
    ToolHost,
    /// The session's config cannot run, such as a model the provider does
    /// not offer.
    Configuration,
    /// Anything else.
    Other,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
