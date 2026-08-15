use pretty_assertions::assert_eq;

use super::{activity_label, sanitize_title};

// Covers: title-model output is cleaned to a single short line.
// Owner: title sanitizer
#[test]
fn sanitize_title_strips_quotes_and_caps_length() {
    assert_eq!(
        sanitize_title("  \"Review the auth path\".  "),
        Some("Review the auth path".into())
    );
    assert_eq!(
        sanitize_title("\"Implement resume picker.\""),
        Some("Implement resume picker".into())
    );
    assert_eq!(sanitize_title("\n\n# Draft\n"), Some("Draft".into()));
    assert_eq!(sanitize_title("   "), None);

    let long = "word ".repeat(30);
    let sanitized = sanitize_title(&long).expect("long title");
    assert!(sanitized.ends_with('…'));
    assert!(sanitized.chars().count() <= 80);
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
