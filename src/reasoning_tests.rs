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
    assert_eq!(ReasoningLevel::Minimal.effort(), Some("low"));
    assert_eq!(ReasoningLevel::Low.effort(), Some("low"));
    assert_eq!(ReasoningLevel::Medium.effort(), Some("medium"));
    assert_eq!(ReasoningLevel::High.effort(), Some("high"));
    assert_eq!(ReasoningLevel::Xhigh.effort(), Some("xhigh"));
    assert_eq!(ReasoningLevel::Max.effort(), Some("max"));
}

#[test]
fn skips_unsupported_max_effort_for_older_codex_models() {
    assert_eq!(
        ReasoningLevel::Xhigh.next_for_model("openai-codex", "gpt-5.5"),
        ReasoningLevel::Off
    );
    assert_eq!(
        ReasoningLevel::Max.for_model("openai-codex", "gpt-5.5"),
        ReasoningLevel::Xhigh
    );
}

#[test]
fn enables_max_effort_for_gpt_5_6_codex_models() {
    for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        assert_eq!(
            ReasoningLevel::Xhigh.next_for_model("openai-codex", model),
            ReasoningLevel::Max
        );
        assert_eq!(
            ReasoningLevel::Max.for_model("openai-codex", model),
            ReasoningLevel::Max
        );
    }
}

#[test]
fn remaps_unsupported_max_effort_before_building_provider() {
    assert_eq!(
        crate::model::provider::mapped_reasoning_for_model(
            "openai-codex",
            "gpt-5.5",
            ReasoningLevel::Max,
        ),
        ReasoningLevel::Xhigh
    );
    assert_eq!(
        crate::model::provider::mapped_reasoning_for_model(
            "openai-codex",
            "gpt-5.6-sol",
            ReasoningLevel::Max,
        ),
        ReasoningLevel::Max
    );
}

#[test]
fn parses_and_displays_max_reasoning_level() {
    let level = ReasoningLevel::from_str("MAX").unwrap();

    assert_eq!(level, ReasoningLevel::Max);
    assert_eq!(level.to_string(), "max");
}
