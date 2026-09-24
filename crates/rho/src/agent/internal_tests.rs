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

// Covers: an unset override keeps the reserved definition default; an
// explicit override wins over it.
// Owner: internal agent reasoning
#[test]
fn effective_reasoning_prefers_the_override_over_the_definition_level() {
    for (case, id, reasoning, expected) in [
        (
            "advisor default",
            ADVISOR_AGENT_ID,
            None,
            ReasoningLevel::Medium,
        ),
        (
            "title default",
            SESSION_TITLE_AGENT_ID,
            None,
            ReasoningLevel::Low,
        ),
        (
            "advisor override",
            ADVISOR_AGENT_ID,
            Some(ReasoningLevel::High),
            ReasoningLevel::High,
        ),
    ] {
        assert_eq!(
            effective_internal_agent_reasoning(id, &selection("openai", "gpt-test", reasoning)),
            expected,
            "{case}"
        );
    }
}

// Covers: the permission classifier cannot fall back to executor model or Claude runtime
// Owner: internal agent registry
#[test]
fn permission_classifier_requires_own_rho_model_with_low_reasoning() {
    let id = "permission-classifier";
    assert!(internal_agent_requires_model(id));
    assert!(!internal_agent_accepts_claude_runtime(id));
    assert_eq!(
        effective_internal_agent_reasoning(id, &selection("openai", "gpt-test", None)),
        ReasoningLevel::Low
    );
}

// Covers: model select carries an explicit previous override onto the next
// model and keeps None when the user never set reasoning.
// Owner: internal agent reasoning
#[test]
fn carry_reasoning_keeps_only_an_explicit_override() {
    let next = selection("openai", "gpt-next", None);
    let unset = selection("openai", "gpt-prev", None);
    let explicit = selection("anthropic", "claude-prev", Some(ReasoningLevel::High));
    for (case, previous, expected) in [
        ("no previous", None, None),
        ("unset previous", Some(&unset), None),
        (
            "explicit previous",
            Some(&explicit),
            Some(ReasoningLevel::High),
        ),
    ] {
        assert_eq!(
            carry_internal_agent_reasoning(&next, previous),
            expected,
            "{case}"
        );
    }
}
