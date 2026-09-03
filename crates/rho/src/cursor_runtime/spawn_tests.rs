use pretty_assertions::assert_eq;

use crate::agent::{CursorTool, PromptPolicy};
use crate::permission::PermissionMode;

use super::*;

fn request(
    tools: Vec<CursorTool>,
    model: Option<&str>,
    permission_mode: CursorPermissionMode,
) -> CursorSpawnRequest {
    CursorSpawnRequest {
        system_prompt: PromptPolicy::Extend(String::new()),
        model: model.map(str::to_string),
        tools,
        permission_mode,
        cwd: PathBuf::from("/tmp/project"),
    }
}

fn forbidden_flags_absent(args: &[String]) {
    for flag in ["--force", "--exclude-tools", "--show-thinking", "--resume"] {
        assert!(
            !args.iter().any(|arg| arg == flag),
            "{flag} must never appear: {args:?}"
        );
    }
}

// Covers: Plan and Full argv are the verified cursor-agent -p contract and
// never pass force / exclude / thinking / resume.
// Owner: cursor spawn argv
#[test]
fn builds_explicit_safe_spawn_args() {
    let plan = build_spawn_plan(&request(
        vec![CursorTool::Read],
        Some("composer-2.5"),
        CursorPermissionMode::Plan,
    ));
    assert_eq!(
        plan.args,
        [
            "-p",
            "--output-format",
            "stream-json",
            "--stream-partial-output",
            "--trust",
            "--model",
            "composer-2.5",
            "--mode",
            "plan",
            "--allowed-tools",
            "read_tool_call",
        ]
    );
    forbidden_flags_absent(&plan.args);
    assert_eq!(plan.cwd, PathBuf::from("/tmp/project"));

    let full = build_spawn_plan(&request(
        vec![CursorTool::Read, CursorTool::Edit, CursorTool::Shell],
        Some("composer-2.5"),
        CursorPermissionMode::Full,
    ));
    assert_eq!(
        full.args,
        [
            "-p",
            "--output-format",
            "stream-json",
            "--stream-partial-output",
            "--trust",
            "--model",
            "composer-2.5",
            "--allowed-tools",
            "read_tool_call,edit_tool_call,shell_tool_call",
        ]
    );
    forbidden_flags_absent(&full.args);

    let omitted_model = build_spawn_plan(&request(
        vec![CursorTool::Read],
        None,
        CursorPermissionMode::Full,
    ));
    assert!(!omitted_model.args.iter().any(|arg| arg == "--model"));
    assert!(!omitted_model.args.iter().any(|arg| arg == "--mode"));

    let finalized = finalize_spawn_args(&plan, "11111111-1111-4111-8111-111111111111");
    assert_eq!(
        finalized[plan.args.len()..],
        [
            OsString::from("--new-session-id"),
            OsString::from("11111111-1111-4111-8111-111111111111"),
        ]
    );
}

// Covers: Rho Plan/Bypass map onto Cursor plan/full with Plan dropping
// write/process tools, and Auto/Allow edits/Supervised refuse before spawn.
// Owner: cursor spawn permission mapping
#[test]
fn rho_permission_modes_map_to_cursor_modes() {
    let mixed = [CursorTool::Read, CursorTool::Edit, CursorTool::Shell];
    let writes = [CursorTool::Edit, CursorTool::Shell];
    let reads = [CursorTool::Read, CursorTool::Grep];

    for (mode, tools, expected) in [
        (
            PermissionMode::Plan,
            mixed.as_slice(),
            Ok((CursorPermissionMode::Plan, vec![CursorTool::Read])),
        ),
        (
            PermissionMode::Bypass,
            mixed.as_slice(),
            Ok((CursorPermissionMode::Full, mixed.to_vec())),
        ),
        (
            PermissionMode::Plan,
            reads.as_slice(),
            Ok((CursorPermissionMode::Plan, reads.to_vec())),
        ),
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
        assert_eq!(map_permission_mode(mode, tools), expected);
    }
}

// Covers: frozen workflow argv may keep --model and must not keep --mode,
// --allowed-tools, or --force.
// Owner: cursor spawn frozen identity
#[test]
fn frozen_identity_keeps_model_and_never_permission_flags() {
    let generated = build_spawn_plan(&request(
        vec![CursorTool::Read, CursorTool::Edit],
        Some("composer-2.5"),
        CursorPermissionMode::Full,
    ))
    .args;
    let frozen = vec![
        "-p".into(),
        "--model".into(),
        "frozen-model".into(),
        "--mode".into(),
        "plan".into(),
        "--allowed-tools".into(),
        "shell_tool_call".into(),
        "--force".into(),
        "--exclude-tools".into(),
        "edit_tool_call".into(),
    ];
    let args = apply_frozen_identity_args(generated, &frozen);
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--model", "frozen-model"]),
        "frozen model must overlay generated: {args:?}"
    );
    assert!(
        !args.windows(2).any(|pair| pair == ["--mode", "plan"]),
        "frozen --mode must not survive: {args:?}"
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--allowed-tools", "read_tool_call,edit_tool_call"]),
        "generated allow list must remain: {args:?}"
    );
    forbidden_flags_absent(&args);
}

// Covers: nonempty Extend prepends onto stdin; empty Extend is the user
// prompt alone; Replace is an internal error.
// Owner: cursor spawn prompt composition
#[test]
fn compose_prompt_extends_or_passes_through() {
    assert_eq!(
        compose_prompt(&PromptPolicy::Extend("Be terse.".into()), "hello").unwrap(),
        "Be terse.\n\n---\n\nhello"
    );
    assert_eq!(
        compose_prompt(&PromptPolicy::Extend(String::new()), "hello").unwrap(),
        "hello"
    );
    assert_eq!(
        compose_prompt(&PromptPolicy::Replace("nope".into()), "hello"),
        Err(CursorPromptError::ReplaceUnsupported)
    );
}
