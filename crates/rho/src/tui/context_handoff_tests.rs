use pretty_assertions::assert_eq;
use rho_sdk::model::handoff::HandoffReport;

use super::{
    decision_from_value, ContextHandoffDecision, ContextHandoffImpact, ContextHandoffKind,
    ACTION_COMPACT, ACTION_CONTINUE, ACTION_USE_SOURCE,
};

fn impact(omissions: usize, can_compact: bool, offer_use_source: bool) -> ContextHandoffImpact {
    ContextHandoffImpact {
        source_label: "openai-codex/gpt-5.6-sol".into(),
        target_label: "xai/grok-4".into(),
        omissions: HandoffReport {
            omitted_provider_context: omissions,
            omitted_kinds: if omissions == 0 {
                Vec::new()
            } else {
                vec!["openai_response_output_item".into()]
            },
        },
        can_compact,
        offer_use_source,
    }
}

#[test]
fn resume_offers_source_model_when_available() {
    let choice = impact(3, true, true)
        .choice(ContextHandoffKind::Resume)
        .unwrap();

    assert_eq!(
        choice
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        vec![ACTION_USE_SOURCE, ACTION_COMPACT, ACTION_CONTINUE]
    );
}

#[test]
fn parses_decisions() {
    assert_eq!(
        decision_from_value(ACTION_USE_SOURCE),
        Some(ContextHandoffDecision::UseSourceModel)
    );
    assert_eq!(
        decision_from_value(ACTION_COMPACT),
        Some(ContextHandoffDecision::CompactThenContinue)
    );
    assert_eq!(
        decision_from_value(ACTION_CONTINUE),
        Some(ContextHandoffDecision::ContinueDirect)
    );
    assert_eq!(decision_from_value("nope"), None);
}

#[test]
fn compact_then_continue_pipeline_order() {
    // Decision mapping for the unified executor:
    // UseSource => source -> materialize
    // CompactThenContinue => source? -> materialize -> compact -> target?
    // ContinueDirect => materialize -> target?
    assert_eq!(
        decision_from_value(ACTION_COMPACT),
        Some(ContextHandoffDecision::CompactThenContinue)
    );
    assert_eq!(
        decision_from_value(ACTION_USE_SOURCE),
        Some(ContextHandoffDecision::UseSourceModel)
    );
    assert_eq!(
        decision_from_value(ACTION_CONTINUE),
        Some(ContextHandoffDecision::ContinueDirect)
    );
}
