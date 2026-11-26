//! [`super::SessionReport::Context`]: the next request's context window,
//! attributed by source.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What fills the model's context window on the next request, by source.
///
/// Part tokens are local estimates scaled to [`Self::tokens`], so they share
/// its scale when a provider measured the prompt; fixed request framing is
/// in `tokens` but in no part. Parts with no tokens are left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionContextReport {
    /// Tokens the next request's context takes, on [`Self::basis`].
    pub tokens: u64,
    /// What [`Self::tokens`] is based on.
    pub basis: ContextBasis,
    /// The model's context window in tokens, when the session knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_tokens: Option<u64>,
    /// The system prompt, by source, in prompt order.
    pub system_prompt: Vec<ContextPromptPart>,
    /// The tool schemas sent to the model, by origin, largest first.
    pub tool_schemas: Vec<ContextToolSchemaPart>,
    /// The conversation's messages other than tool results, by kind,
    /// largest first.
    pub messages: Vec<ContextMessagePart>,
    /// Tool results, by the tool that returned them, largest first.
    pub tool_results: Vec<ContextToolResultPart>,
    /// Host context sent with each request, outside the conversation.
    pub request_context_tokens: u64,
}

/// What a context total is based on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextBasis {
    /// The last prompt size a provider reported, plus local estimates for
    /// what the conversation added since.
    ProviderCalibrated,
    /// A local estimate only: no provider measured a prompt of this context.
    LocalEstimate,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One source of the system prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextPromptPart {
    pub source: PromptSourceKind,
    /// The file it was read from, when it came from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub tokens: u64,
}

/// What a part of the system prompt is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PromptSourceKind {
    /// rho's own instructions.
    Base,
    /// A model's prompt, appended to rho's instructions.
    ModelAppend,
    /// A model's prompt, in place of rho's instructions.
    ModelReplace,
    /// An instruction file, such as an `AGENTS.md`.
    InstructionFile,
    /// The listing of the skills the session offers.
    Skills,
    /// The whole system prompt, when the session did not record its
    /// sources.
    Unattributed,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The tool schemas of one origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextToolSchemaPart {
    pub origin: ToolSchemaOrigin,
    /// How many tool schemas of this origin the request sends.
    pub schemas: u64,
    pub tokens: u64,
}

/// Where a tool the model sees comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolSchemaOrigin {
    /// rho's own tools, and any tool not from an MCP server.
    BuiltIn,
    /// An MCP server, by its qualified identity.
    Mcp { server: String },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The messages of one kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextMessagePart {
    pub kind: ContextMessageKind,
    pub tokens: u64,
}

/// A kind of message in the conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextMessageKind {
    /// The model's replies: text, reasoning and tool calls.
    Assistant,
    /// The person's prompts and the notes a host adds to them.
    User,
    /// The summary that replaced compacted history.
    CompactionSummary,
    /// Notices rho adds to the conversation.
    SystemNotice,
    /// Request items rho does not classify yet.
    Unclassified,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The results of one tool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextToolResultPart {
    /// The tool's name; absent for results whose call is no longer in the
    /// conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub tokens: u64,
}
