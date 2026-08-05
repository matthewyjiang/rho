use std::{collections::BTreeSet, sync::LazyLock};

use rho_providers::reasoning::ReasoningLevel;

use super::{AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy, ToolPolicy};

pub(crate) const SESSION_TITLE_AGENT_ID: &str = "session-title";
pub(crate) const GOAL_JUDGE_AGENT_ID: &str = "goal-judge";
pub(crate) const ADVISOR_AGENT_ID: &str = "advisor";

pub(crate) const ADVISOR_PROMPT: &str = "You are a senior advisor reviewing another AI coding agent's live work session. You receive the full session transcript: the agent's system prompt, the user's requests, every tool call and result, and the agent's reasoning so far.\n\nProvide strategic guidance for the agent's next steps:\n- Identify the core difficulty or the decision the agent is facing.\n- Recommend a concrete plan or course correction.\n- Flag risks, failure modes, or wrong assumptions the agent has not ruled out.\n\nBe direct and specific. Reference concrete files, commands, and evidence from the transcript. Do not restate the transcript. Do not write large code blocks; describe the approach. Keep your guidance under 500 words.";

static INTERNAL_DEFINITIONS: LazyLock<Vec<AgentDefinition>> = LazyLock::new(|| {
    vec![
        AgentDefinition {
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
        AgentDefinition {
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
        AgentDefinition {
            id: AgentId::new(ADVISOR_AGENT_ID).expect("valid internal agent ID"),
            description: "Internal agent that reviews the session and advises the executor. Reserved; cannot be overridden or delegated."
                .to_string(),
            prompt: PromptPolicy::Replace(ADVISOR_PROMPT.into()),
            runtime: AgentRuntimeSpec::Rho {
                tools: ToolPolicy::Allow(BTreeSet::new()),
                model: ModelPolicy::Inherit,
                reasoning: Some(ReasoningLevel::Medium),
            },
        },
    ]
});

pub(crate) fn internal_definitions() -> &'static [AgentDefinition] {
    &INTERNAL_DEFINITIONS
}

/// Whether an internal agent needs its own configured model.
///
/// Most internal agents fall back to the conversation model when the user has
/// not chosen one. The advisor does not: an advisor that mirrors the executor
/// adds nothing, so it stays unconfigured until a model is selected.
pub(crate) fn internal_agent_requires_model(id: &str) -> bool {
    id == ADVISOR_AGENT_ID
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
