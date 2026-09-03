use pretty_assertions::assert_eq;

use crate::agent::PromptPolicy;
use rho_providers::reasoning::ReasoningLevel;

use super::*;

fn request(
    tools: Vec<&str>,
    inherit: bool,
    model: Option<&str>,
    permission_mode: ClaudePermissionMode,
    max_turns: u64,
    prompt: PromptPolicy,
) -> ClaudeSpawnRequest {
    request_with_reasoning(
        tools,
        inherit,
        model,
        permission_mode,
        max_turns,
        prompt,
        None,
    )
}

fn request_with_reasoning(
    tools: Vec<&str>,
    inherit: bool,
    model: Option<&str>,
    permission_mode: ClaudePermissionMode,
    max_turns: u64,
    prompt: PromptPolicy,
    reasoning: Option<ReasoningLevel>,
) -> ClaudeSpawnRequest {
    ClaudeSpawnRequest {
        system_prompt: prompt,
        model: model.map(str::to_string),
        tools: tools.into_iter().map(str::to_string).collect(),
        inherit_claude_config: inherit,
        permission_mode,
        cwd: PathBuf::from("/tmp/project"),
        max_turns,
        reasoning,
        session_persistence: SessionPersistence::Keep,
        input_format: ClaudeInputFormat::StreamJson,
    }
}

/// Sole value following `flag` in argv, or `None` when the flag is absent.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let values = flag_values(args, flag);
    assert!(values.len() <= 1, "{flag} carried {} values", values.len());
    values.into_iter().next()
}

fn flag_values(args: &[String], flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == flag {
            index += 1;
            while index < args.len() && !args[index].starts_with("--") && args[index] != "-p" {
                values.push(args[index].clone());
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    values
}

fn finalized(plan: &ClaudeSpawnPlan, output_file: &Path) -> Vec<std::ffi::OsString> {
    finalize_spawn_args(plan, output_file).expect("materialize system prompt")
}

fn os(s: &str) -> std::ffi::OsString {
    std::ffi::OsString::from(s)
}

#[test]
fn builds_explicit_safe_spawn_args() {
    let plan = build_spawn_plan(&request(
        vec!["Read", "Edit", "Bash(git *)"],
        false,
        Some("opus"),
        ClaudePermissionMode::BypassPermissions,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ));

    assert_eq!(
        plan.system_prompt,
        SystemPromptPlan::Replace("Plan carefully.".into())
    );
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--permission-mode", "bypassPermissions"]));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--disallowedTools", "Task,Agent"]));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--tools", "Read,Edit,Bash"]));
    // Bare names and patterns are separate argv elements after --allowedTools.
    let allowed_idx = plan
        .args
        .iter()
        .position(|arg| arg == "--allowedTools")
        .expect("allowedTools present");
    assert_eq!(
        &plan.args[allowed_idx + 1..allowed_idx + 4],
        ["Read", "Edit", "Bash(git *)"]
    );
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--setting-sources", "project"]));
    assert!(plan.args.iter().any(|arg| arg == "--strict-mcp-config"));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--input-format", "stream-json"]));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--max-turns", "8"]));
    assert!(plan.args.windows(2).any(|pair| pair == ["--model", "opus"]));
    assert!(!plan.args.iter().any(|arg| arg == "--effort"));
    // Prompt text stays out of the base argv; file flag is attached on finalize.
    assert!(!plan.args.iter().any(|arg| arg.contains("Plan carefully.")));
    assert!(!plan.args.iter().any(|arg| arg == "--system-prompt"));
    assert!(!plan.args.iter().any(|arg| arg == "--system-prompt-file"));
    assert!(plan.args.contains(&"--include-partial-messages".into()));
    assert!(plan.args.contains(&"--verbose".into()));
    // Bypass uses Claude bypassPermissions via --permission-mode only; do not
    // also pass the separate --dangerously-skip-permissions flag.
    assert!(!plan
        .args
        .iter()
        .any(|arg| arg.contains("dangerously-skip-permissions")));

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let args = finalized(&plan, &output);
    let prompt_path = system_prompt_path(&output);
    assert!(prompt_path.is_file());
    assert_eq!(
        std::fs::read_to_string(&prompt_path).unwrap(),
        "Plan carefully."
    );
    assert!(args.windows(2).any(|pair| {
        pair[0] == os("--system-prompt-file") && pair[1] == prompt_path.as_os_str()
    }));
    assert!(!args.iter().any(|arg| arg == "--system-prompt"));
    assert!(!args.iter().any(|arg| arg == "Plan carefully."));
}

