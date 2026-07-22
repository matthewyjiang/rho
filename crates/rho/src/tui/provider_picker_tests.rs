use super::{refresh_model_list_picker, ALL_REFRESHABLE_PROVIDERS};

#[test]
fn refresh_picker_lists_all_and_available_refreshable_providers() {
    let picker = refresh_model_list_picker(&[
        "api-key".into(),
        "anthropic-api-key".into(),
        "xai-api-key".into(),
    ]);
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        values,
        vec![ALL_REFRESHABLE_PROVIDERS, "openai", "anthropic"]
    );
}

#[test]
fn refresh_picker_distinguishes_openrouter_auth_modes() {
    let picker =
        refresh_model_list_picker(&["openrouter-api-key".into(), "openrouter-oauth".into()]);
    let openrouter = picker
        .items
        .iter()
        .filter(|item| item.label == "OpenRouter")
        .map(|item| (item.value.as_str(), item.detail.as_deref()))
        .collect::<Vec<_>>();

    assert_eq!(
        openrouter,
        vec![
            (
                "openrouter",
                Some("Refresh cached OpenRouter models with OpenRouter API key."),
            ),
            (
                "openrouter-oauth",
                Some("Refresh cached OpenRouter models with OpenRouter OAuth."),
            ),
        ]
    );
}

#[test]
fn refresh_picker_always_offers_all_configured_providers() {
    let picker = refresh_model_list_picker(&[]);

    assert_eq!(picker.items.len(), 1);
    assert_eq!(picker.items[0].value, ALL_REFRESHABLE_PROVIDERS);
}
