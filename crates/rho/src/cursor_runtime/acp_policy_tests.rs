use std::ffi::OsString;
use std::path::PathBuf;

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use crate::acp_runtime::AcpAgentPolicy;
use crate::agent::CursorTool;
use crate::permission::PermissionMode;

use super::*;

fn policy(mode: PermissionMode, tools: &[CursorTool], user_dir: PathBuf) -> CursorAcpPolicy {
    CursorAcpPolicy::new(
        Some("composer-2.5".into()),
        mode,
        tools.to_vec(),
        PathBuf::from("/workspace"),
        CursorConfigPaths {
            cursor_config_dir: Some(user_dir),
            xdg_config_home: None,
            home: PathBuf::from("/home/user"),
        },
    )
}

// Covers: the child's CURSOR_CONFIG_DIR must be the managed dir prepare wrote,
// with Cursor's approval forced to allowlist and the fence's deny list; a
// spawn pointing anywhere else silently runs with the user's unrestricted config.
// Owner: cursor ACP policy wiring
#[test]
fn prepare_writes_the_config_the_spawn_env_points_at() {
    let user = tempfile::tempdir().unwrap();
    std::fs::write(
        user.path().join("cli-config.json"),
        json!({"approvalMode": "unrestricted", "model": {"modelId": "composer-2.5"}}).to_string(),
    )
    .unwrap();
    let run = tempfile::tempdir().unwrap();
    let tools = [CursorTool::Read, CursorTool::Grep];
    let mut policy = policy(PermissionMode::Bypass, &tools, user.path().to_path_buf());

    policy.prepare(run.path()).unwrap();
    let plan = policy.spawn_plan(/*frozen*/ None).unwrap();

    let config_dir = run.path().join("cursor-config");
    assert_eq!(
        plan.argv,
        ["--trust", "--model", "composer-2.5", "acp"].map(OsString::from)
    );
    assert_eq!(
        plan.env,
        vec![(
            OsString::from("CURSOR_CONFIG_DIR"),
            config_dir.clone().into_os_string()
        )]
    );
    let written: Value =
        serde_json::from_slice(&std::fs::read(config_dir.join("cli-config.json")).unwrap())
            .unwrap();
    let expected = acp_config::derive_config(
        Some(json!({"approvalMode": "unrestricted", "model": {"modelId": "composer-2.5"}})),
        &acp_config::fence(PermissionMode::Bypass, &tools),
    )
    .unwrap();
    assert_eq!(written, expected);
    assert_eq!(written["approvalMode"], "allowlist");
}

// Covers: plan runs must switch Cursor into plan mode (argv `--mode` is ignored
// under acp); unsupported modes and unprepared policies fail closed.
// Owner: cursor ACP policy wiring
#[test]
fn modes_and_unprepared_policy_fail_closed() {
    let user = tempfile::tempdir().unwrap();
    let tools = [CursorTool::Read, CursorTool::Shell];

    let plan = policy(PermissionMode::Plan, &tools, user.path().to_path_buf());
    assert_eq!(plan.session_mode(), Some(SessionModeId::new("plan")));
    assert_eq!(
        policy(PermissionMode::Bypass, &tools, user.path().to_path_buf()).session_mode(),
        None
    );

    let unprepared = policy(PermissionMode::Bypass, &tools, user.path().to_path_buf());
    assert!(unprepared.spawn_plan(None).is_err());

    let run = tempfile::tempdir().unwrap();
    let mut auto = policy(PermissionMode::Auto, &tools, user.path().to_path_buf());
    assert_eq!(
        auto.prepare(run.path()),
        Err(spawn::CursorSpawnError::ApprovalUnsupported(PermissionMode::Auto).to_string())
    );
    assert!(!run.path().join("cursor-config").exists());
}
