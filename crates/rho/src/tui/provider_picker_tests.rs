use rho_providers::model::catalog;

use super::{
    login_group_next, login_group_picker, login_method_picker, refresh_model_list_picker,
    LoginGroupNext, ALL_REFRESHABLE_PROVIDERS,
};

#[test]
fn login_picker_lists_poolside() {
    let picker = login_group_picker();
    let poolside = picker
        .items
        .iter()
        .find(|item| item.value == "poolside")
        .expect("Poolside should be available for login");

    assert_eq!(poolside.label, "Poolside");
}

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
        vec![ALL_REFRESHABLE_PROVIDERS, "anthropic", "openai"]
    );
}

#[test]
fn login_picker_lists_providers_alphabetically_by_label() {
    let picker = login_group_picker();
    let labels = picker
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels
        .windows(2)
        .all(|pair| { pair[0].to_ascii_lowercase() <= pair[1].to_ascii_lowercase() }));
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

#[test]
fn login_group_picker_keeps_claude_code_out_of_top_level() {
    let picker = login_group_picker();
    assert!(
        picker.items.iter().all(|item| item.value != "claude-code"),
        "claude-code belongs under Anthropic methods, not as its own group: {:?}",
        picker
            .items
            .iter()
            .map(|item| item.value.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        picker.items.iter().any(|item| item.value == "anthropic"),
        "Anthropic group should remain available"
    );
    assert!(
        picker.items.iter().all(|item| {
            let label = item.label.to_ascii_lowercase();
            !label.contains("external runtime")
                && item.section.as_deref() != Some("External runtimes")
        }),
        "login groups must not expose a standalone external-runtimes section: {:?}",
        picker
            .items
            .iter()
            .map(|item| (item.section.as_deref(), item.label.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn anthropic_login_methods_include_claude_code_for_delegation() {
    let picker = login_method_picker(catalog::login_group("anthropic").expect("anthropic group"));
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(values, vec!["anthropic", "claude-code"]);

    let api_key = &picker.items[0];
    assert_eq!(api_key.label, "API Key");
    assert_eq!(api_key.value, "anthropic");

    let claude = &picker.items[1];
    assert_eq!(claude.label, "Claude Code (delegation only)");
    assert_eq!(claude.value, "claude-code");
    let detail = claude
        .detail
        .as_deref()
        .expect("claude code method should explain ownership");
    assert!(
        detail.contains("External Claude binary") || detail.contains("subscription"),
        "detail should mark the external/subscription runtime: {detail}"
    );
    assert!(
        detail.contains("not Anthropic API billing"),
        "detail should distinguish Claude Code from Anthropic API billing: {detail}"
    );
    assert!(
        detail.contains("managed by Claude Code") && detail.contains("not Rho"),
        "detail should say Claude Code owns credentials, not Rho: {detail}"
    );
}

#[test]
fn login_group_next_opens_anthropic_methods_including_claude_code() {
    match login_group_next(catalog::login_group("anthropic").expect("anthropic group")) {
        LoginGroupNext::MethodPicker(picker) => {
            assert_eq!(picker.title, "select Anthropic login method");
            let values = picker
                .items
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>();
            assert_eq!(values, vec!["anthropic", "claude-code"]);
        }
        LoginGroupNext::Provider(provider) => {
            panic!("Anthropic should open a method picker, got direct provider {provider}")
        }
    }
}

#[test]
fn login_group_next_keeps_single_catalog_method_direct() {
    match login_group_next(catalog::login_group("poolside").expect("poolside group")) {
        LoginGroupNext::Provider(provider) => assert_eq!(provider, "poolside"),
        LoginGroupNext::MethodPicker(picker) => panic!(
            "single-method groups should stay direct, got {:?}",
            picker
                .items
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>()
        ),
    }
}
