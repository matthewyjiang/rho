//! The `advisor` tool: a reviewer model reads the session and advises the executor.
//!
//! The tool takes no arguments. Rho serializes the session itself, so nothing
//! the executor writes reaches the advisor. The advisor runs as a one-shot Rho
//! agent with no tools and returns only advice text.

mod transcript;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rho_sdk::{
    model::{ModelUsage, ToolSpec},
    tool::{
        OperationKind, Tool as SdkTool, ToolContext, ToolError, ToolErrorKind, ToolFuture,
        ToolInvocation, ToolMetadata, ToolOutput, ToolSecurity,
    },
    CancellationToken, Session, SessionId,
};
use serde_json::json;

use crate::{
    agent::{
        internal_agent_requires_model, internal_definition, run_one_shot_agent,
        OneShotAgentRequest, ADVISOR_AGENT_ID,
    },
    config::{Config, InternalAgentModelConfig},
};

pub(crate) use transcript::{TranscriptBudget, DEFAULT_TRANSCRIPT_BUDGET};

pub(crate) const TOOL_NAME: &str = "advisor";

const USAGE_PURPOSE: &str = "advisor";

const TOOL_DESCRIPTION: &str = "Consult a stronger reviewer model about this session. Takes no parameters: your whole conversation history, including the task, every tool call you made, and every result you saw, is forwarded automatically. Returns strategic guidance on what to do next. Call it before substantive work, when you are stuck, when you consider changing approach, and when you believe the task is complete.";

const NO_MODEL_MESSAGE: &str =
    "advisor mode has no advisor model. Choose one with /advisor, then call advisor again.";

const NO_SESSION_MESSAGE: &str = "the advisor is not attached to a live session";

const NO_WORKSPACE_MESSAGE: &str = "the advisor requires a configured workspace";

const NO_GUIDANCE_MESSAGE: &str = "the advisor model returned no guidance";

/// The advisor's configured model, or `None` when the user has not chosen one.
///
/// The advisor is the one internal agent with no conversation-model fallback
/// (see [`internal_agent_requires_model`]): an advisor that mirrors the
/// executor adds nothing, so an unset model stays unset.
pub(crate) fn advisor_model(config: &Config) -> Option<&InternalAgentModelConfig> {
    debug_assert!(internal_agent_requires_model(ADVISOR_AGENT_ID));
    config.internal_agent_model(ADVISOR_AGENT_ID)
}

/// Reasoning level the advisor run will use.
///
/// An explicit config value wins. Otherwise the reserved advisor definition
/// default applies (medium).
pub(crate) fn advisor_effective_reasoning(
    model: &InternalAgentModelConfig,
) -> rho_providers::reasoning::ReasoningLevel {
    model.reasoning.unwrap_or_else(|| {
        internal_definition(ADVISOR_AGENT_ID)
            .reasoning()
            .unwrap_or(rho_providers::reasoning::ReasoningLevel::Medium)
    })
}

/// Whether the `advisor` tool can run under this configuration.
///
/// Advisor mode on with no advisor model is a real state; the tool stays off
/// until the user picks a model.
pub(crate) fn advisor_available(config: &Config) -> bool {
    config.advisor_mode && advisor_model(config).is_some()
}

/// Builds the `advisor` tool.
///
/// It owns no resources beyond the store and needs no shutdown, so the tool set
/// registers it directly instead of through a bundle. That also lets advisor
/// mode register and remove it mid-session without rebuilding the tool set.
pub(super) fn advisor_tool(store: AdvisorSessionStore) -> Arc<dyn SdkTool> {
    Arc::new(AdvisorTool::new(store, DEFAULT_TRANSCRIPT_BUDGET))
}

/// Live session state the `advisor` tool reads when the executor calls it.
///
/// Built with the tool set, before a session exists, then bound afterwards the
/// way `WebAccessStore` and `SubagentManager` are. Holds a [`Session`] handle
/// rather than a copy of the history, so a replaced session rebinds in one
/// place and the tool always reads current state.
#[derive(Clone, Default)]
pub struct AdvisorSessionStore {
    state: Arc<Mutex<AdvisorSessionState>>,
}

#[derive(Default)]
struct AdvisorSessionState {
    session: Option<Session>,
    system_prompt: Option<String>,
    model: Option<InternalAgentModelConfig>,
    /// Provider-reported advisor spend not yet folded into the parent TUI total.
    unclaimed_cost_usd_micros: u64,
}

