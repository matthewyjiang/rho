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
fn deserialization_restores_level_set_invariants() {
    let capabilities: ReasoningCapabilities =
        serde_json::from_str(r#"{"levels":{"levels":["max","off","low","low"]}}"#).unwrap();

    assert_eq!(
        capabilities.levels(),
        Some(
            [
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::Max
            ]
            .as_slice()
        )
    );
}
