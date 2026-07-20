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
fn refresh_picker_always_offers_all_configured_providers() {
    let picker = refresh_model_list_picker(&[]);

    assert_eq!(picker.items.len(), 1);
    assert_eq!(picker.items[0].value, ALL_REFRESHABLE_PROVIDERS);
}
