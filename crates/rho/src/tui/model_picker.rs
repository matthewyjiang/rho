use super::{
    PickerAction, PickerBadge, PickerBadgeTone, PickerItem, PickerKeyHints, RuntimeModelView,
    UiPicker,
};
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

pub(super) fn internal_agent_model_picker(
    agent_id: &str,
    current_provider: &str,
    current_model: &str,
    uses_conversation_model: bool,
    favorite_models: &[String],
    available_auths: &[String],
) -> UiPicker {
    let mut picker = model_picker_for_current(
        &format!("select model for {agent_id}"),
        CurrentModel {
            provider: current_provider,
            model: current_model,
            badge: "selected",
        },
        favorite_models,
        available_auths,
        PickerAction::SelectInternalAgentModel,
    );
    let selected_model = picker.items.iter().position(|item| {
        item.value == rho_providers::provider::model_reference(current_provider, current_model)
    });
    picker.items.insert(
        0,
        PickerItem {
            section: None,
            label: "Use conversation model".into(),
            detail: Some("Follow the active conversation provider, model, and auth.".into()),
            preview: None,
            badge: uses_conversation_model.then_some(PickerBadge {
                text: "selected".into(),
                tone: PickerBadgeTone::Selected,
            }),
            value: USE_CONVERSATION_MODEL.into(),
            selection_verb: None,
        },
    );
    picker.selected = if uses_conversation_model {
        0
    } else {
        selected_model.map_or(0, |index| index + 1)
    };
    picker
}

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
