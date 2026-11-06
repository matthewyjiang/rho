use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ServiceTier},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    ReasoningLevel, UserInput,
};

use super::SettingChangeError;
use crate::{
    app::session_runtime::test_support::{runtime_on, HangUntilCancel},
    config::Config,
};

fn config(provider: &str, model: &str, auth: &str) -> Config {
    Config {
        provider: provider.into(),
        model: model.into(),
        auth: auth.into(),
        ..Config::default()
    }
}

/// A provider on `provider`/`model` that answers one request.
fn answers_once(provider: &str, model: &str) -> ScriptedProvider {
    ScriptedProvider::new(
        ModelIdentity::new(provider, "scripted", model),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    )
}

// Covers: a host-selected reasoning level must apply to the next idle turn.
// Owner: session runtime settings
#[tokio::test]
async fn reasoning_level_applies_to_the_next_turn() {
    let root = tempfile::tempdir().unwrap();
    let config = config("test", "model", "api-key");
    let provider = answers_once("test", "model");
    let mut runtime = runtime_on(root.path(), &config, Arc::new(provider.clone())).await;
    runtime
        .built
        .session
        .set_reasoning_level(ReasoningLevel::Medium)
        .unwrap();

    runtime
        .set_reasoning_level(ReasoningLevel::High, &config)
        .expect("apply reasoning level");

    runtime.built.session.complete("next").await.unwrap();
    assert_eq!(
        (
            runtime.reasoning_level(),
            provider.recorded_requests()[0].reasoning_level
        ),
        (ReasoningLevel::High, ReasoningLevel::High)
    );
}

// Covers: changing the reasoning level while a turn runs must fail as busy
// instead of racing the turn.
// Owner: session runtime settings
#[tokio::test]
async fn reasoning_level_change_during_a_turn_is_busy() {
    let root = tempfile::tempdir().unwrap();
    let config = config("test", "model", "api-key");
    let mut runtime = runtime_on(root.path(), &config, Arc::new(HangUntilCancel)).await;
    runtime
        .built
        .session
        .set_reasoning_level(ReasoningLevel::Medium)
        .unwrap();
    let mut run = runtime
        .built
        .session
        .start(UserInput::text("hold"))
        .await
        .unwrap();

    let error = runtime
        .set_reasoning_level(ReasoningLevel::High, &config)
        .expect_err("busy session");
    run.cancel();
    let _ = run.outcome().await;

    assert!(
        matches!(error, SettingChangeError::Busy),
        "unexpected error: {error:?}"
    );
}

// Covers: a level the current model does not support is refused with the
// levels it does, never normalized to one of them.
// Owner: session runtime settings
#[tokio::test]
async fn unsupported_reasoning_level_names_the_supported_ones() {
    let root = tempfile::tempdir().unwrap();
    let config = config("poolside", "model", "api-key");
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("poolside", "poolside", "model"),
        [],
    ));
    let mut runtime = runtime_on(root.path(), &config, provider).await;
    runtime
        .built
        .session
        .set_reasoning_level(ReasoningLevel::Off)
        .unwrap();

    let error = runtime
        .set_reasoning_level(ReasoningLevel::High, &config)
        .expect_err("unsupported level");

    let message = match error {
        SettingChangeError::Unsupported(message) => message,
        error => panic!("unexpected error: {error:?}"),
    };
    assert_eq!(
        (message.as_str(), runtime.reasoning_level()),
        (
            "provider 'poolside' model 'model' does not support reasoning level 'high'; supported levels: off, max",
            ReasoningLevel::Off,
        )
    );
}

// Covers: a fast mode toggle must reach the next model request's service
// tier; enabling it on a model without fast mode is refused (even when a
// tier carried across a model switch already matches), while disabling that
// carried tier always succeeds.
// Owner: session runtime settings
#[tokio::test]
async fn fast_mode_sets_the_next_request_service_tier() {
    let codex = ("openai-codex", "gpt-5.5", "codex");
    let plain = ("test", "model", "api-key");
    let priority = Some(ServiceTier::Priority);
    let cases = [
        (codex, None, true, Ok(priority)),
        (codex, priority, false, Ok(None)),
        (plain, None, true, Err("unsupported")),
        (plain, priority, true, Err("unsupported")),
        (plain, priority, false, Ok(None)),
    ];
    for ((provider_id, model, auth), start, enabled, expected) in cases {
        let root = tempfile::tempdir().unwrap();
        let config = config(provider_id, model, auth);
        let provider = answers_once(provider_id, model);
        let mut runtime = runtime_on(root.path(), &config, Arc::new(provider.clone())).await;
        runtime.built.session.set_service_tier(start).unwrap();

        let result = runtime.set_fast_mode(enabled).map_err(|error| match error {
            SettingChangeError::Unsupported(_) => "unsupported",
            SettingChangeError::Busy => "busy",
            SettingChangeError::Apply(_) => "apply",
        });

        let label = format!("{provider_id}/{model} enabled={enabled}");
        match expected {
            Ok(tier) => {
                assert_eq!(result, Ok(()), "{label}");
                runtime.built.session.complete("next").await.unwrap();
                assert_eq!(
                    provider.recorded_requests()[0].service_tier,
                    tier,
                    "{label}"
                );
            }
            Err(kind) => {
                assert_eq!(result, Err(kind), "{label}");
                assert_eq!(runtime.built.session.service_tier(), start, "{label}");
            }
        }
    }
}
