use pretty_assertions::assert_eq;

use super::*;
use crate::model::ModelMetadata;

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
    for model in ["grok-4.5", "grok-4.6"] {
        let mandatory = XaiReasoningProfile::from_metadata(model, None);
        assert_eq!(mandatory.effort(ReasoningLevel::Off), None);
        assert_eq!(mandatory.effort(ReasoningLevel::High), Some("high"));
    }

    let optional = XaiReasoningProfile::from_metadata("grok-4.3", None);
    assert_eq!(optional.effort(ReasoningLevel::Off), Some("none"));

    for model in ["grok-build-0.1", "grok-composer-2.5-fast", "future-grok"] {
        let profile = XaiReasoningProfile::from_metadata(model, None);
        assert_eq!(profile.effort(ReasoningLevel::High), None);
    }

    let fixed = XaiReasoningProfile::not_configurable();
    assert_eq!(fixed.effort(ReasoningLevel::High), None);
}

// Covers: models.dev levels clamp generic Rho values; the offline Mandatory
// fallback must not emit minimal/max once a catalog row is known.
// Owner: xAI reasoning profile
#[test]
fn catalog_metadata_clamps_flagship_grok_generic_levels() {
    let metadata = ModelMetadata {
        supported_reasoning_levels: Some(vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ]),
        reasoning_capabilities_known: true,
        reasoning_metadata_complete: true,
        ..ModelMetadata::default()
    };

    for model in ["grok-4.5", "grok-4.6"] {
        let profile = XaiReasoningProfile::from_metadata(model, Some(metadata.clone()));
        assert_eq!(profile.effort(ReasoningLevel::Minimal), Some("low"));
        assert_eq!(profile.effort(ReasoningLevel::Max), Some("high"));
        assert_eq!(profile.effort(ReasoningLevel::Xhigh), Some("high"));
        assert_eq!(profile.effort(ReasoningLevel::Off), Some("low"));
        assert_eq!(profile.effort(ReasoningLevel::High), Some("high"));
    }
}

// Covers: optional Grok catalog Off stays wire "none"; Omit off-behavior must
// not drop the field once a models.dev row is present.
// Owner: xAI reasoning profile
#[test]
fn catalog_optional_grok_encodes_off_as_none() {
    let metadata = ModelMetadata {
        supported_reasoning_levels: Some(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
            ReasoningLevel::High,
        ]),
        reasoning_capabilities_known: true,
        reasoning_metadata_complete: true,
        ..ModelMetadata::default()
    };

    let profile = XaiReasoningProfile::from_metadata("grok-4.3", Some(metadata));
    assert_eq!(profile.effort(ReasoningLevel::Off), Some("none"));
    assert_eq!(profile.effort(ReasoningLevel::High), Some("high"));
}
