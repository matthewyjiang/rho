use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use pretty_assertions::assert_eq;

use super::*;

fn permission_request(kind: ToolKind) -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        "session",
        ToolCallUpdate::new("call", ToolCallUpdateFields::new().kind(kind)),
        vec![PermissionOption::new(
            "allow",
            "Allow",
            PermissionOptionKind::AllowOnce,
        )],
    )
}

// Covers: Plan pins agy's `plan` mode and rejects every permission request;
// Bypass pins `default` and approves a declared command; both spawn agy on
// the run's own agent; a pinned model becomes the session option; runs never
// authenticate; nothing is approved or spawned before `prepare`.
// Owner: Antigravity policy wiring; the driver applying these is covered in
// `acp_runtime` session tests, the fence table in `fence` tests.
#[test]
fn prepared_run_pins_agent_mode_permissions_and_model() {
    let run_dir = tempfile::tempdir().unwrap();
    let execute = permission_request(ToolKind::Execute);
    for (mode, expected_mode, expected_decision) in [
        (PermissionMode::Plan, "plan", PermissionDecision::Reject),
        (
            PermissionMode::Bypass,
            "default",
            PermissionDecision::AllowOnce,
        ),
    ] {
        let mut policy = AntigravityAcpPolicy::new(
            Some("gemini-3.8-flash".into()),
            vec![AntigravityTool::ReadFile, AntigravityTool::RunCommand],
            mode,
            PathBuf::from("/work"),
        );
        assert_eq!(
            policy.decide_permission(&execute),
            PermissionDecision::Reject,
            "{mode}: before prepare"
        );
        assert!(policy.spawn_plan(None).is_err(), "{mode}: before prepare");
        assert_eq!(policy.prepare(run_dir.path()), Ok(vec![]));
        let agent = policy.prepared.as_ref().unwrap().agent.clone();
        assert_eq!(agent.root, run_dir.path().join("agy"));
        let argv = policy.spawn_plan(None).unwrap().argv;
        assert_eq!(
            (
                argv,
                policy.session_mode(),
                policy.session_config(),
                policy.auth_method(&[]),
                policy.decide_permission(&execute),
            ),
            (
                vec![
                    "acp".into(),
                    "--cli_dir_name".into(),
                    "rho".into(),
                    "--add-dir".into(),
                    agent.root.into_os_string(),
                    "--agent".into(),
                    agent.name.into(),
                ],
                Some(SessionModeId::new(expected_mode)),
                vec![SessionConfigChoice {
                    id: "model".into(),
                    value: "gemini-3.8-flash".into(),
                }],
                None,
                expected_decision,
            ),
            "{mode}"
        );
    }
}

// Covers: `prepare` gates the run through the fence, so a mode Rho cannot
// answer for fails before spawn (the fence table owns the other cases).
// Owner: Antigravity preflight; message wording is not locked.
#[test]
fn prepare_refuses_modes_that_need_a_human() {
    let run_dir = tempfile::tempdir().unwrap();
    let prepared = AntigravityAcpPolicy::new(
        None,
        vec![AntigravityTool::ReadFile],
        PermissionMode::Auto,
        PathBuf::from("/work"),
    )
    .prepare(run_dir.path());
    assert!(prepared.is_err());
    assert!(
        !run_dir.path().join("agy").exists(),
        "no agent file is written"
    );
}
