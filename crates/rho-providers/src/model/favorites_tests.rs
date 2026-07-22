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
fn normalizes_favorites() {
    let favorites = vec![
        " openai/gpt-5.5 ".into(),
        "missing-separator".into(),
        "openai/gpt-5.5".into(),
        "anthropic/claude".into(),
    ];

    assert_eq!(
        normalized_favorite_models(&favorites),
        vec![
            FavoriteModel::new("openai", "gpt-5.5"),
            FavoriteModel::new("anthropic", "claude"),
        ]
    );
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

#[test]
fn toggles_favorites() {
    let mut favorites = vec!["openai/gpt-5.5".into()];

    assert!(!toggle_favorite(&mut favorites, "openai", "gpt-5.5"));
    assert!(favorites.is_empty());

    assert!(toggle_favorite(&mut favorites, "anthropic", "claude"));
    assert_eq!(favorites, vec!["anthropic/claude"]);
}
