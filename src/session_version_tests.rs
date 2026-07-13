use super::*;

#[test]
fn rejects_sessions_from_a_newer_format_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("1_future-version.jsonl");
    let entry = SessionEntry::Session {
        version: SESSION_VERSION + 1,
        id: "future-version".into(),
        timestamp: "1".into(),
        cwd: directory.path().to_path_buf(),
    };
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&entry).unwrap()),
    )
    .unwrap();

    let error = summarize_session_file(&path, directory.path()).unwrap_err();

    assert!(error.to_string().contains("unsupported session version"));
}
