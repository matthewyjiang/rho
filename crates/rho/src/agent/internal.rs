use std::{collections::BTreeSet, sync::LazyLock};

use rho_providers::{
    model::{models_dev, ReasoningCapabilities, ReasoningRequestSource},
    reasoning::ReasoningLevel,
};

use crate::config::{InternalAgentModelConfig, InternalAgentTarget};

use super::{AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy, ToolPolicy};

pub(crate) const SESSION_TITLE_AGENT_ID: &str = "session-title";
pub(crate) const GOAL_JUDGE_AGENT_ID: &str = "goal-judge";
pub(crate) const ADVISOR_AGENT_ID: &str = "advisor";
pub(crate) const PERMISSION_CLASSIFIER_AGENT_ID: &str = "permission-classifier";

pub(crate) const ADVISOR_PROMPT: &str = "You are a senior advisor reviewing another AI coding agent's live work session. You receive the full session transcript: the agent's system prompt, the user's requests, every tool call and result, and the agent's reasoning so far.\n\nProvide strategic guidance for the agent's next steps:\n- Identify the core difficulty or the decision the agent is facing.\n- Recommend a concrete plan or course correction.\n- Flag risks, failure modes, or wrong assumptions the agent has not ruled out.\n\nBe direct and specific. Reference concrete files, commands, and evidence from the transcript. Do not restate the transcript. Do not write large code blocks; describe the approach. Keep your guidance under 500 words.";

/// One reserved internal agent: its definition plus the model policy that only
/// internal agents have, declared side by side so the table is the single
/// source of truth.
struct InternalAgent {
    definition: AgentDefinition,
    /// Runs only with an explicitly configured model; no conversation-model
    /// fallback. The definition's `ModelPolicy` cannot express this, because
    /// internal agents resolve their models from config, not from binding.
    requires_own_model: bool,
    /// May delegate to the Claude Code CLI instead of running on Rho's own
    /// stack. Only agents whose whole product is free-form text qualify: a
    /// delegated run costs a process spawn and returns no structured output.
    accepts_claude_runtime: bool,
}

static INTERNAL_AGENTS: LazyLock<Vec<InternalAgent>> = LazyLock::new(|| {
    vec![
        InternalAgent {
            definition: AgentDefinition {
                id: AgentId::new(SESSION_TITLE_AGENT_ID).expect("valid internal agent ID"),
                description: "Internal agent that names chat sessions. Reserved; cannot be overridden or delegated."
                    .to_string(),
                prompt: PromptPolicy::Replace(crate::tui::SESSION_TITLE_PROMPT.into()),
                runtime: AgentRuntimeSpec::Rho {
                    tools: ToolPolicy::Allow(BTreeSet::new()),
                    model: ModelPolicy::Inherit,
                    reasoning: Some(ReasoningLevel::Low),
                },
            },
            requires_own_model: false,
            accepts_claude_runtime: false,
        },
        InternalAgent {
            definition: AgentDefinition {
                id: AgentId::new(GOAL_JUDGE_AGENT_ID).expect("valid internal agent ID"),
                description: "Internal agent that evaluates goal completion. Reserved; cannot be overridden or delegated."
                    .to_string(),
                prompt: PromptPolicy::Replace(crate::tui::GOAL_JUDGE_PROMPT.into()),
                runtime: AgentRuntimeSpec::Rho {
                    tools: ToolPolicy::Allow(BTreeSet::new()),
                    model: ModelPolicy::Inherit,
                    reasoning: Some(ReasoningLevel::Low),
                },
            },
            requires_own_model: false,
            accepts_claude_runtime: false,
        },
        InternalAgent {
            definition: AgentDefinition {
                id: AgentId::new(ADVISOR_AGENT_ID).expect("valid internal agent ID"),
                description: "Internal agent that reviews the session and advises the executor. Reserved; cannot be overridden, and runs on Rho or Claude Code."
                    .to_string(),
                prompt: PromptPolicy::Replace(ADVISOR_PROMPT.into()),
                runtime: AgentRuntimeSpec::Rho {
                    tools: ToolPolicy::Allow(BTreeSet::new()),
                    // Unused: the advisor requires its own model (below).
                    model: ModelPolicy::Inherit,
                    reasoning: Some(ReasoningLevel::Medium),
                },
            },
            // An advisor that mirrors the executor adds nothing, so it stays
            // unconfigured until a model is selected.
            requires_own_model: true,
            // The advisor returns prose, so Claude Code's harness can produce
            // it as well as Rho's own loop can.
            accepts_claude_runtime: true,
        },
        InternalAgent {
            definition: AgentDefinition {
                id: AgentId::new(PERMISSION_CLASSIFIER_AGENT_ID)
                    .expect("valid internal agent ID"),
                description: "Internal agent that classifies pending permission requests. Reserved; cannot be overridden or delegated."
                    .to_string(),
                prompt: PromptPolicy::Replace(
                    crate::permission_classifier::CLASSIFIER_PROMPT.into(),
                ),
                runtime: AgentRuntimeSpec::Rho {
                    tools: ToolPolicy::Allow(BTreeSet::new()),
                    // Unused: the permission classifier requires its own model
                    // and must not fall back to the executor.
                    model: ModelPolicy::Inherit,
                    reasoning: Some(ReasoningLevel::Low),
                },
            },
            requires_own_model: true,
            accepts_claude_runtime: false,
        },
    ]
});

