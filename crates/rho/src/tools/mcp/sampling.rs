//! Answering `sampling/createMessage`: a server asking Rho's model to write
//! something.
//!
//! Sampling spends the user's tokens on work the user never asked for, so it is
//! behind two independent gates that both have to open:
//!
//! 1. the server is opted in by config (`sampling = "ask"`), which is also the
//!    only reason Rho declares the `sampling` capability to it; and
//! 2. the user answers yes to this particular request.
//!
//! The second gate is raised inside the in-flight tool call (see
//! [`super::inflight`]), because that is the only place with a live route to the
//! user: the interactive host drains questionnaires while a turn runs and not
//! while it idles. It is a question rather than a capability approval for two
//! reasons. Rho's default permission mode allows every capability by policy, so
//! an approval prompt would never reach a person there, and token spend a server
//! asked for is not something that mode ever opted into. And the SDK's
//! resource-aware execution contract deliberately withholds `authorize` from a
//! running tool, so an MCP tool cannot raise a capability request it did not
//! declare during preparation.
//!
//! Anything else is a rejection: an unbound model, no in-flight call, a refused
//! prompt, or a request that outlives its budget.

// `sampling` carries a SEP-2577 deprecation marker in rmcp while it is still the
// only completion request in the shipping protocol. Rho implements the current
// wire protocol.
#![expect(deprecated)]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rho_sdk::{
    provider::ModelProvider, HostChoice, HostInputRequest, HostQuestion, SelectionMode, SessionId,
};
use rmcp::{
    model::{
        CreateMessageRequestParams, CreateMessageResult, Role, SamplingMessage,
        SamplingMessageContentBlock,
    },
    ErrorData as McpError,
};

use super::{
    config::McpSamplingPolicy,
    inflight::{McpCaller, McpInFlightCalls},
};

// A server must never be able to hold a turn open on a model request. Long
// enough for a slow reasoning model, short enough to be a bound.
const MCP_SAMPLING_BUDGET: std::time::Duration = std::time::Duration::from_secs(180);

/// Attributes sampling spend in the usage ledger, apart from the user's own
/// turns and from other internal one-shots.
const SAMPLING_USAGE_PURPOSE: &str = "mcp_sampling";

const SAMPLING_AGENT_ID: &str = "mcp-sampling";

/// Fallback system prompt for a server that sent none.
const SAMPLING_DEFAULT_PROMPT: &str =
    "You are answering a request from a Model Context Protocol server. Answer only what is asked, in plain text.";

/// The live session state one sampling call needs.
///
/// Held behind [`McpSamplingBridge`] rather than captured when a server
/// connects, because servers connect during tool assembly and because the user
/// can change models mid-session; a captured provider would go stale.
#[derive(Clone)]
pub(crate) struct McpSamplingModel {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) session_id: SessionId,
    pub(crate) workspace_path: PathBuf,
}

impl std::fmt::Debug for McpSamplingModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpSamplingModel")
            .field("provider", &self.provider.identity())
            .finish_non_exhaustive()
    }
}

/// Late-bound access to the session's model.
///
/// Mirrors the parent binding used for delegated questionnaires: the host binds
/// once the runtime exists, rebinds when the model changes, and an unbound
/// bridge is an error rather than a silent success.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpSamplingBridge {
    model: Arc<Mutex<Option<McpSamplingModel>>>,
}

impl McpSamplingBridge {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Install the model sampling runs against. Replaces any previous binding.
    pub(crate) fn bind(&self, model: McpSamplingModel) {
        *self.lock() = Some(model);
    }

    /// Drop the binding so later requests fail closed.
    pub(crate) fn unbind(&self) {
        *self.lock() = None;
    }

    fn bound_model(&self) -> Result<McpSamplingModel, McpError> {
        self.lock().clone().ok_or_else(|| {
            McpError::internal_error("Rho has no model bound for MCP sampling in this run", None)
        })
    }

    /// A poisoned lock still holds a valid binding, so recover rather than turn
    /// an unrelated panic into a failed request.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<McpSamplingModel>> {
        self.model.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// Serves one session's sampling requests under that server's policy.
#[derive(Clone, Debug)]
pub(crate) struct McpSamplingService {
    identity: String,
    policy: McpSamplingPolicy,
    bridge: McpSamplingBridge,
    calls: McpInFlightCalls,
}

impl McpSamplingService {
    pub(crate) fn new(
        identity: impl Into<String>,
        policy: McpSamplingPolicy,
        bridge: McpSamplingBridge,
        calls: McpInFlightCalls,
    ) -> Self {
        Self {
            identity: identity.into(),
            policy,
            bridge,
            calls,
        }
    }

    pub(crate) async fn create_message(
        &self,
        params: CreateMessageRequestParams,
    ) -> Result<CreateMessageResult, McpError> {
        if !self.policy.is_offered() {
            return Err(McpError::invalid_request(
                "this MCP server is not configured for sampling in Rho",
                None,
            ));
        }
        let model = self.bridge.bound_model()?;
        let caller = self
            .calls
            .sole_caller()
            .map_err(|error| McpError::invalid_request(error.reason(), None))?;
        self.confirm_with_user(&caller, &params).await?;
        self.run(&caller, &model, params).await
    }

