//! Input requests: what a session's turn waits on a person for (#1660).
//!
//! A turn that needs a tool approval, a plan approval, or answers to a
//! question opens an [`InputRequest`] and its job waits in
//! [`super::JobState::WaitingForInput`]. Attached clients learn of it from a
//! [`super::SessionEvent::InputRequested`] event and may answer it with
//! [`SESSION_ANSWER`]. The first answer wins; every attached client then
//! sees [`super::SessionEvent::InputResolved`]. A client that detaches or
//! disconnects answers nothing: with nobody attached the request stays
//! open until someone answers it or the turn is cancelled.

use std::{collections::BTreeMap, fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{SessionId, ToolCallId};

/// Answer one of a session's open input requests.
pub const SESSION_ANSWER: &str = "rho/session/answer";

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
    /// The tool call that asks, if a tool call does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallId>,
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
    /// Whether an answer may pick several choices, rather than one.
    pub multiple: bool,
    /// Whether an answer may give its own text instead of a choice.
    pub allow_other: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// The answer the agent suggests, as it gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Whether to pre-select `default` (`true`) or only focus it, so the
    /// user still picks it (`false`).
    pub preselect_default: bool,
    /// Whether an answer must answer this question.
    pub required: bool,
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

/// Result of [`SESSION_ANSWER`]: the answer won, and the turn has it.
/// An answer that lost, to another client's or to the turn withdrawing the
/// request, is [`crate::ErrorCode::InputNotOpen`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionAnswerResponse {}
