use super::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, TuiInfo, UiPicker};
use rho_providers::model::{catalog, favorites};

pub(super) fn model_picker(info: &TuiInfo, available_auths: &[String]) -> UiPicker {
    model_picker_for_current(
        "select model",
        "type fuzzy search, ctrl-p pin/unpin, tab complete, up/down select, enter confirm, esc cancel",
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
    info: &TuiInfo,
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
        "current run keeps its model; selection applies after it fully ends, ctrl-p pin/unpin, enter confirm, esc cancel",
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

pub(super) fn title_model_picker(
    current_provider: &str,
    current_model: &str,
    favorite_models: &[String],
    available_auths: &[String],
) -> UiPicker {
    model_picker_for_current(
        "select title model",
        "type fuzzy search, ctrl-p pin/unpin, tab complete, up/down select, enter confirm, esc cancel",
        CurrentModel {
            provider: current_provider,
            model: current_model,
            badge: "selected",
        },
        favorite_models,
        available_auths,
        PickerAction::SelectTitleModel,
    )
}

struct CurrentModel<'a> {
    provider: &'a str,
    model: &'a str,
    badge: &'a str,
}

fn model_picker_for_current(
    title: &str,
    help: &str,
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
    let current = format!("{current_provider}/{current_model}");
    let favorites = favorites::normalized_favorite_models(favorite_models);
    let items = favorites::reorder_models_by_favorites(
        catalog::available_models_for_auths(available_auths),
        &favorites,
    )
    .into_iter()
    .map(|entry| {
        let value = format!("{}/{}", entry.provider, entry.model);
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
            label: value.clone(),
            detail: Some(if pinned {
                "Press Ctrl-P to unpin this model.".into()
            } else {
                "Press Ctrl-P to pin this model to the top of model pickers.".into()
            }),
            preview: None,
            badge,
            value,
        }
    })
    .collect::<Vec<_>>();

    let mut picker = UiPicker::new(title, help, items, action);
    if let Some(index) = picker.items.iter().position(|item| item.value == current) {
        picker.selected = index;
    }
    picker
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config_repository::ConfigRepository;
    use rho_providers::credentials::{
        available_auth_modes, save_codex_tokens, MemoryCredentialStore,
    };

    #[test]
    fn model_picker_orders_pinned_models_before_selected_model() {
        let store = MemoryCredentialStore::default();
        save_codex_tokens(
            &store,
            &rho_providers::credentials::CodexTokens {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                id_token: None,
                account_id: None,
            },
        )
        .unwrap();
        let auths = available_auth_modes(&store);
        let info = TuiInfo {
            cwd: std::path::PathBuf::from("/tmp/project"),
            provider: "openai-codex".into(),
            model: "gpt-5.6-sol".into(),
            reasoning: rho_providers::reasoning::ReasoningLevel::Low,
            reasoning_source: rho_providers::model::ReasoningRequestSource::PersistedOrDefault,
            permission_mode: crate::permission::PermissionMode::Auto,
            show_reasoning_output: true,
            auth: "codex".into(),
            title_provider: None,
            title_model: None,
            title_auth: None,
            favorite_models: vec!["openai-codex/gpt-5.4-mini".into()],
            session_id: None,
            recovered_messages: Vec::new(),
            open_resume_picker: false,
            config_repository: ConfigRepository::new(None),
            auth_unavailable: None,
            update_notice: None,
            pending_update_notice: None,
            diagnostics: crate::diagnostics::test_diagnostics("openai", "gpt-test"),
            herdr: crate::herdr::HerdrReporter::default(),
            max_tool_output_lines: 10,
            keybindings: crate::keybindings::Keybindings::default(),
            prompt_templates: Default::default(),
        };

        let picker = model_picker(&info, &auths);

        assert_eq!(picker.items[0].value, "openai-codex/gpt-5.4-mini");
        assert_eq!(
            picker.selected_item().unwrap().value,
            "openai-codex/gpt-5.6-sol"
        );
    }

    #[test]
    fn running_model_picker_marks_pending_model_and_explains_timing() {
        let store = MemoryCredentialStore::default();
        save_codex_tokens(
            &store,
            &rho_providers::credentials::CodexTokens {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                id_token: None,
                account_id: None,
            },
        )
        .unwrap();
        let auths = available_auth_modes(&store);
        let info = TuiInfo {
            cwd: std::path::PathBuf::from("/tmp/project"),
            provider: "openai-codex".into(),
            model: "gpt-5.5".into(),
            reasoning: rho_providers::reasoning::ReasoningLevel::Low,
            reasoning_source: rho_providers::model::ReasoningRequestSource::PersistedOrDefault,
            permission_mode: crate::permission::PermissionMode::Auto,
            show_reasoning_output: true,
            auth: "codex".into(),
            title_provider: None,
            title_model: None,
            title_auth: None,
            favorite_models: Vec::new(),
            session_id: None,
            recovered_messages: Vec::new(),
            open_resume_picker: false,
            config_repository: ConfigRepository::new(None),
            auth_unavailable: None,
            update_notice: None,
            pending_update_notice: None,
            diagnostics: crate::diagnostics::test_diagnostics("openai", "gpt-test"),
            herdr: crate::herdr::HerdrReporter::default(),
            max_tool_output_lines: 10,
            keybindings: crate::keybindings::Keybindings::default(),
            prompt_templates: Default::default(),
        };
        let pending = rho_providers::model::catalog::ModelSelection {
            provider: "openai-codex".into(),
            model: "gpt-5.4-mini".into(),
            auth: "codex".into(),
            from_catalog: true,
        };

        let picker = model_picker_during_run(&info, Some(&pending), &auths);

        assert_eq!(picker.title, "select model for next turn");
        assert!(picker.help.contains("after it fully ends"));
        let selected = picker.selected_item().unwrap();
        assert_eq!(selected.value, "openai-codex/gpt-5.4-mini");
        assert_eq!(selected.badge.as_ref().unwrap().text, "pending");
    }
}
