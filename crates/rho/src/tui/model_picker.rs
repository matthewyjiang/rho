use super::{PickerBadge, PickerBadgeTone, PickerItem, PickerKeyHints, RuntimeModelView, UiPicker};
use crate::claude_runtime::models as claude_models;
use crate::config::CLAUDE_CLI_RUNTIME_KEY;
use crate::keybindings::Keybindings;
use rho_providers::model::{catalog, favorites};

/// Which models a conversation or internal-agent picker lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ModelPickerScope {
    #[default]
    All,
    Pinned,
}

impl ModelPickerScope {
    pub(super) fn other(self) -> Self {
        match self {
            Self::All => Self::Pinned,
            Self::Pinned => Self::All,
        }
    }

    fn title_suffix(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Pinned => "pinned",
        }
    }

    pub(super) fn status_label(self) -> &'static str {
        match self {
            Self::All => "all models",
            Self::Pinned => "pinned models",
        }
    }
}

/// The scope a picker can actually show: `Pinned` degrades to `All` when no
/// pin has auth.
///
/// This is the single place that decides whether the pinned view is usable.
/// Callers pass the result down; nothing re-derives it.
pub(super) fn effective_model_picker_scope(
    scope: ModelPickerScope,
    favorite_models: &[String],
    available_auths: &[String],
) -> ModelPickerScope {
    if scope == ModelPickerScope::All {
        return ModelPickerScope::All;
    }
    resolve_scope(
        scope,
        &favorites::normalized_favorite_models(favorite_models),
        &catalog::available_models_for_auths(available_auths),
    )
}

/// [`effective_model_picker_scope`] for callers that already normalized.
fn resolve_scope(
    scope: ModelPickerScope,
    favorites: &[favorites::FavoriteModel],
    available: &[catalog::ModelCatalogEntry],
) -> ModelPickerScope {
    if scope == ModelPickerScope::Pinned
        && !favorites::available_favorites(favorites, available).is_empty()
    {
        ModelPickerScope::Pinned
    } else {
        ModelPickerScope::All
    }
}

pub(super) fn model_picker(
    info: &RuntimeModelView,
    available_auths: &[String],
    scope: ModelPickerScope,
) -> UiPicker {
    model_picker_for_current(
        "select model",
        CurrentModel {
            provider: &info.provider,
            model: &info.model,
            badge: "selected",
        },
        &info.favorite_models,
        available_auths,
        /*internal_agent*/ false,
        scope,
        &info.keybindings,
    )
}

pub(super) fn model_picker_during_run(
    info: &RuntimeModelView,
    pending: Option<&rho_providers::model::catalog::ModelSelection>,
    available_auths: &[String],
    scope: ModelPickerScope,
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
        /*internal_agent*/ false,
        scope,
        &info.keybindings,
    )
}

pub(super) const USE_CONVERSATION_MODEL: &str = "Use conversation model";

/// Separates the runtime key from the pass-through model in a Claude Code row
/// value.
///
/// Claude Code is not a Rho provider, so its rows cannot be addressed by a
/// `provider/model` reference. They carry [`CLAUDE_CLI_RUNTIME_KEY`] instead,
/// so config and picker keep one spelling of the runtime.
const CLAUDE_CODE_ROW_MODEL_SEPARATOR: char = ':';

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

/// The row value a Claude Code row carries. Inverse of the Claude Code arm of
/// [`parse_internal_agent_model_row`].
fn claude_code_row_value(model: Option<&str>) -> String {
    format!(
        "{CLAUDE_CLI_RUNTIME_KEY}{CLAUDE_CODE_ROW_MODEL_SEPARATOR}{}",
        model.unwrap_or_default()
    )
}