#[test]
fn extend_prompt_uses_append_system_prompt_file() {
    let plan = build_spawn_plan(&request(
        vec!["Read"],
        false,
        None,
        ClaudePermissionMode::BypassPermissions,
        4,
        PromptPolicy::Extend("Extra instructions.".into()),
    ));
    assert_eq!(
        plan.system_prompt,
        SystemPromptPlan::Extend("Extra instructions.".into())
    );
    assert_eq!(
        plan.system_prompt.file_flag(),
        Some("--append-system-prompt-file")
    );

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let args = finalized(&plan, &output);
    let prompt_path = system_prompt_path(&output);
    assert_eq!(
        std::fs::read_to_string(&prompt_path).unwrap(),
        "Extra instructions."
    );
    assert!(args.windows(2).any(|pair| {
        pair[0] == os("--append-system-prompt-file") && pair[1] == prompt_path.as_os_str()
    }));
    assert!(!args.iter().any(|arg| arg == "--system-prompt-file"));
    assert!(!args.iter().any(|arg| arg == "--system-prompt"));
    assert!(!args.iter().any(|arg| arg == "--append-system-prompt"));
}

#[test]
fn replace_prompt_uses_system_prompt_file_exactly() {
    let plan = build_spawn_plan(&request(
        vec!["Read"],
        false,
        None,
        ClaudePermissionMode::BypassPermissions,
        4,
        PromptPolicy::Replace("Only this.".into()),
    ));
    assert_eq!(
        plan.system_prompt,
        SystemPromptPlan::Replace("Only this.".into())
    );

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let args = finalized(&plan, &output);
    assert!(args
        .windows(2)
        .any(|pair| pair[0] == os("--system-prompt-file")));
    assert!(!args
        .iter()
        .any(|arg| *arg == os("--append-system-prompt-file")));
    assert_eq!(
        std::fs::read_to_string(system_prompt_path(&output)).unwrap(),
        "Only this."
    );
}

#[test]
fn empty_extend_omits_system_prompt_entirely() {
    let plan = build_spawn_plan(&request(
        vec!["Read"],
        false,
        None,
        ClaudePermissionMode::BypassPermissions,
        4,
        PromptPolicy::Extend(String::new()),
    ));
    assert_eq!(plan.system_prompt, SystemPromptPlan::Omit);
    assert!(plan.system_prompt.file_flag().is_none());
    assert!(plan.system_prompt.text().is_none());

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let args = finalized(&plan, &output);
    let expected: Vec<std::ffi::OsString> =
        plan.args.iter().map(std::ffi::OsString::from).collect();
    assert_eq!(args, expected);
    assert!(!system_prompt_path(&output).exists());
    assert!(!args
        .iter()
        .any(|arg| arg.to_string_lossy().contains("system-prompt")));
    assert!(!args
        .iter()
        .any(|arg| arg.to_string_lossy().contains("You are a coding agent")));
}

#[test]
fn multiline_replace_prompt_preserves_bytes_in_file() {
    let body = "Line one.\nLine two.\r\n\tIndented \"quote\" & <meta>.\n";
    let plan = build_spawn_plan(&request(
        vec!["Read"],
        false,
        None,
        ClaudePermissionMode::BypassPermissions,
        4,
        PromptPolicy::Replace(body.into()),
    ));
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let args = finalized(&plan, &output);
    let prompt_path = system_prompt_path(&output);
    let written = std::fs::read(&prompt_path).unwrap();
    assert_eq!(written, body.as_bytes());
    // Path only on argv - never the multiline body.
    assert!(!args.iter().any(|arg| {
        let text = arg.to_string_lossy();
        text.contains('\n') || text.contains('\r')
    }));
    assert!(args
        .windows(2)
        .any(|pair| pair[0] == os("--system-prompt-file")));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&prompt_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "system prompt file should be owner-private");
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_system_prompt_path_uses_os_string_argv() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let plan = build_spawn_plan(&request(
        vec!["Read"],
        false,
        None,
        ClaudePermissionMode::BypassPermissions,
        4,
        PromptPolicy::Replace("secret prompt bytes".into()),
    ));

    // Construct a non-UTF-8 output path under a valid parent. macOS may reject
    // non-UTF-8 path components at the filesystem boundary (`Illegal byte
    // sequence`); when write fails the error stays a Write, not a UTF-8 gate.
    let dir = tempfile::tempdir().unwrap();
    let output = dir
        .path()
        .join(OsStr::from_bytes(b"run-\xff-dir"))
        .join("result.json");

    match finalize_spawn_args(&plan, &output) {
        Ok(args) => {
            let prompt_path = system_prompt_path(&output);
            assert!(
                prompt_path.exists(),
                "native path should write the private prompt file"
            );
            assert!(
                args.iter()
                    .any(|arg| arg.as_os_str() == prompt_path.as_os_str()),
                "argv must carry the native OsString path token"
            );
        }
        Err(ClaudeSpawnMaterializeError::Write { .. }) => {
            // Filesystem rejected the non-UTF-8 component; no UTF-8 argv gate.
        }
    }
}

