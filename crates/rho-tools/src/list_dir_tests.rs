use super::{list_directory, render_listing};
use pretty_assertions::assert_eq;
use serde_json::json;

// Covers: newline-bearing filenames remain one structured entry, not parsed text.
// Owner: OS/filesystem directory listing.
#[cfg(unix)]
#[tokio::test]
async fn structured_entries_preserve_newline_names() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a\nb"), "").unwrap();
    let rendered = render_listing(
        list_directory(dir.path()).await.unwrap(),
        crate::DEFAULT_MAX_OUTPUT_BYTES,
    );
    assert_eq!(
        serde_json::to_value(rendered.data().unwrap()).unwrap(),
        json!({"entries": [{"name": "a\nb", "kind": "file"}], "truncated": false})
    );
}
