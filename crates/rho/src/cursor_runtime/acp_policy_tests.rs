use std::path::PathBuf;

use pretty_assertions::assert_eq;

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