#[test]
fn inherit_config_widens_setting_sources() {
    let plan = build_spawn_plan(&request(
        vec!["Read"],
        true,
        None,
        ClaudePermissionMode::Plan,
        32,
        PromptPolicy::Replace("Plan carefully.".into()),
    ));
    assert_eq!(
        flag_value(&plan.args, "--permission-mode").as_deref(),
        Some("plan")
    );
    assert_eq!(
        flag_value(&plan.args, "--setting-sources").as_deref(),
        Some("user,project,local")
    );
    assert!(!plan.args.iter().any(|arg| arg == "--model"));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--max-turns", "32"]));
}

#[test]
fn model_is_passed_byte_for_byte_without_alias_rewrite() {
    let plan = build_spawn_plan(&request(
        vec!["Read"],
        false,
        Some("claude-opus-4-6"),
        ClaudePermissionMode::BypassPermissions,
        16,
        PromptPolicy::Replace("Plan carefully.".into()),
    ));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--model", "claude-opus-4-6"]));
}

// Covers: Auto / Allow edits reach dontAsk only when every tool is proven
// free for that Rho class. Specifiers, inherited Claude settings, write or
// process tools, and unknown / plugin / MCP names fail closed.
// Owner: Claude spawn argv mapping
#[test]
fn rho_permission_modes_map_to_claude_cli_modes() {
    use crate::permission::PermissionMode;

    let bare = ["Read".to_string(), "Glob".to_string()];
    let network = ["WebSearch".to_string(), "WebFetch".to_string()];
    let narrowed = ["Read".to_string(), "Bash(git status:*)".to_string()];
    let bash = ["Bash".to_string()];
    let edit = ["Edit".to_string()];
    let write = ["Write".to_string()];
    let notebook = ["NotebookEdit".to_string()];
    let powershell = ["PowerShell".to_string()];
    let mcp = ["mcp__server__tool".to_string()];
    let list_mcp_resource = ["ListMcpResourcesTool".to_string()];
    let read_mcp_resource = ["ReadMcpResourceTool".to_string()];
    let unknown = ["FutureClaudeTool".to_string()];

    for (mode, tools, inherit, expected) in [
        (
            PermissionMode::Plan,
            bare.as_slice(),
            false,
            Ok(ClaudePermissionMode::Plan),
        ),
        (
            PermissionMode::Bypass,
            narrowed.as_slice(),
            true,
            Ok(ClaudePermissionMode::BypassPermissions),
        ),
        (
            PermissionMode::Auto,
            bare.as_slice(),
            false,
            Ok(ClaudePermissionMode::DontAsk),
        ),
        (
            PermissionMode::AllowEdits,
            bare.as_slice(),
            false,
            Ok(ClaudePermissionMode::DontAsk),
        ),
        (
            PermissionMode::Auto,
            bash.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::Auto,
            edit.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::Auto,
            write.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::AllowEdits,
            bash.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::AllowEdits,
            edit.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::AllowEdits,
            write.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::Auto,
            network.as_slice(),
            false,
            Ok(ClaudePermissionMode::DontAsk),
        ),
        (
            PermissionMode::AllowEdits,
            network.as_slice(),
            false,
            Ok(ClaudePermissionMode::DontAsk),
        ),
        (
            PermissionMode::Auto,
            notebook.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::AllowEdits,
            powershell.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::Auto,
            mcp.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::Auto,
            list_mcp_resource.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::AllowEdits,
            read_mcp_resource.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::AllowEdits,
            unknown.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::Auto,
            &["Read".to_string(), "mcp__server__tool".to_string()][..],
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::Plan,
            mcp.as_slice(),
            false,
            Ok(ClaudePermissionMode::Plan),
        ),
        (
            PermissionMode::Bypass,
            unknown.as_slice(),
            false,
            Ok(ClaudePermissionMode::BypassPermissions),
        ),
        (
            PermissionMode::Auto,
            &[][..],
            false,
            Ok(ClaudePermissionMode::DontAsk),
        ),
        (
            PermissionMode::Auto,
            narrowed.as_slice(),
            false,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::AllowEdits,
            bare.as_slice(),
            true,
            Err(ClaudeSpawnError::DontAskUnbound),
        ),
        (
            PermissionMode::Supervised,
            bare.as_slice(),
            false,
            Err(ClaudeSpawnError::SupervisedUnsupported),
        ),
    ] {
        assert_eq!(map_permission_mode(mode, tools, inherit), expected);
        if let Ok(mapped) = expected {
            assert_eq!(
                mapped.as_cli_flag(),
                match mapped {
                    ClaudePermissionMode::Plan => "plan",
                    ClaudePermissionMode::BypassPermissions => "bypassPermissions",
                    ClaudePermissionMode::DontAsk => "dontAsk",
                }
            );
        }
    }
}

