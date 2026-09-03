//! Draft mutation tests for agent edit/save helpers.

use pretty_assertions::assert_eq;

use super::*;
use crate::agent::{
    parse_definition, AgentDefinition, AgentId, AgentRuntime, AgentRuntimeSpec, ClaudeAgentConfig,
    ClaudeToolPolicy, CursorAgentConfig, CursorTool, ModelPolicy, ModelSelection, PromptPolicy,
    ReasoningLevel, ToolCapability, ToolPolicy,
};

fn rho_draft() -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new("draft").unwrap(),
        description: "draft agent".into(),
        prompt: PromptPolicy::Extend("body".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            model: ModelPolicy::Inherit,
            reasoning: None,
        },
    }
}

fn claude_draft() -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new("claude-draft").unwrap(),
        description: "claude draft".into(),
        prompt: PromptPolicy::Extend("body".into()),
        runtime: AgentRuntimeSpec::ClaudeCli(ClaudeAgentConfig {
            tools: ClaudeToolPolicy::None,
            inherit_claude_config: false,
            model: None,
            reasoning: None,
        }),
    }
}

// Covers: switching rho -> claude-cli drops incompatible fields
// Owner: agent edit
#[test]
fn switching_to_claude_cli_resets_incompatible_fields() {
    let mut draft = AgentDefinition {
        id: AgentId::new("switch").unwrap(),
        description: "switch agent".into(),
        prompt: PromptPolicy::Extend("body".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(
                [ToolCapability::ReadFile, ToolCapability::Shell]
                    .into_iter()
                    .collect(),
            ),
            model: ModelPolicy::Select(ModelSelection {
                provider: Some("openai".into()),
                model: "gpt-5.5".into(),
                auth: None,
            }),
            reasoning: Some(ReasoningLevel::Off),
        },
    };

    assert!(draft.switch_runtime_kind("claude-cli"));
    match &draft.runtime {
        AgentRuntimeSpec::ClaudeCli(config) => {
            assert_eq!(config.model.as_deref(), Some("gpt-5.5"));
            assert!(!config.inherit_claude_config);
            assert_eq!(config.reasoning, None);
            assert!(matches!(config.tools, ClaudeToolPolicy::None));
        }
        _ => panic!("expected claude runtime"),
    }
}

// Covers: switching to cursor drops reasoning and starts with no tools so
// save cannot emit an unrestricted cursor-agent -p allow list.
// Owner: agent edit
#[test]
fn switching_to_cursor_resets_reasoning_and_requires_tools() {
    let mut draft = AgentDefinition {
        id: AgentId::new("switch-cursor").unwrap(),
        description: "switch agent".into(),
        prompt: PromptPolicy::Replace("body".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(
                [ToolCapability::ReadFile, ToolCapability::Shell]
                    .into_iter()
                    .collect(),
            ),
            model: ModelPolicy::Select(ModelSelection {
                provider: Some("openai".into()),
                model: "gpt-5.3-codex-high".into(),
                auth: None,
            }),
            reasoning: Some(ReasoningLevel::High),
        },
    };

    assert!(draft.switch_runtime_kind("cursor"));
    assert_eq!(draft.prompt, PromptPolicy::Extend("body".into()));
    match &draft.runtime {
        AgentRuntimeSpec::Cursor(config) => {
            assert_eq!(config.model.as_deref(), Some("gpt-5.3-codex-high"));
            assert_eq!(config.tools, Vec::<CursorTool>::new());
        }
        other => panic!("expected cursor runtime, got {other:?}"),
    }
    assert_eq!(draft.reasoning(), None);
    assert_eq!(
        draft.validate_for_edit().as_deref(),
        Some("cursor agents need at least one tool")
    );
    draft.set_tools_text("[read_tool_call]").unwrap();
    assert_eq!(
        draft.runtime,
        AgentRuntimeSpec::Cursor(CursorAgentConfig {
            tools: vec![CursorTool::Read],
            model: Some("gpt-5.3-codex-high".into()),
        })
    );
    assert_eq!(draft.validate_for_edit(), None);
    assert_eq!(
        draft.set_tools_text("[]").unwrap_err(),
        "cursor agents need at least one tool"
    );
}

