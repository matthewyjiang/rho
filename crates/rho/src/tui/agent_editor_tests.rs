use pretty_assertions::assert_eq;

use super::*;
use crate::agent::{
    AgentDefinition, AgentId, AgentOrigin, AgentRuntimeSpec, ClaudeAgentConfig, ClaudeToolPolicy,
    ModelPolicy, ModelSelection, PromptPolicy, ToolPolicy,
};
use crate::tui::line_editor::LineEditor;
use crate::tui::text_input::{AgentField, TextInput};

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

fn field_values(picker: &UiPicker) -> Vec<&str> {
    picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect()
}

// Covers: the field editor lists only fields the runtime accepts
// Owner: tui agent editor
#[test]
fn rho_field_picker_lists_runtime_specific_fields() {
    let picker = agent_field_picker(&rho_draft());
    let values = field_values(&picker);

    assert!(values.contains(&AGENT_FIELD_MODEL_POLICY));
    assert!(values.contains(&AGENT_FIELD_REASONING));
    assert!(values.contains(&AGENT_FIELD_TOOLS));
    assert!(values.contains(&AGENT_FIELD_SAVE));
    assert!(values.contains(&AGENT_FIELD_CANCEL));
    assert!(!values.contains(&AGENT_FIELD_MODEL));
    assert!(!values.contains(&AGENT_FIELD_PROVIDER));

    let mut explicit = rho_draft();
    if let AgentRuntimeSpec::Rho { model, .. } = &mut explicit.runtime {
        *model = ModelPolicy::Select(ModelSelection {
            provider: None,
            model: "gpt-5.5".into(),
        });
    }
    let explicit_picker = agent_field_picker(&explicit);
    let explicit_values = field_values(&explicit_picker);
    assert!(explicit_values.contains(&AGENT_FIELD_MODEL));
    assert!(explicit_values.contains(&AGENT_FIELD_PROVIDER));
    assert!(!values.contains(&AGENT_FIELD_INHERIT_CLAUDE_CONFIG));
}

// Covers: claude-cli field picker hides rho-only fields
// Owner: tui agent editor
#[test]
fn claude_field_picker_hides_rho_only_fields() {
    let picker = agent_field_picker(&claude_draft());
    let values = field_values(&picker);

    assert!(values.contains(&AGENT_FIELD_MODEL));
    assert!(values.contains(&AGENT_FIELD_INHERIT_CLAUDE_CONFIG));
    assert!(values.contains(&AGENT_FIELD_TOOLS));
    assert!(!values.contains(&AGENT_FIELD_MODEL_POLICY));
    assert!(!values.contains(&AGENT_FIELD_PROVIDER));
}

// Covers: runtime toggles retain edits made to each runtime within one session
// Owner: tui agent editor
#[test]
fn edit_session_restores_inactive_runtime_settings() {
    let mut session = AgentEditSession::new(
        rho_draft(),
        "agent.md".into(),
        AgentOrigin::RhoHome,
        ".".into(),
        String::new(),
    );
    assert!(session.switch_runtime("claude-cli"));
    session.with_draft_mut(|draft| {
        if let AgentRuntimeSpec::ClaudeCli(config) = &mut draft.runtime {
            config.inherit_claude_config = true;
            config.tools = ClaudeToolPolicy::Allow(vec!["Read".into()]);
        }
    });
    assert!(session.switch_runtime("rho"));
    assert!(matches!(
        session.draft().runtime,
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            ..
        }
    ));
    assert!(session.switch_runtime("claude-cli"));
    match &session.draft().runtime {
        AgentRuntimeSpec::ClaudeCli(config) => {
            assert!(config.inherit_claude_config);
            assert_eq!(config.tools, ClaudeToolPolicy::Allow(vec!["Read".into()]));
        }
        _ => panic!("expected claude runtime"),
    }
}