// Covers: a bound Auto spawn uses dontAsk, empty setting sources, and does
// not expose a Bash base tool that Claude would read-only-approve.
// Owner: Claude spawn argv mapping
#[test]
fn auto_spawn_plan_uses_dont_ask_with_declared_tool_boundary() {
    let mode = map_permission_mode(
        crate::permission::PermissionMode::Auto,
        &["Read".into(), "Glob".into()],
        false,
    )
    .expect("bound Auto maps to Claude dontAsk");
    let plan = build_spawn_plan(&request(
        vec!["Read", "Glob"],
        false,
        None,
        mode,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--permission-mode", "dontAsk"]));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--setting-sources", ""]));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--tools", "Read,Glob"]));
    assert_eq!(
        flag_values(&plan.args, "--allowedTools"),
        vec!["Read".to_string(), "Glob".to_string()]
    );
}

#[test]
fn empty_tools_sets_tools_flag_to_empty_string() {
    let plan = build_spawn_plan(&request(
        Vec::new(),
        false,
        None,
        ClaudePermissionMode::BypassPermissions,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ));
    assert_eq!(flag_value(&plan.args, "--tools").as_deref(), Some(""));
    assert!(plan.args.windows(2).any(|pair| pair == ["--tools", ""]));
    assert!(!plan.args.iter().any(|arg| arg == "--allowedTools"));
}

#[test]
fn nested_agent_tools_are_never_made_available_even_if_listed() {
    for permission_mode in [
        ClaudePermissionMode::BypassPermissions,
        ClaudePermissionMode::Plan,
        ClaudePermissionMode::DontAsk,
    ] {
        let plan = build_spawn_plan(&request(
            vec!["Read", "Task", "Task(sub)", "Agent", "Agent(explore)"],
            false,
            None,
            permission_mode,
            8,
            PromptPolicy::Replace("Plan carefully.".into()),
        ));
        assert_eq!(
            flag_value(&plan.args, "--tools").as_deref(),
            Some("Read"),
            "{permission_mode:?}"
        );
        assert!(
            plan.args
                .windows(2)
                .any(|pair| pair == ["--disallowedTools", "Task,Agent"]),
            "{permission_mode:?}"
        );
        let allowed = flag_values(&plan.args, "--allowedTools");
        assert_eq!(allowed, vec!["Read".to_string()], "{permission_mode:?}");
    }
}

