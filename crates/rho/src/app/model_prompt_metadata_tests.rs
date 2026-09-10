use super::*;
use pretty_assertions::assert_eq;

// Covers: accepted Unix filenames must not panic while saving provenance.
// Owner: host provenance serialization
#[test]
fn non_utf8_model_prompt_path_serializes_as_display_text() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".rho/model-prompts");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(OsString::from_vec(b"\xff.md".to_vec()));
    std::fs::write(&path, "---\nprovider: test\nmodel: selected\n---\noverlay").unwrap();
    let model = crate::model_identity::PromptModel::from_sdk_identity(
        &rho_sdk::model::ModelIdentity::new("test", "test", "selected"),
    );
    let prompt = crate::prompt::model_prompts::load(Some(home.path()), &model)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&encode(Some(&prompt))).unwrap(),
        serde_json::json!({"path": path.display().to_string(), "mode": "append", "sha256": prompt.sha256}),
    );
}