// Covers: switching claude-cli -> rho resets tools and keeps model/reasoning
// Owner: agent edit
#[test]
fn switching_to_rho_keeps_compatible_fields() {
    let mut draft = AgentDefinition {
        id: AgentId::new("back").unwrap(),
        description: "back agent".into(),
        prompt: PromptPolicy::Extend("body".into()),
        runtime: AgentRuntimeSpec::ClaudeCli(ClaudeAgentConfig {
            tools: ClaudeToolPolicy::Allow(vec!["Read".into()]),
            inherit_claude_config: true,
            model: Some("opus".into()),
            reasoning: Some(ReasoningLevel::High),
        }),
    };

    assert!(draft.switch_runtime_kind("rho"));
    match &draft.runtime {
        AgentRuntimeSpec::Rho {
            tools,
            model,
            reasoning,
        } => {
            assert!(matches!(tools, ToolPolicy::All));
            assert!(matches!(model, ModelPolicy::Select(_)));
            assert_eq!(*reasoning, Some(ReasoningLevel::High));
        }
        _ => panic!("expected rho runtime"),
    }
}

// Covers: save round-trips and rejects stale sources
// Owner: agent edit
#[test]
fn save_definition_round_trips_and_detects_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("draft.md");
    let draft = rho_draft();
    let original = "---\ndescription: old\n---\nold body\n";
    std::fs::write(&path, original).unwrap();

    let contents = save_definition(&draft, &path, original).unwrap();
    let reparsed = parse_definition(&path, "draft", &contents).unwrap();
    assert_eq!(reparsed, draft);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    assert_eq!(
        save_definition(&draft, &path, original).unwrap_err(),
        SaveDefinitionError::Conflict
    );
    assert!(path.with_file_name(".draft.md.rho-edit.lock").exists());
}

// Covers: unlinking the sidecar on drop would split lock identity; a held
// writer, a failed contender, and a later writer must share one inode.
// Owner: agent edit lock
#[test]
fn save_lock_survives_drop_and_rejects_a_contender() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("draft.md");
    let lock_path = agent_lock_path(&path);
    let held = acquire_agent_file_lock(&path).unwrap();
    assert!(lock_path.exists());
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&lock_path).unwrap().ino()
    };

    let contender = save_definition(&rho_draft(), &path, "");
    assert!(matches!(contender, Err(SaveDefinitionError::Write(_))));
    assert!(!path.exists());

    drop(held);
    assert!(lock_path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&lock_path).unwrap().ino(), inode);
    }

    let contents = save_definition(&rho_draft(), &path, "").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    assert!(lock_path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&lock_path).unwrap().ino(), inode);
    }
}

// Covers: first save of a new agent creates parent directories.
// Owner: agent edit
#[test]
fn save_definition_creates_missing_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/agents/draft.md");
    let contents = save_definition(&rho_draft(), &path, "").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    let reparsed = parse_definition(&path, "draft", &contents).unwrap();
    assert_eq!(reparsed, rho_draft());
}

// Covers: saving through a symlink cannot modify the linked target
// Owner: agent edit
#[cfg(unix)]
#[test]
fn save_definition_rejects_a_symlink_destination() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.md");
    let path = dir.path().join("draft.md");
    let original = "---\ndescription: target\n---\nbody\n";
    std::fs::write(&target, original).unwrap();
    symlink(&target, &path).unwrap();

    let error = save_definition(&rho_draft(), &path, original).unwrap_err();
    assert!(matches!(error, SaveDefinitionError::Write(_)));
    assert_eq!(std::fs::read_to_string(target).unwrap(), original);
}

// Covers: tools text round-trips for both runtimes
// Owner: agent edit
#[test]
fn tools_text_round_trips_for_rho_and_claude() {
    let mut rho = rho_draft();
    rho.set_tools_text("[read_file, shell]").unwrap();
    match &rho.runtime {
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(capabilities),
            ..
        } => {
            assert!(capabilities.contains(&ToolCapability::ReadFile));
            assert!(capabilities.contains(&ToolCapability::Shell));
        }
        _ => panic!("expected rho allow tools"),
    }
    assert_eq!(rho.tools_text(), "[read_file, shell]");

    let mut claude = claude_draft();
    claude.set_tools_text("[Read, Edit]").unwrap();
    assert_eq!(claude.tools_text(), "[Read, Edit]");
}