static INTERNAL_DEFINITIONS: LazyLock<Vec<AgentDefinition>> = LazyLock::new(|| {
    INTERNAL_AGENTS
        .iter()
        .map(|agent| agent.definition.clone())
        .collect()
});

pub(crate) fn internal_definitions() -> &'static [AgentDefinition] {
    &INTERNAL_DEFINITIONS
}

/// Whether an internal agent needs its own configured model, as declared in
/// the internal agent table. Unknown ids follow the common case and fall back
/// to the conversation model.
pub(crate) fn internal_agent_requires_model(id: &str) -> bool {
    INTERNAL_AGENTS
        .iter()
        .any(|agent| agent.requires_own_model && agent.definition.id.as_str() == id)
}

/// Whether an internal agent may delegate to the Claude Code CLI. Unknown ids
/// stay on Rho's own runtime.
pub(crate) fn internal_agent_accepts_claude_runtime(id: &str) -> bool {
    INTERNAL_AGENTS
        .iter()
        .any(|agent| agent.accepts_claude_runtime && agent.definition.id.as_str() == id)
}

pub(crate) fn internal_definition(id: &str) -> &'static AgentDefinition {
    internal_definitions()
        .iter()
        .find(|definition| definition.id.as_str() == id)
        .expect("registered internal agent definition")
}

pub(crate) fn is_internal_agent_id(id: &AgentId) -> bool {
    internal_definitions()
        .iter()
        .any(|definition| definition.id == *id)
}

/// Reasoning levels a selection can take.
///
/// Rho selections read the model's advertised controls. Claude Code resolves
/// the model itself and never appears in the models.dev catalog, so a
/// delegating selection uses Claude's fixed `--effort` ladder instead.
pub(crate) fn internal_agent_reasoning_capabilities(
    selection: &InternalAgentModelConfig,
) -> ReasoningCapabilities {
    match &selection.target {
        InternalAgentTarget::Rho(rho) => {
            models_dev::current_reasoning_capabilities(&rho.provider, &rho.model)
        }
        InternalAgentTarget::ClaudeCli { .. } => CLAUDE_REASONING_CAPABILITIES.clone(),
    }
}

/// Claude's fixed `--effort` ladder as selection capabilities. Built once:
/// picker render paths ask for this on every frame.
static CLAUDE_REASONING_CAPABILITIES: LazyLock<ReasoningCapabilities> = LazyLock::new(|| {
    ReasoningCapabilities::Levels(crate::claude_runtime::spawn::CLAUDE_EFFORT_LEVELS.clone())
});

/// Reasoning level an internal-agent one-shot will use for `selection`.
///
/// Explicit config wins. Otherwise the reserved definition default applies.
/// Persisted/default values are normalized onto the selection's capabilities so
/// a carried level never rejects at call time.
pub(crate) fn effective_internal_agent_reasoning(
    id: &str,
    selection: &InternalAgentModelConfig,
) -> ReasoningLevel {
    let requested = selection.reasoning.unwrap_or_else(|| {
        internal_definition(id)
            .reasoning()
            .expect("internal agent definitions set a reasoning level")
    });
    let capabilities = internal_agent_reasoning_capabilities(selection);
    match capabilities.resolve(requested, ReasoningRequestSource::PersistedOrDefault) {
        // One-shot still needs a concrete level; the provider ignores it when
        // the model has no selectable control.
        rho_providers::model::ReasoningResolution::NotConfigurable => requested,
        resolution => resolution.effective().unwrap_or(requested),
    }
}

/// Reasoning override to store after selecting a new internal-agent model.
///
/// Only an **explicit** previous override is carried, and only when the new
/// selection is reasoning-configurable. `None` keeps the definition default.
pub(crate) fn carry_internal_agent_reasoning(
    selection: &InternalAgentModelConfig,
    previous: Option<&InternalAgentModelConfig>,
) -> Option<ReasoningLevel> {
    let capabilities = internal_agent_reasoning_capabilities(selection);
    if capabilities == ReasoningCapabilities::NotConfigurable {
        return None;
    }
    let requested = previous.and_then(|prev| prev.reasoning)?;
    capabilities
        .resolve(requested, ReasoningRequestSource::PersistedOrDefault)
        .effective()
}

#[cfg(test)]
#[path = "internal_tests.rs"]
mod tests;
