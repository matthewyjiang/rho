use super::*;

fn entry(provider: &str, model: &str) -> ModelCatalogEntry {
    ModelCatalogEntry {
        provider: provider.into(),
        model: model.into(),
        display_name: model.into(),
        auth_modes: vec!["auth".into()],
    }
}

#[test]
fn poolside_favorites_normalize_to_internal_model_and_match_legacy_forms() {
    let favorites = normalized_favorite_models(&[
        "poolside/poolside/laguna-m.1".into(),
        "poolside/laguna-m.1".into(),
    ]);

    assert_eq!(favorites.len(), 1);
    assert_eq!(favorites[0].model, "laguna-m.1");
    assert_eq!(favorites[0].value(), "poolside/laguna-m.1");
    assert!(favorites[0].matches("poolside", "laguna-m.1"));
    assert!(favorites[0].matches("poolside", "poolside/laguna-m.1"));
    assert!(favorites[0].matches("poolside", "poolside/poolside/laguna-m.1"));
}

#[test]
fn legacy_provider_aliases_normalize_favorites() {
    let favorites = normalized_favorite_models(&[
        "openrouter-oauth/anthropic/claude-sonnet-4".into(),
        "openrouter/anthropic/claude-sonnet-4".into(),
        "xai-oauth/grok-4.5".into(),
    ]);

    assert_eq!(
        favorite_model_values(&favorites),
        vec!["openrouter/anthropic/claude-sonnet-4", "xai/grok-4.5",]
    );
    assert!(favorites[0].matches("openrouter-oauth", "anthropic/claude-sonnet-4"));
    assert!(favorites[1].matches("xai-oauth", "grok-4.5"));
}

#[test]
fn reorders_available_models_by_favorites() {
    let models = vec![
        entry("anthropic", "claude"),
        entry("openai", "gpt-5.5"),
        entry("github-copilot", "gpt-4.1"),
    ];
    let favorites = normalized_favorite_models(&[
        "openai/gpt-5.5".into(),
        "unavailable/model".into(),
        "anthropic/claude".into(),
    ]);

    let ordered = reorder_models_by_favorites(models, &favorites);

    assert_eq!(
        ordered
            .iter()
            .map(|entry| format!("{}/{}", entry.provider, entry.model))
            .collect::<Vec<_>>(),
        vec![
            "openai/gpt-5.5",
            "anthropic/claude",
            "github-copilot/gpt-4.1",
        ]
    );
}

// Covers: composer cycle must walk only pins that currently have auth, wrap,
// and stay put when the only usable pin is already selected.
// Owner: pure unit (favorite cycle policy)
#[test]
fn cycles_usable_favorites_in_pin_order() {
    let models = vec![
        entry("anthropic", "claude"),
        entry("openai", "gpt-5.5"),
        entry("github-copilot", "gpt-4.1"),
    ];
    let favorites = normalized_favorite_models(&[
        "openai/gpt-5.5".into(),
        "unavailable/model".into(),
        "anthropic/claude".into(),
    ]);

    let cases = [
        (
            "openai",
            "gpt-5.5",
            CycleDirection::Forward,
            Some("anthropic/claude"),
        ),
        (
            "anthropic",
            "claude",
            CycleDirection::Forward,
            Some("openai/gpt-5.5"),
        ),
        (
            "openai",
            "gpt-5.5",
            CycleDirection::Backward,
            Some("anthropic/claude"),
        ),
        (
            "github-copilot",
            "gpt-4.1",
            CycleDirection::Forward,
            Some("openai/gpt-5.5"),
        ),
        (
            "github-copilot",
            "gpt-4.1",
            CycleDirection::Backward,
            Some("anthropic/claude"),
        ),
    ];
    for (provider, model, direction, expected) in cases {
        let switched = match cycle_favorite(&favorites, &models, provider, model, direction) {
            CycleOutcome::Switch(favorite) => Some(favorite.value()),
            other => panic!("{provider}/{model} {direction:?} expected a switch, got {other:?}"),
        };
        assert_eq!(
            switched,
            expected.map(str::to_string),
            "{provider}/{model} {direction:?}"
        );
    }
}

// Covers: the two "nothing happened" cases need different UI, so the outcome
// must distinguish an empty pin list from an already-current single pin.
// Owner: pure unit (favorite cycle policy)
#[test]
fn cycle_reports_why_it_did_not_move() {
    let models = vec![entry("openai", "gpt-5.5")];
    let cases = [
        (vec![], CycleOutcome::NoPins),
        (vec!["unavailable/model".to_string()], CycleOutcome::NoPins),
        (vec!["openai/gpt-5.5".to_string()], CycleOutcome::Unchanged),
    ];
    for (favorites, expected) in cases {
        assert_eq!(
            cycle_favorite(
                &normalized_favorite_models(&favorites),
                &models,
                "openai",
                "gpt-5.5",
                CycleDirection::Forward
            ),
            expected,
            "{favorites:?}"
        );
    }
}

#[test]
fn toggles_favorites() {
    let mut favorites = vec!["openai/gpt-5.5".into()];

    assert!(!toggle_favorite(&mut favorites, "openai", "gpt-5.5"));
    assert!(favorites.is_empty());

    assert!(toggle_favorite(&mut favorites, "anthropic", "claude"));
    assert_eq!(favorites, vec!["anthropic/claude"]);
}
