use super::*;

fn stored_session(agent_id: Option<&str>) -> (tempfile::TempDir, tempfile::TempDir, Session) {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = match agent_id {
        Some(agent_id) => {
            Session::create_in_root_with_agent(root.path(), cwd.path(), agent_id, "fingerprint-a")
                .unwrap()
        }
        None => Session::create_in_root(root.path(), cwd.path()).unwrap(),
    };
    (root, cwd, session)
}

#[test]
fn resume_accepts_unchanged_agent_definition() {
    let (_root, _cwd, session) = stored_session(Some("reviewer"));
    validate_resume_agent(&session, "reviewer", "fingerprint-a").unwrap();
}

#[test]
fn resume_reports_changed_agent_definition() {
    let (_root, _cwd, session) = stored_session(Some("reviewer"));
    let error = validate_resume_agent(&session, "reviewer", "fingerprint-b").unwrap_err();
    assert!(error.to_string().contains("definition changed"));
}

#[test]
fn resume_reports_missing_agent_identity() {
    let (_root, _cwd, session) = stored_session(None);
    let error = validate_resume_agent(&session, "default", "fingerprint-a").unwrap_err();
    assert!(error
        .to_string()
        .contains("no stored agent definition identity"));
}
