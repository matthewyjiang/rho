use pretty_assertions::assert_eq;

use super::*;
use crate::agent::{
    AgentId, ClaudeAgentConfig, ClaudeToolPolicy, CursorAgentConfig, ModelPolicy, PromptPolicy,
    ToolCapability,
};

fn draft(runtime: AgentRuntimeSpec) -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new("draft").unwrap(),
        description: "draft".into(),
        prompt: PromptPolicy::Extend("body".into()),
        runtime,
    }
}

fn on_labels(picker: &UiPicker) -> Vec<&str> {
    picker
        .items
        .iter()
        .filter(|item| item.badge.is_some())
        .map(|item| item.label.as_str())
        .collect()
}

// Covers: each runtime's tools picker lists its own vocabulary, marks the
// draft's current allow list, and keeps Claude names Rho does not offer so a
// hand-written specifier can be switched off without retyping the list.
// Owner: tui agent tools picker
#[test]
fn tools_picker_marks_current_allow_list_per_runtime() {
    struct Case {
        name: &'static str,
        runtime: AgentRuntimeSpec,
        expected_on: &'static [&'static str],
        expected_len: usize,
    }
    let cases = [
        Case {
            name: "rho all marks every row",
            runtime: AgentRuntimeSpec::Rho {
                tools: ToolPolicy::All,
                model: ModelPolicy::Inherit,
                reasoning: None,
            },
            expected_on: &[
                "all",
                "advisor",
                "agent",
                "agents",
                "bash",
                "edit",
                "fetch_content",
                "get_search_content",
                "glob",
                "grep",
                "list_dir",
                "powershell",
                "process",
                "questionnaire",
                "read_file",
                "rho",
                "shell",
                "skill",
                "web_search",
                "workflow",
                "write",
            ],
            expected_len: BUILTIN_TOOL_CAPABILITIES.len() + 1,
        },
        Case {
            name: "rho allow marks listed rows only",
            runtime: AgentRuntimeSpec::Rho {
                tools: ToolPolicy::Allow(
                    [ToolCapability::ReadFile, ToolCapability::Shell]
                        .into_iter()
                        .collect(),
                ),
                model: ModelPolicy::Inherit,
                reasoning: None,
            },
            expected_on: &["read_file", "shell"],
            expected_len: BUILTIN_TOOL_CAPABILITIES.len() + 1,
        },
        Case {
            name: "claude keeps an unoffered specifier row",
            runtime: AgentRuntimeSpec::ClaudeCli(ClaudeAgentConfig {
                tools: ClaudeToolPolicy::Allow(vec!["Read".into(), "Bash(git *)".into()]),
                inherit_claude_config: false,
                model: None,
                reasoning: None,
            }),
            expected_on: &["Read", "Bash(git *)"],
            expected_len: claude_tools::CLAUDE_TOOLS.len() + 2,
        },
        Case {
            name: "cursor lists the closed set",
            runtime: AgentRuntimeSpec::Cursor(CursorAgentConfig {
                tools: vec![CursorTool::Grep],
                model: None,
            }),
            expected_on: &["grep_tool_call"],
            expected_len: CursorTool::ALL.len(),
        },
    ];
    for case in cases {
        let picker = agent_tools_picker(&draft(case.runtime));
        assert_eq!(on_labels(&picker), case.expected_on, "{}", case.name);
        assert_eq!(picker.items.len(), case.expected_len, "{}", case.name);
        assert!(picker.space_confirms_selection(), "{}", case.name);
    }
}

// Covers: only Claude offers the free-text escape and only Rho offers `all`.
// Owner: tui agent tools picker
#[test]
fn tools_picker_escape_rows_follow_runtime() {
    let rho = agent_tools_picker(&draft(AgentRuntimeSpec::Rho {
        tools: ToolPolicy::All,
        model: ModelPolicy::Inherit,
        reasoning: None,
    }));
    let claude = agent_tools_picker(&draft(AgentRuntimeSpec::ClaudeCli(ClaudeAgentConfig {
        tools: ClaudeToolPolicy::None,
        inherit_claude_config: false,
        model: None,
        reasoning: None,
    })));
    let cursor = agent_tools_picker(&draft(AgentRuntimeSpec::Cursor(CursorAgentConfig {
        tools: vec![CursorTool::Read],
        model: None,
    })));
    let has = |picker: &UiPicker, value: &str| picker.items.iter().any(|item| item.value == value);
    assert!(has(&rho, AGENT_TOOL_ALL));
    assert!(!has(&rho, AGENT_TOOL_OTHER));
    assert!(!has(&claude, AGENT_TOOL_ALL));
    assert!(has(&claude, AGENT_TOOL_OTHER));
    assert!(!has(&cursor, AGENT_TOOL_ALL));
    assert!(!has(&cursor, AGENT_TOOL_OTHER));
}
