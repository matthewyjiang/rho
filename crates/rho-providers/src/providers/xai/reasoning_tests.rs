use pretty_assertions::assert_eq;

use super::*;

#[test]
fn exact_mandatory_reasoning_clamps_requests_and_never_emits_none() {
    let profile = XaiReasoningProfile::exact([
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ]);

    for (level, expected) in [
        (ReasoningLevel::Off, "low"),
        (ReasoningLevel::Minimal, "low"),
        (ReasoningLevel::Low, "low"),
        (ReasoningLevel::Medium, "medium"),
        (ReasoningLevel::High, "high"),
        (ReasoningLevel::Xhigh, "high"),
        (ReasoningLevel::Max, "high"),
    ] {
        assert_eq!(profile.effort(level), Some(expected));
    }
}

#[test]
fn exact_optional_reasoning_encodes_off_as_none() {
    let profile = XaiReasoningProfile::exact([
        ReasoningLevel::Off,
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ]);

    assert_eq!(profile.effort(ReasoningLevel::Off), Some("none"));
}

#[test]
fn unknown_metadata_does_not_synthesize_reasoning_and_non_configurable_omits_it() {
    let unknown = XaiReasoningProfile::from_metadata("grok-4.5", None);
    assert_eq!(unknown.effort(ReasoningLevel::Off), None);
    assert_eq!(unknown.effort(ReasoningLevel::High), Some("high"));

    let optional = XaiReasoningProfile::from_metadata("grok-4.3", None);
    assert_eq!(optional.effort(ReasoningLevel::Off), Some("none"));

    for model in ["grok-build-0.1", "grok-composer-2.5-fast", "future-grok"] {
        let profile = XaiReasoningProfile::from_metadata(model, None);
        assert_eq!(profile.effort(ReasoningLevel::High), None);
    }

    let fixed = XaiReasoningProfile::not_configurable();
    assert_eq!(fixed.effort(ReasoningLevel::High), None);
}