// Covers: toggling a tool flips membership per runtime; rho `all` expands to
// the built-in set on the first removal and `set_tools_all` restores it.
// Owner: agent edit
#[test]
fn toggle_tool_flips_membership_per_runtime() {
    let mut rho = rho_draft();
    rho.toggle_tool("shell").unwrap();
    match &rho.runtime {
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(set),
            ..
        } => {
            assert_eq!(set.len(), BUILTIN_TOOL_CAPABILITIES.len() - 1);
            assert!(!set.contains(&ToolCapability::Shell));
        }
        other => panic!("expected allow set, got {other:?}"),
    }
    rho.toggle_tool("shell").unwrap();
    assert!(matches!(
        &rho.runtime,
        AgentRuntimeSpec::Rho { tools: ToolPolicy::Allow(set), .. } if set.contains(&ToolCapability::Shell)
    ));
    assert_eq!(
        rho.toggle_tool("Read").unwrap_err(),
        "unknown tool 'Read' for runtime: rho"
    );
    let narrow: ToolCapabilitySet = [ToolCapability::ReadFile].into_iter().collect();
    match rho.toggle_tools_all(narrow.clone()) {
        ToolsAllToggle::TurnedOn { replaced } => {
            assert_eq!(replaced.len(), BUILTIN_TOOL_CAPABILITIES.len())
        }
        other => panic!("expected TurnedOn, got {other:?}"),
    }
    assert!(matches!(
        rho.runtime,
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            ..
        }
    ));
    assert_eq!(
        rho.toggle_tools_all(narrow.clone()),
        ToolsAllToggle::TurnedOff
    );
    assert_eq!(
        rho.runtime,
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(narrow),
            model: ModelPolicy::Inherit,
            reasoning: None,
        }
    );

    let mut claude = claude_draft();
    claude.toggle_tool("Read").unwrap();
    claude.toggle_tool("Bash(git *)").unwrap();
    assert_eq!(
        claude.tools_text(),
        "[Read, Bash(git *)]",
        "unoffered names must survive the toggle so free-text edits see them"
    );
    claude.toggle_tool("Read").unwrap();
    claude.toggle_tool("Bash(git *)").unwrap();
    assert_eq!(claude.tools_text(), "[]");
    assert_eq!(
        claude.toggle_tools_all(ToolCapabilitySet::new()),
        ToolsAllToggle::Unsupported
    );

    let mut cursor = rho_draft();
    assert!(cursor.switch_runtime_kind("cursor"));
    assert!(cursor.toggle_tool("task_tool_call").is_err());
    cursor.toggle_tool("grep_tool_call").unwrap();
    cursor.toggle_tool("read_tool_call").unwrap();
    assert_eq!(cursor.tools_text(), "[grep_tool_call, read_tool_call]");
    cursor.toggle_tool("grep_tool_call").unwrap();
    assert_eq!(cursor.tools_text(), "[read_tool_call]");
}

// Covers: the nav badge collapses long lists to a count while the summary
// keeps every name for the detail pane.
// Owner: agent edit
#[test]
fn tools_badge_collapses_to_count_but_summary_keeps_names() {
    let mut rho = rho_draft();
    assert_eq!(
        (rho.tools_badge(), rho.tools_summary()),
        ("all".into(), "all".into())
    );

    rho.set_tools_text("[]").unwrap();
    assert_eq!(
        (rho.tools_badge(), rho.tools_summary()),
        ("none".into(), "none".into())
    );

    rho.set_tools_text("[read_file, grep, glob]").unwrap();
    assert_eq!(rho.tools_badge(), "glob, grep, read_file");

    rho.set_tools_text("[read_file, grep, glob, shell]")
        .unwrap();
    assert_eq!(rho.tools_badge(), "4 tools");
    assert_eq!(rho.tools_summary(), "glob, grep, read_file, shell");
}

// Covers: prompt policy preserves body
// Owner: agent edit
#[test]
fn prompt_policy_choice_preserves_body() {
    let mut draft = rho_draft();
    draft.prompt = PromptPolicy::Extend("keep me".into());
    assert!(draft.set_prompt_policy_kind("replace"));
    assert_eq!(draft.prompt, PromptPolicy::Replace("keep me".into()));
}

