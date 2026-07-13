use std::str::FromStr;

use super::ReasoningLevel;

#[test]
fn cycles_through_all_reasoning_levels() {
    let mut level = ReasoningLevel::Off;
    let mut levels = Vec::new();

    for _ in 0..7 {
        level = level.next();
        levels.push(level);
    }

    assert_eq!(
        levels,
        vec![
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
            ReasoningLevel::Max,
            ReasoningLevel::Off,
        ]
    );
}

#[test]
fn maps_reasoning_levels_to_provider_effort() {
    assert_eq!(ReasoningLevel::Off.effort(), None);
    assert_eq!(ReasoningLevel::Minimal.effort(), Some("minimal"));
    assert_eq!(ReasoningLevel::Low.effort(), Some("low"));
    assert_eq!(ReasoningLevel::Medium.effort(), Some("medium"));
    assert_eq!(ReasoningLevel::High.effort(), Some("high"));
    assert_eq!(ReasoningLevel::Xhigh.effort(), Some("xhigh"));
    assert_eq!(ReasoningLevel::Max.effort(), Some("max"));
}

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
fn falls_back_to_full_cycle_without_capability_metadata() {
    assert_eq!(
        ReasoningLevel::Off.next_supported(None),
        ReasoningLevel::Minimal
    );
}

#[test]
fn normalizes_to_nearest_supported_level_without_exceeding_selection() {
    let supported = [
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::High,
        ReasoningLevel::Xhigh,
    ];

    assert_eq!(
        ReasoningLevel::Minimal.normalize(Some(&supported)),
        ReasoningLevel::Off
    );
    assert_eq!(
        ReasoningLevel::Max.normalize(Some(&supported)),
        ReasoningLevel::Xhigh
    );
}

#[test]
fn keeps_selection_without_capability_metadata() {
    assert_eq!(ReasoningLevel::Max.normalize(None), ReasoningLevel::Max);
}

#[test]
fn parses_and_displays_max_reasoning_level() {
    let level = ReasoningLevel::from_str("MAX").unwrap();

    assert_eq!(level, ReasoningLevel::Max);
    assert_eq!(level.to_string(), "max");
}
