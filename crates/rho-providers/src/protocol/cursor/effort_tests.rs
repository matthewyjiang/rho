use pretty_assertions::assert_eq;

use crate::reasoning::ReasoningLevel;

use super::{split_effort, strip_effort_display_suffix};

// Covers: xhigh must not collapse to high, and product names keep inner tokens
// Owner: cursor protocol
#[test]
fn effort_suffixes_split_on_the_trailing_token_only() {
    let cases = [
        ("grok-4.6", "grok-4.6", None),
        ("grok-4.6-low", "grok-4.6", Some(ReasoningLevel::Low)),
        ("grok-4.6-medium", "grok-4.6", Some(ReasoningLevel::Medium)),
        ("grok-4.6-high", "grok-4.6", Some(ReasoningLevel::High)),
        ("grok-4.6-xhigh", "grok-4.6", Some(ReasoningLevel::Xhigh)),
        (
            "claude-4.6-opus-high",
            "claude-4.6-opus",
            Some(ReasoningLevel::High),
        ),
        ("grok-code-fast-1", "grok-code-fast-1", None),
        ("auto", "auto", None),
    ];
    for (model, catalog, effort) in cases {
        assert_eq!(split_effort(model), (catalog, effort), "model={model}");
    }
}

// Covers: Extra High must strip before High so xhigh display names stay readable
// Owner: cursor protocol
#[test]
fn effort_display_names_strip_longest_suffix_first() {
    assert_eq!(
        strip_effort_display_suffix("Grok 4.6 Extra High"),
        "Grok 4.6"
    );
    assert_eq!(strip_effort_display_suffix("Grok 4.6 High"), "Grok 4.6");
    assert_eq!(strip_effort_display_suffix("Grok 4.6"), "Grok 4.6");
}
