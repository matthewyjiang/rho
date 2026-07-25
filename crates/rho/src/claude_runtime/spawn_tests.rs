use pretty_assertions::assert_eq;

use crate::agent::{
    AgentDefinition, AgentId, AgentRuntime, AgentTools, ModelPolicy, ModelSelection, PromptPolicy,
};
use rho_providers::reasoning::ReasoningLevel;

use super::*;

fn definition(
    tools: Vec<&str>,
    inherit: bool,
    model: Option<&str>,
    prompt: PromptPolicy,
) -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new("claude-planner").unwrap(),
        description: "plan".into(),
        prompt,
        model: match model {
            Some(model) => ModelPolicy::Select(ModelSelection {
                provider: None,
                model: model.into(),
            }),
            None => ModelPolicy::Inherit,
        },
        runtime: AgentRuntime::ClaudeCli,
        tools: AgentTools::Claude(tools.into_iter().map(str::to_string).collect()),
        reasoning: None,
        inherit_claude_config: inherit,
    }
}

fn request(
    tools: Vec<&str>,
    inherit: bool,
    model: Option<&str>,
    permission_mode: PermissionMode,
    max_turns: u64,
    prompt: PromptPolicy,
) -> ClaudeSpawnRequest {
    request_with_effort(
        tools,
        inherit,
        model,
        permission_mode,
        max_turns,
        prompt,
        None,
    )
}

fn request_with_effort(
    tools: Vec<&str>,
    inherit: bool,
    model: Option<&str>,
    permission_mode: PermissionMode,
    max_turns: u64,
    prompt: PromptPolicy,
    effort: Option<&'static str>,
) -> ClaudeSpawnRequest {
    ClaudeSpawnRequest {
        definition: definition(tools.clone(), inherit, model, prompt),
        model: model.map(str::to_string),
        tools: tools.into_iter().map(str::to_string).collect(),
        inherit_claude_config: inherit,
        permission_mode,
        cwd: PathBuf::from("/tmp/project"),
        max_turns,
        effort,
    }
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
        PermissionMode::Auto,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();

    assert_eq!(plan.permission_mode, "dontAsk");
    assert_eq!(plan.setting_sources, "project");
    assert_eq!(plan.model.as_deref(), Some("opus"));
    assert_eq!(
        plan.system_prompt,
        SystemPromptPlan::Replace("Plan carefully.".into())
    );
    assert_eq!(
        plan.tool_base_names.as_slice(),
        ["Read", "Edit", "Bash"].as_slice()
    );
    assert_eq!(
        plan.allowed_tool_entries.as_slice(),
        ["Read", "Edit", "Bash(git *)"].as_slice()
    );
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--permission-mode", "dontAsk"]));
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--disallowedTools", "Task"]));
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
        .any(|pair| pair == ["--max-turns", "8"]));
    assert!(plan.args.windows(2).any(|pair| pair == ["--model", "opus"]));
    assert!(plan.effort.is_none());
    assert!(!plan.args.iter().any(|arg| arg == "--effort"));
    // Prompt text stays out of the base argv; file flag is attached on finalize.
    assert!(!plan.args.iter().any(|arg| arg.contains("Plan carefully.")));
    assert!(!plan.args.iter().any(|arg| arg == "--system-prompt"));
    assert!(!plan.args.iter().any(|arg| arg == "--system-prompt-file"));
    assert!(plan.args.contains(&"--include-partial-messages".into()));
    assert!(plan.args.contains(&"--verbose".into()));
    assert!(!plan.args.iter().any(|arg| arg.contains("bypass")));

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
        PermissionMode::Auto,
        4,
        PromptPolicy::Extend("Extra instructions.".into()),
    ))
    .unwrap();
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
        PermissionMode::Auto,
        4,
        PromptPolicy::Replace("Only this.".into()),
    ))
    .unwrap();
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
        PermissionMode::Auto,
        4,
        PromptPolicy::Extend(String::new()),
    ))
    .unwrap();
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
        PermissionMode::Auto,
        4,
        PromptPolicy::Replace(body.into()),
    ))
    .unwrap();
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
fn non_utf8_system_prompt_path_is_rejected_before_write() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let plan = build_spawn_plan(&request(
        vec!["Read"],
        false,
        None,
        PermissionMode::Auto,
        4,
        PromptPolicy::Replace("secret prompt bytes".into()),
    ))
    .unwrap();

    // Construct a non-UTF-8 output path under a valid parent without creating it.
    // macOS rejects non-UTF-8 path components at the filesystem boundary
    // (`Illegal byte sequence`), so the test must not depend on mkdir.
    let dir = tempfile::tempdir().unwrap();
    let output = dir
        .path()
        .join(OsStr::from_bytes(b"run-\xff-dir"))
        .join("result.json");

    let err = finalize_spawn_args(&plan, &output).expect_err("non-utf8 path must fail");
    assert!(
        matches!(err, ClaudeSpawnMaterializeError::NonUtf8Path { .. }),
        "unexpected error: {err}"
    );
    assert!(
        !system_prompt_path(&output).exists(),
        "must not write private prompt when path cannot be represented exactly"
    );
}

