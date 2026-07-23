use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_providers::model::catalog::ModelSelection;

use super::super::InteractiveModelSelection;
use crate::{model_aliases::ModelAliases, tui::tests::test_app};

fn aliases(entries: &[(&str, &str)]) -> ModelAliases {
    ModelAliases::from_entries(
        entries
            .iter()
            .map(|(name, target)| (name.to_string(), target.to_string()))
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap()
}

#[test]
fn resolves_alias_before_interactive_model_lookup() {
    let mut app = test_app();
    app.info.runtime.model_aliases = aliases(&[("deep", "openai-codex/gpt-5.5")]);

    let resolved = app
        .resolve_model_selection("@deep", &app.info.runtime.provider, &app.info.runtime.auth)
        .unwrap();

    assert_eq!(
        resolved,
        InteractiveModelSelection {
            selection: ModelSelection {
                provider: "openai-codex".into(),
                model: "gpt-5.5".into(),
                auth: "codex".into(),
                from_catalog: true,
            },
            alias: Some("deep".into()),
        }
    );
}

#[test]
fn bare_alias_keeps_current_provider() {
    let mut app = test_app();
    app.info.runtime.model_aliases = aliases(&[("fast", "gpt-5.5")]);

    let resolved = app
        .resolve_model_selection("@fast", "openai-codex", "codex")
        .unwrap();

    assert_eq!(
        resolved,
        InteractiveModelSelection {
            selection: ModelSelection {
                provider: "openai-codex".into(),
                model: "gpt-5.5".into(),
                auth: "codex".into(),
                from_catalog: true,
            },
            alias: Some("fast".into()),
        }
    );
}

#[test]
fn reports_undefined_alias_in_interactive_model_lookup() {
    let app = test_app();

    let error = app
        .resolve_model_selection(
            "@missing",
            &app.info.runtime.provider,
            &app.info.runtime.auth,
        )
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "model alias '@missing' is not defined; define it in [model.aliases] or use a concrete model reference"
    );
}

fn target(provider: &str, model: &str, auth: &str) -> ModelSelection {
    ModelSelection {
        provider: provider.into(),
        model: model.into(),
        auth: auth.into(),
        from_catalog: true,
    }
}

#[test]
fn handoff_requires_warm_cache_and_compactable_history() {
    let current = crate::tui::tests::test_bootstrap().runtime;
    let target = target("other-provider", "other-model", "other-auth");

    assert!(!super::should_offer_model_handoff(
        &current, &target, false, true
    ));
    assert!(!super::should_offer_model_handoff(
        &current, &target, true, false
    ));
    assert!(super::should_offer_model_handoff(
        &current, &target, true, true
    ));
}

#[test]
fn handoff_is_not_offered_for_current_selection() {
    let current = crate::tui::tests::test_bootstrap().runtime;
    let target = target(&current.provider, &current.model, &current.auth);

    assert!(!super::should_offer_model_handoff(
        &current, &target, true, true
    ));
}
