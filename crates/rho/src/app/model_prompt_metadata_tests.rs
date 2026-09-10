use super::*;
use pretty_assertions::assert_eq;

// Covers: accepted Unix filenames must not panic while saving provenance.
// Owner: host provenance serialization
// macOS rejects non-UTF-8 path components at the filesystem boundary, so this
// constructs the path without writing it. encode() only needs display text.
#[test]
fn non_utf8_model_prompt_path_serializes_as_display_text() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};
    let path = PathBuf::from(OsString::from_vec(b"\xff.md".to_vec()));
    let prompt = crate::prompt::model_prompts::ModelPrompt {
        path: path.clone(),
        mode: crate::prompt::model_prompts::ModelPromptMode::Append,
        body: "overlay".into(),
        sha256: "abc".into(),
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&encode(Some(&prompt))).unwrap(),
        serde_json::json!({"path": path.display().to_string(), "mode": "append", "sha256": "abc"}),
    );
}
