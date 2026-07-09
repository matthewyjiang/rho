use super::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, TuiInfo, UiPicker};
use crate::model::{catalog, favorites};

pub(super) fn model_picker(info: &TuiInfo, available_auths: &[String]) -> UiPicker {
    model_picker_for_current(
        "select model",
        &info.provider,
        &info.model,
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
        current_provider,
        current_model,
        favorite_models,
        available_auths,
        PickerAction::SelectTitleModel,
    )
}

fn model_picker_for_current(
    title: &str,
    current_provider: &str,
    current_model: &str,
    favorite_models: &[String],
    available_auths: &[String],
    action: PickerAction,
) -> UiPicker {
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
                text: "pinned, selected".into(),
                tone: PickerBadgeTone::Selected,
            }),
            (true, false) => Some(PickerBadge {
                text: "pinned".into(),
                tone: PickerBadgeTone::Favorite,
            }),
            (false, true) => Some(PickerBadge {
                text: "selected".into(),
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

    let mut picker = UiPicker::new(
        title,
        "type regex filter, ctrl-p pin/unpin, tab complete, up/down select, enter confirm, esc cancel",
        items,
        action,
    );
    if let Some(index) = picker.items.iter().position(|item| item.value == current) {
        picker.selected = index;
    }
    picker
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::available_auth_modes;
    use crate::credentials::{save_codex_tokens, MemoryCredentialStore};

    #[test]
    fn model_picker_orders_pinned_models_before_selected_model() {
        let store = MemoryCredentialStore::default();
        save_codex_tokens(
            &store,
            &crate::credentials::CodexTokens {
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
            reasoning: crate::reasoning::ReasoningLevel::Low,
            show_reasoning_output: true,
            auth: "codex".into(),
            title_provider: None,
            title_model: None,
            title_auth: None,
            favorite_models: vec!["openai-codex/gpt-5.4-mini".into()],
            questionnaire_enabled: true,
            session_id: None,
            recovered_messages: Vec::new(),
            open_resume_picker: false,
            config_path: None,
            auth_unavailable: None,
            update_notice: None,
            herdr: crate::herdr::HerdrReporter::default(),
            max_tool_output_lines: 10,
        };

        let picker = model_picker(&info, &auths);

        assert_eq!(picker.items[0].value, "openai-codex/gpt-5.4-mini");
        assert_eq!(
            picker.selected_item().unwrap().value,
            "openai-codex/gpt-5.6-sol"
        );
    }
}
