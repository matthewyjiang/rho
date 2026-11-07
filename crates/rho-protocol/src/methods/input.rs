//! Input requests: what a session's turn waits on a person for (#1660).
//!
//! A turn that needs a tool approval, a plan approval, or answers to a
//! question opens an [`InputRequest`] and its job waits in
//! [`super::JobState::WaitingForInput`]. Attached clients learn of it from a
//! [`super::SessionEvent::InputRequested`] event, or a client that attaches
//! later from [`super::SessionAttachResponse::inputs`], and may answer it
//! with [`SESSION_ANSWER`]. The first answer wins; every attached client then
//! sees [`super::SessionEvent::InputResolved`]. A client that detaches or
//! disconnects answers nothing: with nobody attached the request stays
//! open until someone answers it or the turn is cancelled.
//!
//! A request with an [`InputTimeout`] is the exception the user configured:
//! the daemon answers it itself at the deadline, unless a client answers it
//! first or holds it with [`SESSION_HOLD`], as a client does once its user
//! starts on the request.

use std::{collections::BTreeMap, fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{SessionId, ToolCallId};

/// Answer one of a session's open input requests.
pub const SESSION_ANSWER: &str = "rho/session/answer";

/// Stop an open input request's timeout, so it waits for an answer.
pub const SESSION_HOLD: &str = "rho/session/hold";

/// Identifies one input request among its session's, for as long as the
/// daemon has the session loaded.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct InputRequestId(pub u64);

impl fmt::Display for InputRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// One thing a turn waits on a person for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputRequest {
    pub id: InputRequestId,
    pub ask: Ask,
    /// The tool call that asks, when a tool call does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallId>,
    /// When the daemon answers the request itself, if it will; absent once
    /// the request is held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<InputTimeout>,
}

/// An answer the daemon gives an input request at a deadline, unless a
/// client answers the request or holds it first. Only a question whose
/// agent offered fallback answers, in a session whose user set
/// `[questionnaire] timeout_seconds`, has one (#1660).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputTimeout {
    /// Unix time, in milliseconds, when the daemon answers.
    pub deadline_ms: u64,
    /// What the daemon answers with.
    pub answer: InputAnswer,
    /// Why the agent offered that answer.
    pub reason: String,
}

/// What an input request asks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Ask {
    /// May a tool use a capability? Answer [`InputAnswer::AllowOnce`],
    /// [`InputAnswer::AllowForSession`], or [`InputAnswer::Deny`].
    Approval(ApprovalAsk),
    /// May the session leave plan mode to carry out its plan? Answer
    /// [`InputAnswer::ApprovePlan`] or [`InputAnswer::KeepPlanning`].
    PlanApproval(PlanApprovalAsk),
    /// Questions for the user. Answer [`InputAnswer::Answers`],
    /// [`InputAnswer::Decline`], or [`InputAnswer::Dismiss`].
    Question(QuestionAsk),
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A tool asks to use a capability its session's permission mode does not
/// grant on its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalAsk {
    /// Who asks.
    pub source: ApprovalSource,
    /// What it would do.
    pub operation: Operation,
    /// Why the policy asks, in its words.
    pub reason: String,
}

