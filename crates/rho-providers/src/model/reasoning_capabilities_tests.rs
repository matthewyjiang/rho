use super::*;

#[test]
fn level_sets_are_sorted_and_deduplicated() {
    let levels = ReasoningLevelSet::new(vec![
        ReasoningLevel::Max,
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::Low,
    ]);

    assert_eq!(
        levels.levels(),
        &[
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::Max
        ]
    );
}

#[test]
fn exact_levels_do_not_inject_off_into_cycling_or_normalization() {
    let capabilities = ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ]));

    assert_eq!(
        capabilities.next_level(ReasoningLevel::High),
        ReasoningLevel::Low
    );
    assert_eq!(
        capabilities.resolve(
            ReasoningLevel::Off,
            ReasoningRequestSource::PersistedOrDefault,
        ),
        ReasoningResolution::Normalized {
            requested: ReasoningLevel::Off,
            effective: ReasoningLevel::Low,
        }
    );
}

#[test]
fn unsupported_explicit_choices_are_not_silently_changed() {
    let capabilities = ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
        ReasoningLevel::Low,
        ReasoningLevel::High,
    ]));
    let resolution = capabilities.resolve(ReasoningLevel::Off, ReasoningRequestSource::Explicit);

    assert_eq!(
        resolution,
        ReasoningResolution::UnsupportedExplicit(ReasoningLevel::Off)
    );
    assert_eq!(resolution.effective(), None);
}

#[test]
fn not_configurable_models_have_no_effective_or_selectable_level() {
    let capabilities = ReasoningCapabilities::NotConfigurable;

    assert_eq!(
        capabilities.resolve(
            ReasoningLevel::Medium,
            ReasoningRequestSource::PersistedOrDefault,
        ),
        ReasoningResolution::NotConfigurable
    );
    assert_eq!(
        capabilities.next_level(ReasoningLevel::Medium),
        ReasoningLevel::Medium
    );
    assert_eq!(ReasoningResolution::NotConfigurable.effective(), None);
}

#[test]
fn empty_level_sets_are_rejected_during_deserialization() {
    let error =
        serde_json::from_str::<ReasoningCapabilities>(r#"{"levels":{"levels":[]}}"#).unwrap_err();

    assert!(error.to_string().contains("at least one level"));
}

#[test]
#[should_panic(expected = "reasoning level sets must contain at least one level")]
fn empty_level_sets_cannot_be_constructed() {
    ReasoningLevelSet::new(Vec::new());
}

#[test]
fn legacy_unrestricted_capabilities_deserialize_as_unknown() {
    assert_eq!(
        serde_json::from_str::<ReasoningCapabilities>(r#""unrestricted""#).unwrap(),
        ReasoningCapabilities::Unknown
    );
}

#[test]
fn deserialization_restores_level_set_invariants() {
    let capabilities: ReasoningCapabilities =
        serde_json::from_str(r#"{"levels":{"levels":["max","off","low","low"]}}"#).unwrap();

    assert_eq!(
        capabilities.levels(),
        Some(
            [
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::Max,
            ]
            .as_slice()
        )
    );
}
