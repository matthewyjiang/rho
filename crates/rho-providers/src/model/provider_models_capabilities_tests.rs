use super::*;
use crate::{model::ReasoningLevelSet, reasoning::ReasoningLevel};
use pretty_assertions::assert_eq;

#[test]
fn old_kimi_cache_rows_are_incomplete_and_need_refresh() {
    let cache_dir = unique_test_cache_dir("reasoning-old-row");
    fs::create_dir_all(&cache_dir).unwrap();
    let connection = Connection::open(cache_dir.join("provider-models.sqlite3")).unwrap();
    connection
        .execute_batch(
            "create table provider_models (
                provider text not null,
                model text not null,
                display_name text not null,
                raw_json text,
                updated_at integer not null,
                primary key(provider, model)
            );
            create table provider_model_refresh (
                provider text primary key,
                updated_at integer not null,
                error text
            );
            insert into provider_models values ('kimi-code', 'k3', 'Kimi K3', null, 1);
            insert into provider_model_refresh values ('kimi-code', strftime('%s', 'now'), null);",
        )
        .unwrap();
    drop(connection);

    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        assert!(provider_model_capabilities_need_refresh("kimi-code", "k3"));
        assert_eq!(
            cached_provider_model("kimi-code", "k3")
                .unwrap()
                .reasoning_capabilities,
            ReasoningCapabilities::Unknown
        );
    });
    let _ = fs::remove_dir_all(cache_dir);
}

#[test]
fn provider_snapshot_expiration_applies_to_every_model_in_the_snapshot() {
    let cache_dir = unique_test_cache_dir("reasoning-expired-snapshot");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        let capabilities = ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
            ReasoningLevel::Off,
            ReasoningLevel::Low,
        ]));
        let models = ["k3", "k3-preview"].map(|model| ProviderModel {
            provider: "kimi-code".into(),
            model: model.into(),
            display_name: model.into(),
            context_window: None,
            max_output_tokens: None,
            reasoning_capabilities: capabilities.clone(),
        });
        replace_cached_provider_models("kimi-code", &models).unwrap();
        let connection = open_provider_models_cache().unwrap();
        connection
            .execute(
                "update provider_model_refresh set updated_at = 0 where provider = 'kimi-code'",
                [],
            )
            .unwrap();

        assert!(provider_model_capabilities_need_refresh("kimi-code", "k3"));
        assert!(provider_model_capabilities_need_refresh(
            "kimi-code",
            "k3-preview"
        ));
    });
    let _ = fs::remove_dir_all(cache_dir);
}