    /// Ask the user about this specific request.
    async fn confirm_with_user(
        &self,
        caller: &McpCaller,
        params: &CreateMessageRequestParams,
    ) -> Result<(), McpError> {
        let question = HostQuestion::new(
            "allow",
            format!(
                "MCP server `{}` wants to send {} message(s) to your model and asks for up to {} tokens. Allow it?",
                self.identity,
                params.messages.len(),
                params.max_tokens,
            ),
            vec![HostChoice::new("yes", "Yes"), HostChoice::new("no", "No")],
            SelectionMode::One,
        )
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let request = HostInputRequest::questionnaire(
            format!("MCP server `{}` asks to use your model", self.identity),
            vec![question],
        )
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let response = caller
            .ask(request)
            .await
            .map_err(|error| McpError::invalid_request(error.to_string(), None))?;
        let allowed = response
            .answers()
            .get("allow")
            .is_some_and(|answers| answers.iter().any(|answer| answer == "yes"));
        if !allowed {
            return Err(McpError::invalid_request(
                "the user refused this sampling request",
                None,
            ));
        }
        Ok(())
    }

    async fn run(
        &self,
        caller: &McpCaller,
        model: &McpSamplingModel,
        params: CreateMessageRequestParams,
    ) -> Result<CreateMessageResult, McpError> {
        let definition = sampling_definition(params.system_prompt.as_deref())
            .map_err(|error| McpError::internal_error(error, None))?;
        let input = flatten_messages(&params.messages);
        let usage_recording = crate::usage::default_recording().await;
        let request = crate::agent::OneShotAgentRequest {
            definition: &definition,
            usage_purpose: SAMPLING_USAGE_PURPOSE,
            reasoning: None,
            input,
            cancellation: caller.cancellation().clone(),
            session_id: &model.session_id,
            workspace_path: &model.workspace_path,
        };
        let started = crate::agent::run_one_shot_with_provider(
            model.provider.as_ref(),
            request,
            usage_recording,
            /*updates*/ None,
        );
        let result = tokio::time::timeout(MCP_SAMPLING_BUDGET, started)
            .await
            .map_err(|_| {
                McpError::internal_error(
                    format!(
                        "the sampling request exceeded its {}s budget",
                        MCP_SAMPLING_BUDGET.as_secs()
                    ),
                    None,
                )
            })?
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let text = result.texts.join("\n");
        let identity = model.provider.identity();
        Ok(
            CreateMessageResult::new(SamplingMessage::assistant_text(text), identity.model)
                .with_stop_reason(CreateMessageResult::STOP_REASON_END_TURN),
        )
    }
}

/// The one-shot definition a sampling request runs under.
///
/// A server's `modelPreferences` is deliberately ignored: the model, provider,
/// and credentials are the user's configuration, and letting a server steer
/// them would turn a sampling request into a way to pick which of the user's
/// accounts pays and which model sees the prompt. The server does get its
/// system prompt, because that is the request's content rather than its target.
fn sampling_definition(
    system_prompt: Option<&str>,
) -> Result<crate::agent::AgentDefinition, String> {
    let prompt = system_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .unwrap_or(SAMPLING_DEFAULT_PROMPT);
    Ok(crate::agent::AgentDefinition {
        id: crate::agent::AgentId::new(SAMPLING_AGENT_ID).map_err(|error| error.to_string())?,
        description: "Answers one Model Context Protocol sampling request.".into(),
        prompt: crate::agent::PromptPolicy::Replace(prompt.to_owned()),
        runtime: crate::agent::AgentRuntimeSpec::Rho {
            tools: crate::agent::ToolPolicy::Allow(std::collections::BTreeSet::new()),
            model: crate::agent::ModelPolicy::Inherit,
            reasoning: Some(crate::agent::ReasoningLevel::Low),
        },
    })
}

/// Flatten the server's conversation into one user message.
///
/// Rho's one-shot path takes a system prompt and a single user turn, so a
/// multi-turn sampling conversation is rendered as labelled text rather than
/// replayed as real turns. Non-text blocks are named but not sent: Rho does not
/// forward a server's images or tool results into the user's model.
fn flatten_messages(messages: &[SamplingMessage]) -> String {
    let mut rendered = Vec::with_capacity(messages.len());
    for message in messages {
        let speaker = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        let body = message
            .content
            .iter()
            .map(|block| match block {
                SamplingMessageContentBlock::Text(text) => text.text.clone(),
                other => format!("[unsupported {} content omitted]", content_label(other)),
            })
            .collect::<Vec<_>>()
            .join("\n");
        rendered.push(format!("{speaker}: {body}"));
    }
    rendered.join("\n\n")
}

fn content_label(block: &SamplingMessageContentBlock) -> &'static str {
    match block {
        SamplingMessageContentBlock::Text(_) => "text",
        SamplingMessageContentBlock::Image(_) => "image",
        // Non-exhaustive upstream, and every added block kind is one Rho does
        // not forward, so one label covers them all.
        _ => "non-text",
    }
}

#[cfg(test)]
#[path = "sampling_tests.rs"]
mod tests;