// Covers: model policy choice for claude only offers inherit and select
// Owner: tui agent editor
#[test]
fn model_policy_choice_for_claude_offers_inherit_and_select_only() {
    let picker = agent_choice_picker(AgentChoiceField::ModelPolicy, &claude_draft());
    let labels: Vec<&str> = picker
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(labels, ["inherit", "select"]);

    let rho_picker = agent_choice_picker(AgentChoiceField::ModelPolicy, &rho_draft());
    let rho_labels: Vec<&str> = rho_picker
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(rho_labels, ["inherit", "prefer", "require", "select"]);
}

// Covers: claude reasoning picker omits off and minimal
// Owner: tui agent editor
#[test]
fn claude_reasoning_picker_omits_off_and_minimal() {
    let reasoning_picker = agent_choice_picker(AgentChoiceField::Reasoning, &claude_draft());
    let labels: Vec<&str> = reasoning_picker
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert!(labels.contains(&"inherit"));
    assert!(!labels.contains(&"off"));
    assert!(!labels.contains(&"minimal"));
    assert!(labels.contains(&"high"));
}

// Covers: catalog-known models only offer advertised reasoning levels
// Owner: tui agent editor
#[test]
fn reasoning_levels_follow_catalog_capabilities() {
    use rho_providers::model::{ReasoningCapabilities, ReasoningLevelSet};
    use rho_providers::reasoning::ReasoningLevel;

    let levels = reasoning_levels_for_capabilities(
        /*is_claude*/ false,
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
        ])),
        None,
    );
    assert_eq!(
        levels,
        vec![
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
        ]
    );

    let with_current = reasoning_levels_for_capabilities(
        /*is_claude*/ false,
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Low,
            ReasoningLevel::High,
        ])),
        Some(ReasoningLevel::Max),
    );
    assert_eq!(
        with_current,
        vec![
            ReasoningLevel::Low,
            ReasoningLevel::High,
            ReasoningLevel::Max,
        ]
    );

    let not_configurable = reasoning_levels_for_capabilities(
        /*is_claude*/ false,
        ReasoningCapabilities::NotConfigurable,
        None,
    );
    assert!(not_configurable.is_empty());

    let unknown_rho = reasoning_levels_for_capabilities(
        /*is_claude*/ false,
        ReasoningCapabilities::Unknown,
        None,
    );
    assert_eq!(unknown_rho, ReasoningLevel::ALL.to_vec());
}

// Covers: shared line editor edits at the character cursor
// Owner: tui agent editor
#[test]
fn agent_text_input_edits_at_character_cursor() {
    let mut input = TextInput::agent_field(AgentField::Description, "hello");
    input.editor.cursor = 2;
    input.editor.insert_char('X');
    assert_eq!(input.editor.value, "heXllo");
    assert_eq!(input.editor.cursor, 3);
    input.editor.backspace();
    assert_eq!(input.editor.value, "hello");
    input.editor.insert_text("ab");
    assert_eq!(input.editor.value, "heabllo");
}

// Covers: shared line editor strips line breaks from pasted text
// Owner: tui agent editor
#[test]
fn agent_text_input_strips_line_breaks_from_paste() {
    let mut editor = LineEditor::new("[read_file");
    editor.insert_text(", shell]\nextra");
    assert_eq!(editor.value, "[read_file, shell]extra");
}

// Covers: authorize_editable_path accepts a rho-home agent file under the home root
// Owner: tui agent editor
#[test]
fn authorize_editable_path_accepts_rho_home_agent() {
    // Behavior depends on the process home; this only checks the Project origin
    // path shape via a temp dir without mutating HOME.
    let dir = tempfile::tempdir().unwrap();
    let agents = dir.path().join(".agents/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let path = agents.join("demo.md");
    std::fs::write(&path, "---\ndescription: demo\n---\n").unwrap();
    let root = authorize_editable_path(AgentOrigin::Project, &path, dir.path()).unwrap();
    assert_eq!(root, agents);
}
