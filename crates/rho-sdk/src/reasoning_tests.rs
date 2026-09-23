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

// Unsupported levels round up to the next supported level without disabling
// reasoning, and only fall back down when above the highest supported level.
#[test]
fn normalizes_to_the_nearest_supported_level() {
    use ReasoningLevel::{High, Low, Max, Medium, Minimal, Off, Xhigh};

    let sparse: &[ReasoningLevel] = &[Off, Low, High, Max];
    let no_max: &[ReasoningLevel] = &[Off, Low, High, Xhigh];
    let codex: &[ReasoningLevel] = &[Off, Low, Medium, High, Xhigh];
    for (case, level, supported, expected) in [
        ("minimal rounds up", Minimal, sparse, Low),
        ("medium rounds up", Medium, sparse, High),
        ("xhigh rounds up", Xhigh, sparse, Max),
        ("off stays off", Off, &[Low][..], Off),
        ("above highest rounds down", Max, no_max, Xhigh),
        ("codex without minimal", Minimal, codex, Low),
    ] {
        assert_eq!(level.normalize(Some(supported)), expected, "{case}");
    }
}
