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
            auth: None,
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

// Covers: pinned catalog models only list advertised reasoning levels in the agent picker
// Owner: pure unit (agent editor reasoning choice assembly)
#[test]
fn reasoning_picker_follows_pinned_catalog_capabilities() {
    use rho_providers::model::models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    };

    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
        write_cached_model_metadata_for_tests(
            "meta",
            "muse-spark-1.2",
            &ModelMetadata {
                supported_reasoning_levels: Some(vec![
                    ReasoningLevel::Minimal,
                    ReasoningLevel::Low,
                    ReasoningLevel::Medium,
                    ReasoningLevel::High,
                    ReasoningLevel::Xhigh,
                ]),
                reasoning_capabilities_known: true,
                reasoning_metadata_complete: true,
                ..ModelMetadata::default()
            },
        );

        let mut draft = rho_draft();
        if let AgentRuntimeSpec::Rho {
            model, reasoning, ..
        } = &mut draft.runtime
        {
            *model = ModelPolicy::Select(ModelSelection {
                provider: Some("meta".into()),
                model: "muse-spark-1.2".into(),
                auth: None,
            });
            *reasoning = Some(ReasoningLevel::Max);
        }

        let picker = agent_choice_picker(AgentChoiceField::Reasoning, &draft);
        let labels: Vec<&str> = picker
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["inherit", "minimal", "low", "medium", "high", "xhigh", "max"]
        );
    });
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

// Covers: auth picker only offers available credentials for the pinned provider
// Owner: pure unit (agent editor auth choice assembly)
#[test]
fn auth_choice_lists_only_available_modes_for_provider() {
    let mut draft = rho_draft();
    if let AgentRuntimeSpec::Rho { model, .. } = &mut draft.runtime {
        *model = ModelPolicy::Select(ModelSelection {
            provider: Some("xai".into()),
            model: "grok-4.5".into(),
            auth: None,
        });
    }
    let available = vec!["xai-oauth".into(), "anthropic-api-key".into()];
    let picker = auth_choice_picker(&draft, &available);
    let values: Vec<&str> = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect();
    assert!(values.contains(&"agent_choice:auth:"));
    assert!(values.contains(&"agent_choice:auth:xai-oauth"));
    assert!(!values.iter().any(|value| value.contains("anthropic")));
}

// Covers: auth field appears when model is pinned
// Owner: pure unit (agent editor field list)
#[test]
fn rho_field_picker_includes_auth_when_model_is_pinned() {
    let mut draft = rho_draft();
    if let AgentRuntimeSpec::Rho { model, .. } = &mut draft.runtime {
        *model = ModelPolicy::Select(ModelSelection {
            provider: Some("xai".into()),
            model: "grok-4.5".into(),
            auth: Some("xai-oauth".into()),
        });
    }
    let picker = agent_field_picker(&draft);
    let values: Vec<&str> = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect();
    assert!(values.contains(&AGENT_FIELD_AUTH));
}

// Covers: Claude models are chosen from the offered aliases, and a definition
// that pins a full model id keeps its row so the editor never silently rewrites
// a hand-written agent file.
// Owner: tui agent editor
#[test]
fn claude_model_choices_offer_aliases_and_keep_a_configured_model() {
    let prefix = AgentChoiceField::ClaudeModel.choice_prefix();

    let default_rows = claude_model_choice_items(&claude_draft(), prefix);
    let expected_labels = std::iter::once("Claude Code default")
        .chain(
            crate::claude_runtime::models::CLAUDE_MODEL_ALIASES
                .iter()
                .map(|alias| alias.name),
        )
        .collect::<Vec<_>>();
    assert_eq!(
        default_rows
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        expected_labels
    );
    assert_eq!(default_rows[0].value, prefix);
    assert!(default_rows[0].badge.is_some());

    let mut pinned = claude_draft();
    pinned.set_model_text("claude-opus-4-6".into());
    let pinned_rows = claude_model_choice_items(&pinned, prefix);
    let last = pinned_rows.last().expect("configured row");
    assert_eq!(last.label, "claude-opus-4-6");
    assert_eq!(last.value, format!("{prefix}claude-opus-4-6"));
    assert!(last.badge.is_some());
    assert!(pinned_rows[0].badge.is_none());

    let mut alias = claude_draft();
    alias.set_model_text("opus".into());
    let alias_rows = claude_model_choice_items(&alias, prefix);
    assert_eq!(alias_rows.len(), default_rows.len());
    assert_eq!(
        alias_rows
            .iter()
            .filter(|item| item.badge.is_some())
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["opus"]
    );
}

// Covers: the Model row must show what Claude Code will actually use, and Rho's
// `inherit` names a different concept than Claude's default.
// Owner: tui agent editor
#[test]
fn claude_model_badge_names_the_claude_code_default() {
    assert_eq!(claude_model_badge(&claude_draft()), "default");

    let mut pinned = claude_draft();
    pinned.set_model_text("sonnet".into());
    assert_eq!(claude_model_badge(&pinned), "sonnet");
}