/// Who asks for a capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApprovalSource {
    /// One of Rho's own tools.
    BuiltInTool { name: String },
    /// A tool a host or integration provides, such as an MCP server's: a
    /// different trust boundary from Rho's own.
    HostTool { name: String },
    /// Rho itself, as when it builds a prompt.
    Rho,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// What a capability would let a tool do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Operation {
    ReadPath {
        path: PathBuf,
        scope: PathScope,
    },
    WritePath {
        path: PathBuf,
        scope: PathScope,
    },
    DiscoverInstructions {
        path: PathBuf,
        scope: PathScope,
    },
    /// Run `program` with `args` in `cwd`. `shell_command` is the command
    /// line, when the program is a shell running one.
    RunProcess {
        program: PathBuf,
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell_command: Option<String>,
        cwd: PathBuf,
    },
    /// Reach the network, at `url` when the tool names one.
    Network {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    LoadSkill {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
    },
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Where a path an operation names lies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PathScope {
    /// Under the session's workspace.
    Workspace,
    /// Under a directory the user added to the session, `root`.
    GrantedRoot { root: PathBuf },
    /// Anywhere: outside every root the session knows.
    Unrestricted,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The session, in plan mode, asks to carry out its plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanApprovalAsk {
    /// The plan, as markdown.
    pub plan: String,
    /// The permission modes the session may carry the plan out in; an
    /// approval names one.
    pub modes: Vec<PermissionMode>,
}

/// A session's permission mode, as `/permissions` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PermissionMode {
    Bypass,
    Auto,
    AllowEdits,
    Plan,
    Supervised,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Questions the agent asks the user, answered together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionAsk {
    pub title: String,
    pub questions: Vec<Question>,
}

/// One question, with the choices it offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    /// Names the question in an answer's `answers`.
    pub id: String,
    pub prompt: String,
    /// A short label for the question, such as a tab title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub choices: Vec<Choice>,
    /// How many choices an answer may pick.
    pub selection: Selection,
    /// Whether an answer may give its own text instead of a choice.
    pub allow_other: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// The answer the agent suggests, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<DefaultAnswer>,
    /// Whether an answer must answer this question.
    pub required: bool,
}

/// How many choices a question's answer may pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Selection {
    One,
    Many,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The answer the agent suggests for a question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DefaultAnswer {
    /// What the answer would send: the value of each choice it names, or
    /// the agent's own text where it names none.
    pub values: Vec<String>,
    pub style: DefaultStyle,
}

/// How a client offers a question's suggested answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DefaultStyle {
    /// Pre-select it.
    Selected,
    /// Only focus or mark it, so the user still picks it.
    Focused,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One choice a question offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Choice {
    /// What an answer sends when it picks this choice.
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// How an input request closed, as [`super::SessionEvent::InputResolved`]
/// tells every attached client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Resolution {
    /// A client answered it.
    Answered,
    /// The turn no longer waits for it, as when the turn was cancelled.
    Withdrawn,
    /// Nobody answered or held it by its timeout's deadline, so the daemon
    /// answered it with the timeout's answer.
    TimedOut,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Params of [`SESSION_ANSWER`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionAnswerParams {
    pub session: SessionId,
    pub request: InputRequestId,
    /// One of the answers the request's [`Ask`] lists.
    pub answer: InputAnswer,
}

/// An answer to an input request. Each [`Ask`] lists the answers it takes;
/// any other is [`crate::ErrorCode::InvalidParams`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputAnswer {
    /// Approve this one use of the capability.
    AllowOnce,
    /// Approve this exact use for the rest of the session.
    AllowForSession,
    /// Refuse it; the tool learns `reason`, or that the user denied it.
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Leave plan mode and carry out the plan in `mode`, one of the
    /// request's `modes`.
    ApprovePlan { mode: PermissionMode },
    /// Stay in plan mode.
    KeepPlanning,
    /// Answers keyed by question id: the values of the chosen choices, or
    /// the user's own text where a question allows it.
    Answers {
        answers: BTreeMap<String, Vec<String>>,
    },
    /// The user declined to answer the questions.
    Decline,
    /// The user dismissed the questions without choosing.
    Dismiss,
}

/// Result of [`SESSION_ANSWER`]: the answer won, and the daemon handed it
/// to the turn. An answer that lost, to another client's or to the turn
/// withdrawing the request, is [`crate::ErrorCode::InputNotOpen`]. A turn
/// that stops waiting at the same moment, because it was cancelled or the
/// tool that asked gave up, ignores an answer that won.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionAnswerResponse {}

/// Params of [`SESSION_HOLD`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionHoldParams {
    pub session: SessionId,
    pub request: InputRequestId,
}

/// Result of [`SESSION_HOLD`]: the request no longer times out, and every
/// attached client saw [`super::SessionEvent::InputHeld`]. Holding a
/// request that has no timeout, or one already held, changes nothing. A
/// request that closed is [`crate::ErrorCode::InputNotOpen`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionHoldResponse {}
