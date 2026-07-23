use pretty_assertions::assert_eq;
use rho_sdk::model::handoff::HandoffReport;

use super::{
    decision_from_value, is_omission_notice, ContextHandoffDecision, ContextHandoffImpact,
    ContextHandoffKind, ACTION_COMPACT, ACTION_CONTINUE, ACTION_USE_SOURCE,
};

fn impact(
    omissions: usize,
    can_compact: bool,
    source_available: bool,
    cache_warm: bool,
) -> ContextHandoffImpact {
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
        source_model_available: source_available,
        cache_warm,
    }
}

#[test]
fn prompts_for_omissions_even_without_warm_cache() {
    assert!(impact(12, false, false, false).should_prompt());
    assert!(!impact(0, false, false, false).should_prompt());
    assert!(impact(0, true, false, true).should_prompt());
}

#[test]
fn model_switch_omission_options_are_honest_about_native_blocks() {
    let choice = impact(115, true, false, true)
        .choice(ContextHandoffKind::ModelSwitch)
        .unwrap();

    assert!(choice.description.contains("115 provider-native"));
    assert!(choice
        .description
        .contains("does not make native blocks sendable"));
    assert_eq!(
        choice
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        vec![ACTION_COMPACT, ACTION_CONTINUE]
    );
    assert!(choice.options[0]
        .detail
        .contains("still will not be sent to xai/grok-4"));
    assert!(choice.options[1]
        .detail
        .contains("115 native block(s) will not be sent"));
}

#[test]
fn resume_offers_source_model_when_available() {
    let choice = impact(3, true, true, false)
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
    assert!(choice.options[0]
        .label
        .contains("Resume with openai-codex/gpt-5.6-sol"));
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
fn omission_notices_are_recognized() {
    assert!(is_omission_notice(
        "omitted 115 incompatible provider-native context block(s) while resuming session (kinds: openai_response_output_item)"
    ));
    assert!(is_omission_notice(
        "model handoff omitted 2 nonportable provider context block(s): openai_response_output_item; assistant text, tool history, and reasoning summaries were preserved"
    ));
    assert!(!is_omission_notice("resumed session abc123"));
}