pub(super) fn parse_internal_agent_model_row(value: &str) -> InternalAgentModelRow {
    if value == USE_CONVERSATION_MODEL {
        return InternalAgentModelRow::Conversation;
    }
    match value
        .strip_prefix(CLAUDE_CLI_RUNTIME_KEY)
        .and_then(|rest| rest.strip_prefix(CLAUDE_CODE_ROW_MODEL_SEPARATOR))
    {
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

impl InternalAgentSelection {
    /// The [`PickerItem::value`] of the row this selection marks, or `None`
    /// when nothing is configured.
    ///
    /// Encode half of the row vocabulary; [`parse_internal_agent_model_row`] is
    /// the decode half. The conversation row is not here because no selection
    /// names it: the caller offers it, so the caller asks for it by
    /// [`USE_CONVERSATION_MODEL`].
    fn row_value(&self) -> Option<String> {
        match self {
            Self::RhoModel { provider, model } => {
                Some(rho_providers::provider::model_reference(provider, model))
            }
            Self::ClaudeCode { model } => Some(claude_code_row_value(model.as_deref())),
            Self::Unset => None,
        }
    }
}

pub(super) struct InternalAgentPickerInputs<'a> {
    pub(super) agent_id: &'a str,
    pub(super) current: InternalAgentSelection,
    pub(super) conversation_model: ConversationModelRow,
    pub(super) claude_code: ClaudeCodeRows,
    pub(super) favorite_models: &'a [String],
    pub(super) available_auths: &'a [String],
    pub(super) scope: ModelPickerScope,
    pub(super) keybindings: &'a Keybindings,
}

pub(super) fn internal_agent_model_picker(inputs: InternalAgentPickerInputs<'_>) -> UiPicker {
    let InternalAgentPickerInputs {
        agent_id,
        current,
        conversation_model,
        claude_code,
        favorite_models,
        available_auths,
        scope,
        keybindings,
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
        /*internal_agent*/ true,
        scope,
        keybindings,
    );

    let mut leading = Vec::new();
    if let ConversationModelRow::Offered { selected } = conversation_model {
        leading.push(conversation_model_row(selected));
    }
    if claude_code == ClaudeCodeRows::Offered {
        leading.extend(claude_code_rows(&current));
    }
    picker.items.splice(0..0, leading);

    // A selected conversation row outranks `current`, which then still carries
    // the conversation model and so also names a catalog row.
    let wanted_value = match conversation_model {
        ConversationModelRow::Offered { selected: true } => {
            Some(USE_CONVERSATION_MODEL.to_string())
        }
        ConversationModelRow::Offered { selected: false } | ConversationModelRow::Omitted => {
            current.row_value()
        }
    };
    picker.selected = wanted_value
        .and_then(|value| picker.items.iter().position(|item| item.value == value))
        .unwrap_or(0);
    picker
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
        value: claude_code_row_value(model),
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
    if let Some(Some(model)) = selected_model {
        if !claude_models::CLAUDE_MODEL_ALIASES
            .iter()
            .any(|alias| alias.name == model)
        {
            rows.push(row(Some(model), CLAUDE_CODE_ROW_DETAIL.into()));
        }
    }
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
    internal_agent: bool,
    scope: ModelPickerScope,
    keybindings: &Keybindings,
) -> UiPicker {
    let CurrentModel {
        provider: current_provider,
        model: current_model,
        badge: selected_badge,
    } = current;
    let current = rho_providers::provider::model_reference(current_provider, current_model);
    let favorites = favorites::normalized_favorite_models(favorite_models);
    let mut available = catalog::available_models_for_auths(available_auths);
    let effective_scope = resolve_scope(scope, &favorites, &available);
    if effective_scope == ModelPickerScope::Pinned {
        available.retain(|entry| favorites::is_favorite(&favorites, &entry.provider, &entry.model));
    }
    let pin_key = keybindings.cycle_pinned_model.chrome_label();
    let scope_key = keybindings.toggle_tool_output.chrome_label();
    let catalog_items = favorites::reorder_models_by_favorites(available, &favorites)
        .into_iter()
        .map(|entry| {
            let value = rho_providers::provider::model_reference(&entry.provider, &entry.model);
            let pinned = favorites::is_favorite(&favorites, &entry.provider, &entry.model);
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
                    format!("Press {pin_key} to unpin this model.")
                } else {
                    format!("Press {pin_key} to pin this model to the top of model pickers.")
                }),
                preview: None,
                badge,
                value,
                selection_verb: None,
            }
        })
        .collect::<Vec<_>>();

    let title = format!("{title} · {}", effective_scope.title_suffix());
    let mut picker = if internal_agent {
        UiPicker::internal_agent_models(title, catalog_items)
    } else {
        UiPicker::models(title, catalog_items)
    }
    .with_key_hints(PickerKeyHints {
        pin_toggle: Some(pin_key),
        scope_toggle: Some(scope_key),
        tab_complete: true,
        ..Default::default()
    });
    if let Some(index) = picker.items.iter().position(|item| item.value == current) {
        picker.selected = index;
    }
    picker
}

#[cfg(test)]
#[path = "model_picker_tests.rs"]
mod tests;