// Covers: validate_for_edit flags parser constraints early
// Owner: agent edit
#[test]
fn validate_for_edit_flags_overlong_description_and_empty_replace() {
    let mut draft = rho_draft();
    assert_eq!(draft.validate_for_edit(), None);
    draft.description = "x".repeat(1025);
    assert_eq!(
        draft.validate_for_edit().as_deref(),
        Some("description must be at most 1024 characters")
    );
    draft.description = "ok".into();
    draft.prompt = PromptPolicy::Replace(String::new());
    assert_eq!(
        draft.validate_for_edit().as_deref(),
        Some("prompt policy 'replace' requires a non-empty prompt body")
    );
}

// Covers: model text pins select policy for rho
// Owner: agent edit
#[test]
fn setting_model_text_pins_select_policy_for_rho() {
    let mut draft = rho_draft();
    draft.set_model_text("gpt-5.5".into());
    assert_eq!(
        draft.model_policy().as_ref(),
        &ModelPolicy::Select(ModelSelection {
            provider: None,
            model: "gpt-5.5".into(),
            auth: None,
        })
    );
}

// Covers: save rejects invalid drafts before writing
// Owner: agent edit
#[test]
fn save_definition_rejects_empty_replace_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.md");
    let draft = AgentDefinition {
        id: AgentId::new("bad").unwrap(),
        description: "bad draft".into(),
        prompt: PromptPolicy::Replace(String::new()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            model: ModelPolicy::Inherit,
            reasoning: None,
        },
    };
    let error = save_definition(&draft, &path, "").unwrap_err();
    assert!(matches!(error, SaveDefinitionError::Validation(_)));
    assert!(!path.exists());
}

// Covers: write errors surface for unwritable paths
// Owner: agent edit
#[test]
fn save_definition_reports_write_error_for_unwritable_path() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("not-a-directory");
    std::fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("draft.md");
    let error = save_definition(&rho_draft(), &path, "").unwrap_err();
    assert!(matches!(error, SaveDefinitionError::Write(_)));
}

// Covers: same runtime switch is a no-op
// Owner: agent edit
#[test]
fn switching_to_same_runtime_is_noop() {
    let mut draft = rho_draft();
    let before = draft.clone();
    assert!(draft.switch_runtime_kind("rho"));
    assert_eq!(draft, before);
    assert_eq!(draft.runtime.runtime(), AgentRuntime::Rho);
}

// Covers: auth selection pins profile and fills provider when needed
// Owner: agent edit
#[test]
fn set_auth_selection_pins_profile_and_provider() {
    let mut draft = rho_draft();
    draft.set_model_text("grok-4.5".into());
    assert!(draft.set_auth_selection(Some("xai-oauth".into())));
    assert_eq!(draft.provider_text(), "xai");
    assert_eq!(draft.auth_text(), "xai-oauth");
    assert!(draft.set_auth_selection(None));
    assert_eq!(draft.auth_text(), "");
    assert_eq!(draft.provider_text(), "xai");
}

// Covers: provider change drops auth pins that no longer fit
// Owner: agent edit
#[test]
fn set_provider_text_clears_incompatible_auth() {
    let mut draft = rho_draft();
    draft.set_model_text("grok-4.5".into());
    assert!(draft.set_auth_selection(Some("xai-oauth".into())));
    draft.set_provider_text("openai".into());
    assert_eq!(draft.provider_text(), "openai");
    assert_eq!(draft.auth_text(), "");
}

// Covers: set_model_selection keeps compatible auth and clears incompatible
// Owner: agent edit
#[test]
fn set_model_selection_preserves_compatible_auth_only() {
    let mut draft = rho_draft();
    draft.set_model_selection(Some(ModelSelection {
        provider: Some("xai".into()),
        model: "grok-4.5".into(),
        auth: Some("xai-oauth".into()),
    }));
    assert_eq!(draft.auth_text(), "xai-oauth");

    let mut next = draft.current_selection();
    next.provider = Some("openai".into());
    next.model = "gpt-5.5".into();
    next.auth = next.auth.filter(|auth| {
        rho_providers::provider::provider_accepts_auth(next.provider.as_deref().unwrap(), auth)
    });
    draft.set_model_selection(Some(next));
    assert_eq!(draft.provider_text(), "openai");
    assert_eq!(draft.model_text(), "gpt-5.5");
    assert_eq!(draft.auth_text(), "");
}
