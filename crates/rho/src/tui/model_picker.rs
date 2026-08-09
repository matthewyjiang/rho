use super::{
    PickerAction, PickerBadge, PickerBadgeTone, PickerItem, PickerKeyHints, RuntimeModelView,
    UiPicker,
};
use crate::claude_runtime::models as claude_models;
use rho_providers::model::{catalog, favorites};

pub(super) fn model_picker(info: &RuntimeModelView, available_auths: &[String]) -> UiPicker {
    model_picker_for_current(
        "select model",
        CurrentModel {
            provider: &info.provider,
            model: &info.model,
            badge: "selected",
        },
        &info.favorite_models,
        available_auths,
        PickerAction::SelectModel,
    )
}

pub(super) fn model_picker_during_run(
    info: &RuntimeModelView,
    pending: Option<&rho_providers::model::catalog::ModelSelection>,
    available_auths: &[String],
) -> UiPicker {
    let (provider, model, badge) = pending
        .map(|selection| {
            (
                selection.provider.as_str(),
                selection.model.as_str(),
                "pending",
            )
        })
        .unwrap_or((&info.provider, &info.model, "selected"));
    model_picker_for_current(
        "select model for next turn",
        CurrentModel {
            provider,
            model,
            badge,
        },
        &info.favorite_models,
        available_auths,
        PickerAction::SelectModel,
    )
}

pub(super) const USE_CONVERSATION_MODEL: &str = "Use conversation model";

/// Row value prefix for Claude Code rows. Claude Code is not a Rho provider, so
/// its rows cannot be addressed by a `provider/model` reference.
const CLAUDE_CODE_ROW_PREFIX: &str = "claude-cli:";

/// What a selected internal-agent model row asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum InternalAgentModelRow {
    /// Clear the override and follow the conversation model.
    Conversation,
    /// Delegate to the Claude Code CLI. `None` omits `--model`.
    ClaudeCode { model: Option<String> },
    /// A Rho `provider/model` reference to resolve against the catalog.
    RhoModel(String),
}

pub(super) fn parse_internal_agent_model_row(value: &str) -> InternalAgentModelRow {
    if value == USE_CONVERSATION_MODEL {
        return InternalAgentModelRow::Conversation;
    }
    match value.strip_prefix(CLAUDE_CODE_ROW_PREFIX) {
        Some(model) => InternalAgentModelRow::ClaudeCode {
            model: (!model.is_empty()).then(|| model.to_string()),
        },
        None => InternalAgentModelRow::RhoModel(value.to_string()),
    }
}

/// Whether an internal-agent model picker offers the conversation-model row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConversationModelRow {
    /// The agent follows the conversation model when it has no own model.
    Offered { selected: bool },
    /// The agent has no conversation-model fallback, so the row would lie.
    Omitted,
}

/// Whether an internal-agent model picker offers Claude Code rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClaudeCodeRows {
    /// This agent may delegate and the `claude` binary is installed.
    Offered,
    /// This agent cannot delegate, or Claude Code is not installed.
    Omitted,
}

/// Which row an internal agent's picker marks as selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum InternalAgentSelection {
    /// A Rho provider and model.
    RhoModel { provider: String, model: String },
    /// Claude Code, with the pass-through model it pins.
    ClaudeCode { model: Option<String> },
    /// Nothing configured yet.
    Unset,
}

pub(super) struct InternalAgentPickerInputs<'a> {
    pub(super) agent_id: &'a str,
    pub(super) current: InternalAgentSelection,
    pub(super) conversation_model: ConversationModelRow,
    pub(super) claude_code: ClaudeCodeRows,
    pub(super) favorite_models: &'a [String],
    pub(super) available_auths: &'a [String],
}

pub(super) fn internal_agent_model_picker(inputs: InternalAgentPickerInputs<'_>) -> UiPicker {
    let InternalAgentPickerInputs {
        agent_id,
        current,
        conversation_model,
        claude_code,
        favorite_models,
        available_auths,
    } = inputs;
    let rho_current = match &current {
        InternalAgentSelection::RhoModel { provider, model } => (provider.as_str(), model.as_str()),
        InternalAgentSelection::ClaudeCode { .. } | InternalAgentSelection::Unset => ("", ""),
    };
    let mut picker = model_picker_for_current(
        &format!("select model for {agent_id}"),
        CurrentModel {
            provider: rho_current.0,
            model: rho_current.1,
            badge: "selected",
        },
        favorite_models,
        available_auths,
        PickerAction::SelectInternalAgentModel,
    );

    // Leading rows are inserted in reverse so each lands above the last: the
    // conversation row stays first when both are present.
    let mut leading = 0;
    if claude_code == ClaudeCodeRows::Offered {
        let rows = claude_code_rows(&current);
        leading += rows.len();
        picker.items.splice(0..0, rows);
    }
    if let ConversationModelRow::Offered { selected } = conversation_model {
        leading += 1;
        picker.items.insert(0, conversation_model_row(selected));
    }

    picker.selected = picker
        .items
        .iter()
        .position(|item| item.badge.is_some() && item.badge.as_ref().is_some_and(is_selected_badge))
        .unwrap_or(if leading > 0 { 0 } else { picker.selected });
    picker
}

