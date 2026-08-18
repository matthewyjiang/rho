use super::*;
use crate::{model::ReasoningLevelSet, reasoning::ReasoningLevel};
use pretty_assertions::assert_eq;
use serde_json::json;

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

// Covers: null raw_json is incomplete (pre-capability write), while a successful
// fetch stores `{}` as known cache identity with Unknown picker levels so
// startup does not refresh forever and does not hide reasoning
// Owner: anthropic thinking protocol
#[test]
fn anthropic_empty_capabilities_object_is_known_and_null_is_not() {
    let cache_dir = unique_test_cache_dir("anthropic-missing-capabilities");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models(
            "anthropic",
            &[ProviderModel {
                provider: "anthropic".into(),
                model: "claude-opus-5".into(),
                display_name: "Claude Opus 5".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();

        assert!(
            provider_model_capabilities_need_refresh("anthropic", "claude-opus-5"),
            "null raw_json must stay incomplete"
        );

        write_cached_provider_model_raw_json_for_tests(
            "anthropic",
            "claude-opus-5",
            "Claude Opus 5",
            &json!({}),
        )
        .unwrap();

        assert!(
            !provider_model_capabilities_need_refresh("anthropic", "claude-opus-5"),
            "empty object from a successful fetch is known"
        );
        assert_eq!(
            cached_provider_model("anthropic", "claude-opus-5")
                .unwrap()
                .reasoning_capabilities,
            ReasoningCapabilities::Unknown
        );

        write_cached_provider_model_raw_json_for_tests(
            "anthropic",
            "claude-opus-5",
            "Claude Opus 5",
            &json!({
                "thinking": {"supported": true, "types": {"adaptive": {"supported": true}}}
            }),
        )
        .unwrap();

        assert!(!provider_model_capabilities_need_refresh(
            "anthropic",
            "claude-opus-5"
        ));
    });
    let _ = fs::remove_dir_all(cache_dir);
}

// Covers: dated snapshot ids resolve capabilities through the parent alias for
// freshness, picker levels, and wire mode so the three paths cannot drift
// Owner: anthropic thinking protocol
#[test]
fn anthropic_dated_snapshots_reuse_the_parent_alias_for_freshness_and_pickers() {
    let cache_dir = unique_test_cache_dir("anthropic-dated-snapshot-freshness");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        let caps = json!({
            "thinking": {
                "supported": true,
                "types": {"adaptive": {"supported": true}}
            },
            "effort": {
                "supported": true,
                "low": {"supported": true},
                "medium": {"supported": true},
                "high": {"supported": true},
                "max": {"supported": true}
            }
        });
        write_cached_provider_model_raw_json_for_tests(
            "anthropic",
            "claude-opus-5",
            "Claude Opus 5",
            &caps,
        )
        .unwrap();

        let parent = cached_provider_model("anthropic", "claude-opus-5").unwrap();
        let dated = cached_provider_model("anthropic", "claude-opus-5-20260724").unwrap();
        assert_eq!(dated.model, "claude-opus-5-20260724");
        assert_eq!(
            dated.reasoning_capabilities, parent.reasoning_capabilities,
            "dated snapshot must surface the same picker levels as the parent row"
        );
        assert!(matches!(
            dated.reasoning_capabilities,
            ReasoningCapabilities::Levels(_)
        ));

        assert!(!provider_model_capabilities_need_refresh(
            "anthropic",
            "claude-opus-5-20260724"
        ));
        assert!(provider_model_capabilities_need_refresh(
            "anthropic",
            "claude-unknown"
        ));

        let expected_mode = anthropic_thinking_mode_from_value("claude-opus-5", &caps).unwrap();
        assert_eq!(
            cached_anthropic_thinking_mode("claude-opus-5-20260724"),
            Some(expected_mode)
        );
    });
    let _ = fs::remove_dir_all(cache_dir);
}

// Covers: a successful Ollama refresh settles Unknown rows; only expiry retriggers
// Owner: ollama native discovery
#[test]
fn ollama_unknown_reasoning_rows_settle_after_a_fresh_refresh() {
    let cache_dir = unique_test_cache_dir("ollama-unknown-reasoning");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models(
            "ollama",
            &[ProviderModel {
                provider: "ollama".into(),
                model: "gemma4:31b".into(),
                display_name: "gemma4:31b".into(),
                context_window: Some(262_144),
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();
        assert!(
            !provider_model_capabilities_need_refresh("ollama", "gemma4:31b"),
            "fallback-sourced Unknown must not retrigger a successful refresh"
        );

        replace_cached_provider_models(
            "ollama",
            &[ProviderModel {
                provider: "ollama".into(),
                model: "gemma4:31b".into(),
                display_name: "gemma4:31b".into(),
                context_window: Some(262_144),
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                    vec![
                        ReasoningLevel::Off,
                        ReasoningLevel::Low,
                        ReasoningLevel::Medium,
                        ReasoningLevel::High,
                        ReasoningLevel::Max,
                    ],
                )),
            }],
        )
        .unwrap();
        assert!(!provider_model_capabilities_need_refresh(
            "ollama",
            "gemma4:31b"
        ));
    });
    let _ = fs::remove_dir_all(cache_dir);
}
