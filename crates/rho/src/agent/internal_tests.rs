use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;

use super::{
    carry_internal_agent_reasoning, effective_internal_agent_reasoning,
    internal_agent_accepts_claude_runtime, internal_agent_requires_model, ADVISOR_AGENT_ID,
    SESSION_TITLE_AGENT_ID,
};
use crate::config::InternalAgentModelConfig;

fn selection(
    provider: &str,
    model: &str,
    reasoning: Option<ReasoningLevel>,
) -> InternalAgentModelConfig {
    let mut config = InternalAgentModelConfig::new(provider.into(), model.into(), "api-key".into());
    config.reasoning = reasoning;
    config
}

// Covers: unset override keeps the reserved definition default.
// Owner: internal agent reasoning
#[test]
fn effective_reasoning_defaults_to_the_definition_level() {
    assert_eq!(
        effective_internal_agent_reasoning(
            ADVISOR_AGENT_ID,
            &selection("openai", "gpt-test", None)
        ),
        ReasoningLevel::Medium
    );
    assert_eq!(
        effective_internal_agent_reasoning(
            SESSION_TITLE_AGENT_ID,
            &selection("openai", "gpt-test", None)
        ),
        ReasoningLevel::Low
    );
}

// Covers: the permission classifier cannot fall back to executor model or Claude runtime
// Owner: internal agent registry
#[test]
fn permission_classifier_requires_own_rho_model_with_off_reasoning() {
    let id = "permission-classifier";
    assert!(internal_agent_requires_model(id));
    assert!(!internal_agent_accepts_claude_runtime(id));
    assert_eq!(
        effective_internal_agent_reasoning(id, &selection("openai", "gpt-test", None)),
        ReasoningLevel::Off
    );
}

// Covers: an explicit override wins over the definition default.
// Owner: internal agent reasoning
#[test]
fn effective_reasoning_override_wins() {
    assert_eq!(
        effective_internal_agent_reasoning(
            ADVISOR_AGENT_ID,
            &selection("openai", "gpt-test", Some(ReasoningLevel::High))
        ),
        ReasoningLevel::High
    );
}

// Covers: model select keeps None when the user never set reasoning.
// Owner: internal agent reasoning
#[test]
fn carry_reasoning_leaves_unset_overrides_unset() {
    let next = selection("openai", "gpt-next", None);
    let previous = selection("openai", "gpt-prev", None);
    assert_eq!(carry_internal_agent_reasoning(&next, Some(&previous)), None);
    assert_eq!(carry_internal_agent_reasoning(&next, None), None);
}

// Covers: an explicit previous override is carried onto the next model.
// Owner: internal agent reasoning
#[test]
fn carry_reasoning_keeps_an_explicit_override() {
    let next = selection("openai", "gpt-next", None);
    let previous = selection("anthropic", "claude-prev", Some(ReasoningLevel::High));
    assert_eq!(
        carry_internal_agent_reasoning(&next, Some(&previous)),
        Some(ReasoningLevel::High)
    );
}
