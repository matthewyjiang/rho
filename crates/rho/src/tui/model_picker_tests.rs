use pretty_assertions::assert_eq;

use super::*;

fn inputs(
    current: InternalAgentSelection,
    conversation_model: ConversationModelRow,
    claude_code: ClaudeCodeRows,
) -> InternalAgentPickerInputs<'static> {
    InternalAgentPickerInputs {
        agent_id: "advisor",
        current,
        conversation_model,
        claude_code,
        favorite_models: &[],
        available_auths: &[],
    }
}

fn labels(picker: &UiPicker) -> Vec<&str> {
    picker
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect()
}

fn selected_label(picker: &UiPicker) -> &str {
    picker.items[picker.selected].label.as_str()
}

// Covers: choosing a Claude Code row is the only way a user selects the
// delegated runtime, so the rows must appear when offered, stay out when the
// agent cannot delegate, and never depend on the Rho model catalog.
// Owner: internal agent model picker
#[test]
fn claude_code_rows_appear_only_when_the_agent_can_delegate() {
    let offered = internal_agent_model_picker(inputs(
        InternalAgentSelection::Unset,
        ConversationModelRow::Omitted,
        ClaudeCodeRows::Offered,
    ));
    let expected_labels = std::iter::once("claude-code/default".to_string())
        .chain(
            crate::claude_runtime::models::CLAUDE_MODEL_ALIASES
                .iter()
                .map(|alias| format!("claude-code/{}", alias.name)),
        )
        .collect::<Vec<_>>();
    assert_eq!(
        labels(&offered),
        expected_labels
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );

    let omitted = internal_agent_model_picker(inputs(
        InternalAgentSelection::Unset,
        ConversationModelRow::Omitted,
        ClaudeCodeRows::Omitted,
    ));
    assert_eq!(labels(&omitted), Vec::<&str>::new());
}

// Covers: the picker must open on the row that is actually configured, so a
// user does not overwrite a delegating advisor by pressing Enter. A selection
// with no row at all - a Rho model the catalog cannot offer - must fall back to
// the first row rather than point at an unrelated one.
// Owner: internal agent model picker
#[test]
fn the_picker_opens_on_the_configured_row() {
    let cases = [
        (
            InternalAgentSelection::ClaudeCode {
                model: Some("sonnet".into()),
            },
            ConversationModelRow::Omitted,
            "claude-code/sonnet",
        ),
        (
            InternalAgentSelection::ClaudeCode { model: None },
            ConversationModelRow::Omitted,
            "claude-code/default",
        ),
        (
            InternalAgentSelection::ClaudeCode {
                model: Some("claude-opus-4-6".into()),
            },
            ConversationModelRow::Omitted,
            "claude-code/claude-opus-4-6",
        ),
        (
            InternalAgentSelection::Unset,
            ConversationModelRow::Offered { selected: true },
            USE_CONVERSATION_MODEL,
        ),
        (
            InternalAgentSelection::RhoModel {
                provider: "anthropic".into(),
                model: "claude-fable-5".into(),
            },
            ConversationModelRow::Omitted,
            "claude-code/default",
        ),
    ];

    for (current, conversation_model, expected) in cases {
        let picker = internal_agent_model_picker(inputs(
            current.clone(),
            conversation_model,
            ClaudeCodeRows::Offered,
        ));
        assert_eq!(selected_label(&picker), expected, "{current:?}");
    }
}

// Covers: a selected row has to route to the right runtime; a Claude row must
// never be resolved against the Rho catalog and the reverse.
// Owner: internal agent model picker
#[test]
fn each_row_value_routes_to_its_runtime() {
    let picker = internal_agent_model_picker(inputs(
        InternalAgentSelection::Unset,
        ConversationModelRow::Offered { selected: true },
        ClaudeCodeRows::Offered,
    ));
    let rows = picker
        .items
        .iter()
        .map(|item| parse_internal_agent_model_row(&item.value))
        .collect::<Vec<_>>();

    let expected_rows = std::iter::once(InternalAgentModelRow::Conversation)
        .chain(std::iter::once(InternalAgentModelRow::ClaudeCode {
            model: None,
        }))
        .chain(
            crate::claude_runtime::models::CLAUDE_MODEL_ALIASES
                .iter()
                .map(|alias| InternalAgentModelRow::ClaudeCode {
                    model: Some(alias.name.into()),
                }),
        )
        .collect::<Vec<_>>();
    assert_eq!(rows, expected_rows);
    assert_eq!(
        parse_internal_agent_model_row("anthropic/claude-fable-5"),
        InternalAgentModelRow::RhoModel("anthropic/claude-fable-5".into())
    );
}
