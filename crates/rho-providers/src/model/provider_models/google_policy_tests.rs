use pretty_assertions::assert_eq;

use super::*;
use crate::reasoning::ReasoningLevel;

#[test]
fn text_chat_filter_hides_specialty_models() {
    assert!(is_text_chat_model("gemini-3.1-flash-lite"));
    assert!(is_text_chat_model("gemma-4-31b-it"));
    assert!(!is_text_chat_model("gemini-3.1-flash-image"));
    assert!(!is_text_chat_model("gemini-2.5-flash-preview-tts"));
    assert!(!is_text_chat_model("lyria-3-clip-preview"));
    assert!(!is_text_chat_model("nano-banana-pro-preview"));
}

#[test]
fn reasoning_capabilities_are_exact_for_known_families() {
    assert_eq!(
        reasoning_capabilities("gemini-3.1-flash-lite", Some(true)),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ]))
    );
    assert_eq!(
        reasoning_capabilities("gemini-3-pro-preview", Some(true)),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Low,
            ReasoningLevel::High,
        ]))
    );
    assert_eq!(
        reasoning_capabilities("gemini-3.1-pro-preview", Some(true)),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ]))
    );
    assert_eq!(
        reasoning_capabilities("gemini-2.5-flash", Some(true)),
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
            ReasoningLevel::Max,
        ]))
    );
    assert_eq!(
        reasoning_capabilities("gemma-4-31b-it", Some(true)),
        ReasoningCapabilities::NotConfigurable
    );
    assert_eq!(
        reasoning_capabilities("gemini-2.0-flash", None),
        ReasoningCapabilities::NotConfigurable
    );
}

#[test]
fn thinking_policy_covers_supported_levels() {
    assert!(!thinking_policy("gemini-3.1-flash-lite").allows(ReasoningLevel::Off));
    assert!(thinking_policy("gemini-3.1-flash-lite").allows(ReasoningLevel::Medium));
    assert!(thinking_policy("gemini-2.5-flash").allows(ReasoningLevel::Off));
    assert!(thinking_policy("gemini-2.0-flash").allows(ReasoningLevel::Off));
    assert!(!thinking_policy("gemini-2.0-flash").allows(ReasoningLevel::Medium));
}
