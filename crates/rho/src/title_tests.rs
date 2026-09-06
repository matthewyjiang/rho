use pretty_assertions::assert_eq;

use super::{activity_label, sanitize_title};

// Covers: title-model prose is rejected rather than displayed as a truncated title.
// Owner: title sanitizer
#[test]
fn sanitize_title_cleans_labels_and_rejects_prose() {
    let at_character_limit = "a".repeat(80);
    let over_character_limit = "a".repeat(81);
    for (input, expected) in [
        ("  \"Review the auth path\".  ", Some("Review the auth path")),
        ("\"Implement resume picker.\"", Some("Implement resume picker")),
        ("\n\n# Draft\n", Some("Draft")),
        ("   ", None),
        ("Fix one two three four five six", Some("Fix one two three four five six")),
        ("Fix one two three four five six seven", None),
        ("Unable to inspect or modify the repository because no shell or file tools are available", None),
        ("Implement resume picker\nI will inspect the repository first", None),
        (at_character_limit.as_str(), Some(at_character_limit.as_str())),
        (over_character_limit.as_str(), None),
    ] {
        assert_eq!(sanitize_title(input).as_deref(), expected, "input: {input:?}");
    }
}

// Covers: rail and picker share one activity mapping.
// Owner: title display
#[test]
fn activity_label_maps_tool_and_assistant_text() {
    assert_eq!(activity_label(Some("assistant text")), "responding");
    assert_eq!(activity_label(Some("tool: read")), "read");
    assert_eq!(activity_label(Some("starting")), "starting");
    assert_eq!(activity_label(None), "working");
}
