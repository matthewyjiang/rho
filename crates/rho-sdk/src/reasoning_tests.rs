use super::ReasoningLevel;

#[test]
fn cycles_only_through_supported_levels() {
    let supported = [
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ];

    assert_eq!(
        ReasoningLevel::Off.next_supported(Some(&supported)),
        ReasoningLevel::Low
    );
    assert_eq!(
        ReasoningLevel::High.next_supported(Some(&supported)),
        ReasoningLevel::Off
    );
}

#[test]
fn normalizes_unsupported_levels_up_without_disabling_reasoning() {
    let supported = [
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::High,
        ReasoningLevel::Max,
    ];

    assert_eq!(
        ReasoningLevel::Minimal.normalize(Some(&supported)),
        ReasoningLevel::Low
    );
    assert_eq!(
        ReasoningLevel::Medium.normalize(Some(&supported)),
        ReasoningLevel::High
    );
    assert_eq!(
        ReasoningLevel::Xhigh.normalize(Some(&supported)),
        ReasoningLevel::Max
    );
    assert_eq!(
        ReasoningLevel::Off.normalize(Some(&[ReasoningLevel::Low])),
        ReasoningLevel::Off
    );
}

#[test]
fn normalizes_above_the_highest_supported_non_off_level_down() {
    let supported = [
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::High,
        ReasoningLevel::Xhigh,
    ];

    assert_eq!(
        ReasoningLevel::Max.normalize(Some(&supported)),
        ReasoningLevel::Xhigh
    );
}

#[test]
fn codex_levels_without_minimal_round_up_to_low() {
    let supported = [
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
        ReasoningLevel::Xhigh,
    ];

    assert_eq!(
        ReasoningLevel::Minimal.normalize(Some(&supported)),
        ReasoningLevel::Low
    );
}