impl AdvisorSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Points the advisor at the session the executor is running.
    ///
    /// Changing the session id drops unclaimed advisor spend so a new
    /// conversation never inherits cost from the previous one. Same-id rebinds
    /// (runtime policy rebuilds) keep the accumulator.
    pub fn bind_session(&self, session: Session) {
        let mut state = self.lock();
        let same_session = state
            .session
            .as_ref()
            .is_some_and(|current| current.id() == session.id());
        if !same_session {
            state.unclaimed_cost_usd_micros = 0;
        }
        state.session = Some(session);
    }

    /// Records the executor system prompt, which the advisor reviews alongside
    /// the messages.
    pub fn bind_system_prompt(&self, prompt: Option<String>) {
        self.lock().system_prompt = prompt;
    }

    /// Replaces the advisor model, so a `/advisor` model change applies to the
    /// next call without rebuilding the tool set.
    pub fn set_model(&self, model: Option<InternalAgentModelConfig>) {
        self.lock().model = model;
    }

    /// Fold provider-reported cost from a finished advisor call into the
    /// unclaimed total.
    ///
    /// Only `cost_usd_micros` counts — same contract as subagent terminal
    /// costs. Tokens-only providers and models without a provider cost stay
    /// silent; the TUI does not estimate advisor spend from metadata.
    pub fn note_usage(&self, usage: &ModelUsage) {
        let Some(cost) = usage.cost_usd_micros.filter(|cost| *cost > 0) else {
            return;
        };
        let mut state = self.lock();
        state.unclaimed_cost_usd_micros = state.unclaimed_cost_usd_micros.saturating_add(cost);
    }

    /// Takes advisor costs that have not yet been added to the parent session
    /// total. Safe to call from any TUI poll path; returns 0 when nothing is
    /// new. Costs claimed only through this poll can be lost if the TUI exits
    /// before the next refresh - same as subagent terminal-cost claims.
    pub fn claim_cost_usd_micros(&self) -> u64 {
        let mut state = self.lock();
        let claimed = state.unclaimed_cost_usd_micros;
        state.unclaimed_cost_usd_micros = 0;
        claimed
    }

    #[cfg(test)]
    fn unclaimed_cost_usd_micros(&self) -> u64 {
        self.lock().unclaimed_cost_usd_micros
    }

    #[cfg(test)]
    pub fn system_prompt(&self) -> Option<String> {
        self.lock().system_prompt.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AdvisorSessionState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn request(&self, budget: TranscriptBudget) -> Result<AdvisorRequest, ToolError> {
        let state = self.lock();
        let model = state
            .model
            .clone()
            .ok_or_else(|| execution_error(NO_MODEL_MESSAGE))?;
        let session = state
            .session
            .as_ref()
            .ok_or_else(|| execution_error(NO_SESSION_MESSAGE))?;
        // Live history, not committed history: the advisor is called from
        // inside the turn it must review.
        let messages = session.live_history();
        Ok(AdvisorRequest {
            model,
            session_id: session.id().clone(),
            transcript: transcript::render_transcript(
                state.system_prompt.as_deref(),
                &messages,
                budget,
            ),
        })
    }
}

#[derive(Debug)]
struct AdvisorRequest {
    model: InternalAgentModelConfig,
    session_id: SessionId,
    transcript: String,
}

pub(crate) struct AdvisorTool {
    store: AdvisorSessionStore,
    budget: TranscriptBudget,
}

impl AdvisorTool {
    pub(crate) fn new(store: AdvisorSessionStore, budget: TranscriptBudget) -> Self {
        Self { store, budget }
    }
}

impl SdkTool for AdvisorTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: TOOL_NAME.into(),
            description: TOOL_DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    // The advisor reviews the turn it was called from, which only the
    // published in-flight history contains.
    fn reads_live_history(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let workspace_path = context
                .workspace_root()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| execution_error(NO_WORKSPACE_MESSAGE))?;
            let request = self.store.request(self.budget)?;
            let (advice, usage) =
                consult_advisor(request, workspace_path, context.cancellation().clone()).await?;
            // Note before the empty-guidance check: the provider already spent.
            self.store.note_usage(&usage);
            if advice.is_empty() {
                return Err(execution_error(NO_GUIDANCE_MESSAGE));
            }
            Ok(ToolOutput::text(advice)
                .metadata(ToolMetadata::new().operation(OperationKind::Read)))
        })
    }
}

/// Runs the advisor and returns its guidance plus provider usage.
///
/// Every failure comes back as a tool error, never as a run failure, so a
/// broken advisor leaves the executor's turn intact. Successful provider runs
/// (including empty text) return usage so the caller can fold cost into the
/// parent session total.
async fn consult_advisor(
    request: AdvisorRequest,
    workspace_path: PathBuf,
    cancellation: CancellationToken,
) -> Result<(String, ModelUsage), ToolError> {
    let AdvisorRequest {
        model,
        session_id,
        transcript,
    } = request;
    let usage_recording = crate::usage::default_recording().await;
    let started = run_one_shot_agent(
        OneShotAgentRequest {
            definition: internal_definition(ADVISOR_AGENT_ID),
            usage_purpose: USAGE_PURPOSE,
            provider_name: &model.provider,
            model: &model.model,
            auth: &model.auth,
            reasoning: Some(advisor_effective_reasoning(&model)),
            input: transcript,
            cancellation,
            session_id: &session_id,
            workspace_path: &workspace_path,
        },
        usage_recording,
    )
    .map_err(|error| {
        execution_error(format!(
            "advisor model {} could not start: {error}. Choose a rho-runtime advisor model with /advisor.",
            rho_providers::provider::model_reference(&model.provider, &model.model)
        ))
    })?;
    let result = started
        .await
        .map_err(|error| execution_error(format!("the advisor request failed: {error}")))?;
    let advice = result.texts.join("\n").trim().to_owned();
    Ok((advice, result.usage))
}

fn execution_error(message: impl Into<String>) -> ToolError {
    ToolError::new(ToolErrorKind::Execution, message)
}

#[cfg(test)]
#[path = "advisor_tests.rs"]
mod tests;
