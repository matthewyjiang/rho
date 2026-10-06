use pretty_assertions::assert_eq;

use crate::agent::CursorTool;
use crate::permission::PermissionMode;

use super::*;

fn request(model: Option<&str>) -> CursorSpawnRequest {
    CursorSpawnRequest {
        model: model.map(str::to_string),
        cwd: PathBuf::from("/tmp/project"),
    }
}

// Covers: ACP argv carries identity only, with `acp` last; permission flags
// are ignored under `acp` (spike) and must not imply a fence.
// Owner: cursor spawn argv
#[test]
fn builds_identity_only_acp_args() {
    for (model, expected) in [
        (
            Some("composer-2.5"),
            vec!["--trust", "--model", "composer-2.5", "acp"],
        ),
        (None, vec!["--trust", "acp"]),
    ] {
        let plan = build_spawn_plan(&request(model));
        assert_eq!(plan.args, expected);
        assert_eq!(plan.cwd, PathBuf::from("/tmp/project"));
    }
}

// Covers: Plan keeps only read-only tools, Bypass keeps every declared tool,
// and Auto/Allow edits/Supervised refuse before spawn.
// Owner: cursor permission gate
#[test]
fn rho_permission_modes_gate_and_narrow_tools() {
    let mixed = [CursorTool::Read, CursorTool::Edit, CursorTool::Shell];
    let writes = [CursorTool::Edit, CursorTool::Shell];
    let reads = [CursorTool::Read, CursorTool::Grep];

    for (mode, tools, expected) in [
        (
            PermissionMode::Plan,
            mixed.as_slice(),
            Ok(vec![CursorTool::Read]),
        ),
        (PermissionMode::Bypass, mixed.as_slice(), Ok(mixed.to_vec())),
        (PermissionMode::Plan, reads.as_slice(), Ok(reads.to_vec())),
        (
            PermissionMode::Plan,
            writes.as_slice(),
            Err(CursorSpawnError::NoToolsAllowed),
        ),
        (
            PermissionMode::Bypass,
            &[][..],
            Err(CursorSpawnError::NoToolsAllowed),
        ),
        (
            PermissionMode::Auto,
            mixed.as_slice(),
            Err(CursorSpawnError::ApprovalUnsupported(PermissionMode::Auto)),
        ),
        (
            PermissionMode::AllowEdits,
            mixed.as_slice(),
            Err(CursorSpawnError::ApprovalUnsupported(
                PermissionMode::AllowEdits,
            )),
        ),
        (
            PermissionMode::Supervised,
            mixed.as_slice(),
            Err(CursorSpawnError::ApprovalUnsupported(
                PermissionMode::Supervised,
            )),
        ),
    ] {
        let got = map_permission_mode(mode, tools).map(|allowed| allowed.tools().to_vec());
        assert_eq!(got, expected);
    }
}

// Covers: frozen workflow argv (including pre-ACP `-p` plans) may keep only
// --model; `acp` stays the last argument.
// Owner: cursor spawn frozen identity
#[test]
fn frozen_identity_keeps_model_and_acp_last() {
    let frozen: Vec<String> = [
        "-p",
        "--model",
        "frozen-model",
        "--mode",
        "plan",
        "--allowed-tools",
        "shell_tool_call",
        "--force",
    ]
    .map(String::from)
    .to_vec();
    for (model, expected) in [
        (
            Some("composer-2.5"),
            vec!["--trust", "--model", "frozen-model", "acp"],
        ),
        (None, vec!["--trust", "--model", "frozen-model", "acp"]),
    ] {
        let plan = apply_frozen_identity_args(build_spawn_plan(&request(model)), &frozen);
        assert_eq!(plan.args, expected);
    }
}