fn is_selected_badge(badge: &PickerBadge) -> bool {
    badge.tone == PickerBadgeTone::Selected && badge.text.ends_with("selected")
}

fn conversation_model_row(selected: bool) -> PickerItem {
    PickerItem {
        section: None,
        label: USE_CONVERSATION_MODEL.into(),
        detail: Some("Follow the active conversation provider, model, and auth.".into()),
        preview: None,
        badge: selected.then_some(PickerBadge {
            text: "selected".into(),
            tone: PickerBadgeTone::Selected,
        }),
        value: USE_CONVERSATION_MODEL.into(),
        selection_verb: None,
    }
}

/// Claude Code rows: the default, then each offered alias.
///
/// Choosing one also chooses the runtime, so the user never has to know that
/// Claude Code is a separate harness. Cost lands on the Claude subscription
/// rather than a Rho provider, which the detail text says outright.
fn claude_code_rows(current: &InternalAgentSelection) -> Vec<PickerItem> {
    let selected_model = match current {
        InternalAgentSelection::ClaudeCode { model } => Some(model.as_deref()),
        InternalAgentSelection::RhoModel { .. } | InternalAgentSelection::Unset => None,
    };
    let row = |model: Option<&str>, detail: String| PickerItem {
        section: None,
        label: rho_providers::provider::model_reference(
            claude_models::CLAUDE_CODE_SOURCE_LABEL,
            model.unwrap_or(claude_models::CLAUDE_DEFAULT_MODEL_BADGE),
        ),
        detail: Some(detail),
        preview: None,
        badge: (selected_model == Some(model)).then_some(PickerBadge {
            text: "selected".into(),
            tone: PickerBadgeTone::Selected,
        }),
        value: format!("{CLAUDE_CODE_ROW_PREFIX}{}", model.unwrap_or_default()),
        selection_verb: None,
    };
    let mut rows = vec![row(
        None,
        format!(
            "{CLAUDE_CODE_ROW_DETAIL} {}",
            claude_models::CLAUDE_DEFAULT_MODEL_DETAIL
        ),
    )];
    rows.extend(claude_models::CLAUDE_MODEL_ALIASES.iter().map(|alias| {
        row(
            Some(alias.name),
            format!("{CLAUDE_CODE_ROW_DETAIL} {}", alias.detail),
        )
    }));
    rows
}

const CLAUDE_CODE_ROW_DETAIL: &str =
    "Runs on the installed claude binary and bills to your Claude subscription. Sign in with /login claude-code.";

struct CurrentModel<'a> {
    provider: &'a str,
    model: &'a str,
    badge: &'a str,
}

fn model_picker_for_current(
    title: &str,
    current: CurrentModel<'_>,
    favorite_models: &[String],
    available_auths: &[String],
    action: PickerAction,
) -> UiPicker {
    let CurrentModel {
        provider: current_provider,
        model: current_model,
        badge: selected_badge,
    } = current;
    let current = rho_providers::provider::model_reference(current_provider, current_model);
    let favorites = favorites::normalized_favorite_models(favorite_models);
    let items = favorites::reorder_models_by_favorites(
        catalog::available_models_for_auths(available_auths),
        &favorites,
    )
    .into_iter()
    .map(|entry| {
        let value = rho_providers::provider::model_reference(&entry.provider, &entry.model);
        let pinned = favorites
            .iter()
            .any(|favorite| favorite.matches(&entry.provider, &entry.model));
        let selected = entry.provider == current_provider && entry.model == current_model;
        let badge = match (pinned, selected) {
            (true, true) => Some(PickerBadge {
                text: format!("pinned, {selected_badge}"),
                tone: PickerBadgeTone::Selected,
            }),
            (true, false) => Some(PickerBadge {
                text: "pinned".into(),
                tone: PickerBadgeTone::Favorite,
            }),
            (false, true) => Some(PickerBadge {
                text: selected_badge.into(),
                tone: PickerBadgeTone::Selected,
            }),
            (false, false) => None,
        };
        PickerItem {
            section: None,
            label: value.clone(),
            detail: Some(if pinned {
                "Press Ctrl-P to unpin this model.".into()
            } else {
                "Press Ctrl-P to pin this model to the top of model pickers.".into()
            }),
            preview: None,
            badge,
            value,
            selection_verb: None,
        }
    })
    .collect::<Vec<_>>();

    let mut picker = UiPicker::new(title, items, action).with_key_hints(PickerKeyHints {
        pin_toggle: true,
        tab_complete: true,
        row_delete: false,
    });
    if let Some(index) = picker.items.iter().position(|item| item.value == current) {
        picker.selected = index;
    }
    picker
}

#[cfg(test)]
#[path = "model_picker_tests.rs"]
mod tests;