#[test]
fn reasoning_maps_to_claude_effort_flag() {
    for (level, expected) in [
        (ReasoningLevel::Low, "low"),
        (ReasoningLevel::Medium, "medium"),
        (ReasoningLevel::High, "high"),
        (ReasoningLevel::Xhigh, "xhigh"),
        (ReasoningLevel::Max, "max"),
    ] {
        assert_eq!(claude_effort_flag(level), Some(expected));
        let plan = build_spawn_plan(&request_with_reasoning(
            vec!["Read"],
            false,
            None,
            ClaudePermissionMode::BypassPermissions,
            8,
            PromptPolicy::Replace("Plan carefully.".into()),
            Some(level),
        ));
        assert!(
            plan.args
                .windows(2)
                .any(|pair| pair == ["--effort", expected]),
            "missing --effort {expected}"
        );
    }
    assert_eq!(claude_effort_flag(ReasoningLevel::Off), None);
    assert_eq!(claude_effort_flag(ReasoningLevel::Minimal), None);
    for omitted in [
        None,
        Some(ReasoningLevel::Off),
        Some(ReasoningLevel::Minimal),
    ] {
        let plan = build_spawn_plan(&request_with_reasoning(
            vec!["Read"],
            false,
            None,
            ClaudePermissionMode::BypassPermissions,
            8,
            PromptPolicy::Replace("Plan carefully.".into()),
            omitted,
        ));
        assert!(
            !plan.args.iter().any(|arg| arg == "--effort"),
            "{omitted:?} must omit --effort: {:?}",
            plan.args
        );
    }
}

#[test]
fn require_claude_reasoning_keeps_mapped_levels_and_rejects_unmapped() {
    assert_eq!(require_claude_reasoning(None).unwrap(), None);
    assert_eq!(
        require_claude_reasoning(Some(ReasoningLevel::High)).unwrap(),
        Some(ReasoningLevel::High)
    );
    let off = require_claude_reasoning(Some(ReasoningLevel::Off)).unwrap_err();
    assert!(
        off.to_string().contains("not a Claude Code effort level"),
        "{off:#}"
    );
    let minimal = require_claude_reasoning(Some(ReasoningLevel::Minimal)).unwrap_err();
    assert!(
        minimal
            .to_string()
            .contains("not a Claude Code effort level"),
        "{minimal:#}"
    );
}

#[test]
fn detects_max_turns_rejection() {
    assert!(looks_like_max_turns_unsupported(
        "error: unknown option '--max-turns'"
    ));
    assert!(!looks_like_max_turns_unsupported("ran out of turns"));
}

// Covers: a delegated agent run must stay resumable while Rho's own one-shot
// calls must not accumulate single-turn sessions in the user's Claude history.
// Owner: claude spawn argv
#[test]
fn session_persistence_decides_the_no_session_persistence_flag() {
    for (persistence, expected) in [
        (SessionPersistence::Keep, false),
        (SessionPersistence::Discard, true),
    ] {
        let mut spawn_request = request(
            vec!["Read"],
            false,
            None,
            ClaudePermissionMode::BypassPermissions,
            8,
            PromptPolicy::Replace("Plan carefully.".into()),
        );
        spawn_request.session_persistence = persistence;
        let plan = build_spawn_plan(&spawn_request);
        assert_eq!(
            plan.args
                .iter()
                .any(|arg| arg == "--no-session-persistence"),
            expected,
            "{persistence:?} produced {:?}",
            plan.args
        );
    }
}

// Covers: an inline prompt must reach argv under the flag that matches its
// policy, so a Replace prompt never silently becomes an append.
// Owner: claude spawn argv
#[test]
fn inline_prompt_args_carry_the_prompt_under_the_matching_flag() {
    for (prompt, expected) in [
        (
            PromptPolicy::Replace("Review this.".into()),
            Some(("--system-prompt", "Review this.")),
        ),
        (
            PromptPolicy::Extend("Also review this.".into()),
            Some(("--append-system-prompt", "Also review this.")),
        ),
        (PromptPolicy::Extend(String::new()), None),
    ] {
        let plan = build_spawn_plan(&request(
            vec![],
            false,
            None,
            ClaudePermissionMode::Plan,
            1,
            prompt.clone(),
        ));
        let args = inline_prompt_args(&plan);
        match expected {
            Some((flag, text)) => {
                assert_eq!(
                    args[args.len() - 2..],
                    [os(flag), os(text)],
                    "{prompt:?} produced {args:?}"
                );
            }
            None => assert!(
                !args.iter().any(
                    |arg| *arg == os("--system-prompt") || *arg == os("--append-system-prompt")
                ),
                "{prompt:?} produced {args:?}"
            ),
        }
    }
}
