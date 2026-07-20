use super::*;
use rho_providers::model::ReasoningLevelSet;

fn exact() -> ReasoningCapabilities {
    ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
        ReasoningLevel::Low,
        ReasoningLevel::High,
    ]))
}

#[test]
fn model_switch_rejects_an_unsupported_explicit_level() {
    assert_eq!(
        resolve_model_switch_reasoning(
            &exact(),
            ReasoningLevel::Off,
            ReasoningRequestSource::Explicit,
        )
        .expect_err("explicit Off must not be normalized"),
        ReasoningLevel::Off
    );
}

#[test]
fn model_switch_preserves_explicit_provenance_when_capabilities_are_unknown() {
    let resolved = resolve_model_switch_reasoning(
        &ReasoningCapabilities::Unknown,
        ReasoningLevel::Off,
        ReasoningRequestSource::Explicit,
    )
    .unwrap();

    assert_eq!(resolved.effective, ReasoningLevel::Off);
    assert_eq!(resolved.source, ReasoningRequestSource::Explicit);
}

#[test]
fn model_switch_normalizes_only_persisted_values() {
    let resolved = resolve_model_switch_reasoning(
        &exact(),
        ReasoningLevel::Medium,
        ReasoningRequestSource::PersistedOrDefault,
    )
    .unwrap();

    assert_eq!(resolved.effective, ReasoningLevel::High);
    assert_eq!(resolved.source, ReasoningRequestSource::PersistedOrDefault);
}

#[test]
fn model_switch_retains_the_global_preference_for_fixed_models() {
    let resolved = resolve_model_switch_reasoning(
        &ReasoningCapabilities::NotConfigurable,
        ReasoningLevel::High,
        ReasoningRequestSource::Explicit,
    )
    .unwrap();

    assert_eq!(resolved.effective, ReasoningLevel::High);
    assert_eq!(resolved.source, ReasoningRequestSource::Explicit);
}

#[test]
fn explicit_change_during_fetch_is_rejected_and_restored() {
    let resolved = resolve_fetched_reasoning(
        &exact(),
        ReasoningLevel::Medium,
        Some((
            ReasoningLevel::Low,
            ReasoningRequestSource::PersistedOrDefault,
        )),
    );

    assert_eq!(resolved.effective, ReasoningLevel::Low);
    assert_eq!(resolved.rejected, Some(ReasoningLevel::Medium));
}

#[test]
fn initial_explicit_value_remains_explicit_when_metadata_arrives() {
    let resolved = resolve_fetched_reasoning(
        &exact(),
        ReasoningLevel::Medium,
        Some((ReasoningLevel::Medium, ReasoningRequestSource::Explicit)),
    );

    assert_eq!(resolved.effective, ReasoningLevel::High);
    assert_eq!(resolved.rejected, Some(ReasoningLevel::Medium));
}

#[test]
fn unchanged_persisted_value_is_normalized_after_fetch() {
    let resolved = resolve_fetched_reasoning(
        &exact(),
        ReasoningLevel::Medium,
        Some((
            ReasoningLevel::Medium,
            ReasoningRequestSource::PersistedOrDefault,
        )),
    );

    assert_eq!(resolved.effective, ReasoningLevel::High);
    assert_eq!(resolved.rejected, None);
}

#[test]
fn fixed_models_retain_the_global_reasoning_preference() {
    let resolved = resolve_fetched_reasoning(
        &ReasoningCapabilities::NotConfigurable,
        ReasoningLevel::High,
        Some((
            ReasoningLevel::High,
            ReasoningRequestSource::PersistedOrDefault,
        )),
    );

    assert_eq!(resolved.effective, ReasoningLevel::High);
    assert_eq!(resolved.rejected, None);
}
