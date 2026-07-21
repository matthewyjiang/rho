use std::{collections::BTreeSet, sync::LazyLock};

use rho_providers::reasoning::ReasoningLevel;

use super::{AgentDefinition, AgentId, ModelPolicy, PromptPolicy, ToolPolicy};

pub(crate) const SESSION_TITLE_AGENT_ID: &str = "session-title";
pub(crate) const GOAL_JUDGE_AGENT_ID: &str = "goal-judge";

static INTERNAL_DEFINITIONS: LazyLock<Vec<AgentDefinition>> = LazyLock::new(|| {
    vec![
        AgentDefinition {
            id: AgentId::new(SESSION_TITLE_AGENT_ID).expect("valid internal agent ID"),
            description: "Internal agent that names chat sessions. Reserved; cannot be overridden or delegated."
                .to_string(),
            prompt: PromptPolicy::Replace(crate::tui::SESSION_TITLE_PROMPT.into()),
            model: ModelPolicy::Inherit,
            tools: ToolPolicy::Allow(BTreeSet::new()),
            reasoning: Some(ReasoningLevel::Low),
        },
        AgentDefinition {
            id: AgentId::new(GOAL_JUDGE_AGENT_ID).expect("valid internal agent ID"),
            description: "Internal agent that evaluates goal completion. Reserved; cannot be overridden or delegated."
                .to_string(),
            prompt: PromptPolicy::Replace(crate::tui::GOAL_JUDGE_PROMPT.into()),
            model: ModelPolicy::Inherit,
            tools: ToolPolicy::Allow(BTreeSet::new()),
            reasoning: Some(ReasoningLevel::Low),
        },
    ]
});

pub(crate) fn internal_definitions() -> &'static [AgentDefinition] {
    &INTERNAL_DEFINITIONS
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

#[cfg(test)]
#[path = "internal_tests.rs"]
mod tests;