#[test]
fn inherit_config_widens_setting_sources() {
    let plan = build_spawn_plan(&request(
        vec!["Read"],
        true,
        None,
        PermissionMode::Plan,
        32,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();
    assert_eq!(plan.permission_mode, "plan");
    assert_eq!(plan.setting_sources, "user,project,local");
    assert!(plan.model.is_none());
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
        PermissionMode::Auto,
        16,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--model", "claude-opus-4-6"]));
}

#[test]
fn supervised_mode_is_refused() {
    let error = build_spawn_plan(&request(
        vec!["Read"],
        false,
        None,
        PermissionMode::Supervised,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap_err();
    assert_eq!(error, ClaudeSpawnError::SupervisedUnsupported);
}

#[test]
fn empty_tools_sets_tools_flag_to_empty_string() {
    let plan = build_spawn_plan(&request(
        Vec::new(),
        false,
        None,
        PermissionMode::Auto,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();
    assert!(plan.tool_base_names.is_empty());
    assert!(plan.allowed_tool_entries.is_empty());
    assert!(plan.args.windows(2).any(|pair| pair == ["--tools", ""]));
    assert!(!plan.args.iter().any(|arg| arg == "--allowedTools"));
}

#[test]
fn read_only_tools_go_to_tools_and_allowed_tools() {
    let plan = build_spawn_plan(&request(
        vec!["Read"],
        false,
        None,
        PermissionMode::Auto,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();
    assert_eq!(plan.tool_base_names.as_slice(), ["Read"].as_slice());
    assert_eq!(plan.allowed_tool_entries.as_slice(), ["Read"].as_slice());
    assert!(plan.args.windows(2).any(|pair| pair == ["--tools", "Read"]));
    let allowed = flag_values(&plan.args, "--allowedTools");
    assert_eq!(allowed, vec!["Read".to_string()]);
}

#[test]
fn edit_and_read_each_become_allowed_tools_argv_items() {
    let plan = build_spawn_plan(&request(
        vec!["Edit", "Read"],
        false,
        None,
        PermissionMode::Auto,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();
    assert_eq!(plan.tool_base_names.as_slice(), ["Edit", "Read"].as_slice());
    assert_eq!(
        plan.allowed_tool_entries.as_slice(),
        ["Edit", "Read"].as_slice()
    );
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--tools", "Edit,Read"]));
    let allowed = flag_values(&plan.args, "--allowedTools");
    assert_eq!(allowed, vec!["Edit".to_string(), "Read".to_string()]);
}

#[test]
fn bash_pattern_uses_tools_base_and_allowed_tools_pattern() {
    let plan = build_spawn_plan(&request(
        vec!["Bash(git *)"],
        false,
        None,
        PermissionMode::Auto,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();
    assert_eq!(plan.tool_base_names.as_slice(), ["Bash"].as_slice());
    assert_eq!(
        plan.allowed_tool_entries.as_slice(),
        ["Bash(git *)"].as_slice()
    );
    assert!(plan.args.windows(2).any(|pair| pair == ["--tools", "Bash"]));
    let allowed = flag_values(&plan.args, "--allowedTools");
    assert_eq!(allowed, vec!["Bash(git *)".to_string()]);
}

#[test]
fn mixed_bare_and_pattern_tools_all_allowed_separately() {
    let plan = build_spawn_plan(&request(
        vec!["Read", "Bash(git status *)", "Edit", "Bash(cargo *)"],
        false,
        None,
        PermissionMode::Auto,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();
    assert_eq!(
        plan.tool_base_names.as_slice(),
        ["Read", "Bash", "Edit"].as_slice()
    );
    let allowed = flag_values(&plan.args, "--allowedTools");
    assert_eq!(
        allowed,
        vec![
            "Read".to_string(),
            "Bash(git status *)".to_string(),
            "Edit".to_string(),
            "Bash(cargo *)".to_string(),
        ]
    );
}

#[test]
fn task_is_never_made_available_even_if_listed() {
    let plan = build_spawn_plan(&request(
        vec!["Read", "Task", "Task(sub)"],
        false,
        None,
        PermissionMode::Auto,
        8,
        PromptPolicy::Replace("Plan carefully.".into()),
    ))
    .unwrap();
    assert_eq!(plan.tool_base_names.as_slice(), ["Read"].as_slice());
    assert_eq!(plan.allowed_tool_entries.as_slice(), ["Read"].as_slice());
    assert!(plan
        .args
        .windows(2)
        .any(|pair| pair == ["--disallowedTools", "Task"]));
    let allowed = flag_values(&plan.args, "--allowedTools");
    assert_eq!(allowed, vec!["Read".to_string()]);
    assert!(!allowed.iter().any(|entry| entry.contains("Task")));
}

#[test]
fn non_default_max_turns_is_emitted_exactly() {
    for turns in [1_u64, 7, 64, 10_000] {
        let plan = build_spawn_plan(&request(
            vec!["Read"],
            false,
            None,
            PermissionMode::Auto,
            turns,
            PromptPolicy::Replace("Plan carefully.".into()),
        ))
        .unwrap();
        assert!(
            plan.args
                .windows(2)
                .any(|pair| { pair[0] == "--max-turns" && pair[1] == turns.to_string() }),
            "missing --max-turns {turns}"
        );
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
        let plan = build_spawn_plan(&request_with_effort(
            vec!["Read"],
            false,
            None,
            PermissionMode::Auto,
            8,
            PromptPolicy::Replace("Plan carefully.".into()),
            Some(expected),
        ))
        .unwrap();
        assert_eq!(plan.effort, Some(expected));
        assert!(
            plan.args
                .windows(2)
                .any(|pair| pair == ["--effort", expected]),
            "missing --effort {expected}"
        );
    }
    assert_eq!(claude_effort_flag(ReasoningLevel::Off), None);
    assert_eq!(claude_effort_flag(ReasoningLevel::Minimal), None);
}

#[test]
fn detects_max_turns_rejection() {
    assert!(looks_like_max_turns_unsupported(
        "error: unknown option '--max-turns'"
    ));
    assert!(!looks_like_max_turns_unsupported("ran out of turns"));
}
