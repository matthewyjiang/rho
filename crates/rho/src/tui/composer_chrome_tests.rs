use pretty_assertions::assert_eq;

use super::wrap_footer_parts;

// Covers: footer binds must wrap as whole segments instead of clipping mid-hint.
// Owner: tui footer layout
#[test]
fn wrap_footer_parts_keeps_segments_whole() {
    let parts = [
        "select model",
        "Type to search",
        "Enter select",
        "Ctrl+P pin/unpin",
        "Ctrl+O all/pinned",
        "Esc cancel",
    ];

    assert_eq!(
        wrap_footer_parts(parts, 49),
        vec![
            "select model · Type to search · Enter select".to_string(),
            "Ctrl+P pin/unpin · Ctrl+O all/pinned · Esc cancel".to_string(),
        ]
    );
    assert_eq!(
        wrap_footer_parts(parts, 20),
        vec![
            "select model".to_string(),
            "Type to search".to_string(),
            "Enter select".to_string(),
            "Ctrl+P pin/unpin".to_string(),
            "Ctrl+O all/pinned".to_string(),
            "Esc cancel".to_string(),
        ]
    );
    assert_eq!(
        wrap_footer_parts(["", "Enter select", "", "Esc cancel"], 80),
        vec!["Enter select · Esc cancel".to_string()]
    );
    assert_eq!(
        wrap_footer_parts(["Ctrl+O all/pinned"], 8),
        vec!["Ctrl+O all/pinned".to_string()]
    );
}
