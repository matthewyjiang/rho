use pretty_assertions::assert_eq;
use rho_providers::{
    model::{ReasoningCapabilities, ReasoningLevelSet},
    reasoning::ReasoningLevel,
};

use super::offered_reasoning_levels;

// Covers: the reasoning picker is skipped unless the model advertises a real
// choice, so it never guesses levels the model may reject.
// Owner: advisor reasoning picker policy
#[test]
fn reasoning_picker_needs_more_than_one_advertised_level() {
    let off_max = vec![ReasoningLevel::Off, ReasoningLevel::Max];
    let cases = [
        (
            "several advertised levels",
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(off_max.clone())),
            Some(off_max),
        ),
        (
            "single advertised level",
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![ReasoningLevel::High])),
            None,
        ),
        (
            "not configurable",
            ReasoningCapabilities::NotConfigurable,
            None,
        ),
        ("unknown", ReasoningCapabilities::Unknown, None),
    ];
    for (name, capabilities, expected) in cases {
        assert_eq!(
            offered_reasoning_levels(&capabilities).map(<[ReasoningLevel]>::to_vec),
            expected,
            "{name}"
        );
    }
}
