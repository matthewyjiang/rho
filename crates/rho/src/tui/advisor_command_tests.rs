use pretty_assertions::assert_eq;
use rho_providers::model::{
    provider_models::{
        replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
        ProviderModel,
    },
    ReasoningCapabilities,
};

use super::*;
use crate::{commands::parse_command, config::InternalAgentModelConfig, tui::tests::test_app};

fn invocation(command: &str) -> CommandInvocation {
    parse_command(command).unwrap().unwrap()
}

/// Runs `f` with a temporary OpenAI model cache so the advisor model picker can open.
///
/// OpenAI is cache-backed. Without this, `/advisor on` with no model only
/// reports "no cached provider models" and never opens a picker. The cache
/// override is thread-local, so the body stays on one thread.
fn with_cached_openai_models<T>(f: impl FnOnce() -> T) -> T {
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        replace_cached_provider_models_for_tests(
            "openai",
            &[ProviderModel {
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                display_name: "GPT-5.5".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();
        f()
    })
}

/// Records what the command asked the runtime to apply, so the advisor state
/// transition is observable without a live SDK runtime.
#[derive(Default)]
struct FakeAdvisorRuntime {
    applied: Vec<Option<InternalAgentModelConfig>>,
}

impl FakeAdvisorRuntime {
    fn last_applied(&self) -> Option<&Option<InternalAgentModelConfig>> {
        self.applied.last()
    }
}

impl AdvisorRuntime for FakeAdvisorRuntime {
    fn set_advisor(
        &mut self,
        model: Option<InternalAgentModelConfig>,
    ) -> impl std::future::Future<Output = anyhow::Result<Option<String>>> + Send {
        self.applied.push(model);
        std::future::ready(Ok(None))
    }

    fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        Vec::new()
    }
}

fn app_with_advisor_model() -> App {
    let mut app = test_app();
    app.info.runtime.internal_agents.insert(
        ADVISOR_AGENT_ID.into(),
        InternalAgentModelConfig::new("openai".into(), "gpt-5.5".into(), "api-key".into()),
    );
    app
}

// Covers: /advisor on with a configured advisor model turns the mode on and saves it
// Owner: advisor command
#[tokio::test]
async fn enabling_advisor_mode_with_a_model_saves_the_setting() {
    let mut app = app_with_advisor_model();
    let mut agent = FakeAdvisorRuntime::default();

    app.execute_advisor_command_with_runtime(invocation("/advisor on"), &mut agent)
        .await
        .unwrap();

    assert!(app.info.runtime.advisor_mode);
    assert!(
        app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .advisor_mode
    );
    assert_eq!(
        app.status(),
        "advisor mode is on: openai/gpt-5.5 reviews the session"
    );
    assert_eq!(
        agent.last_applied(),
        Some(&Some(InternalAgentModelConfig::new(
            "openai".into(),
            "gpt-5.5".into(),
            "api-key".into()
        )))
    );
}

// Covers: /advisor with no argument flips the current mode
// Owner: advisor command
#[tokio::test]
async fn bare_advisor_command_turns_the_mode_off_again() {
    let mut app = app_with_advisor_model();
    let mut agent = FakeAdvisorRuntime::default();
    app.execute_advisor_command_with_runtime(invocation("/advisor on"), &mut agent)
        .await
        .unwrap();

    app.execute_advisor_command_with_runtime(invocation("/advisor"), &mut agent)
        .await
        .unwrap();

    assert!(!app.info.runtime.advisor_mode);
    assert!(
        !app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .advisor_mode
    );
    assert_eq!(app.status(), "advisor mode is off");
    assert_eq!(agent.last_applied(), Some(&None));
}

// Covers: /advisor off saves the mode off, and turning it on again keeps the model
// Owner: advisor command
#[tokio::test]
async fn disabling_advisor_mode_saves_the_setting() {
    let mut app = app_with_advisor_model();
    let mut agent = FakeAdvisorRuntime::default();
    app.execute_advisor_command_with_runtime(invocation("/advisor on"), &mut agent)
        .await
        .unwrap();

    app.execute_advisor_command_with_runtime(invocation("/advisor off"), &mut agent)
        .await
        .unwrap();

    assert!(!app.info.runtime.advisor_mode);
    assert!(
        !app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .advisor_mode
    );

    // Turning the mode back on must reuse the model chosen before, not ask again.
    app.execute_advisor_command_with_runtime(invocation("/advisor on"), &mut agent)
        .await
        .unwrap();

    assert!(app.info.runtime.advisor_mode);
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert_eq!(
        agent.applied,
        vec![
            Some(InternalAgentModelConfig::new(
                "openai".into(),
                "gpt-5.5".into(),
                "api-key".into()
            )),
            None,
            Some(InternalAgentModelConfig::new(
                "openai".into(),
                "gpt-5.5".into(),
                "api-key".into()
            )),
        ]
    );
}

// Covers: an active-run runtime error must not flip saved or in-memory mode.
// Owner: advisor command
#[tokio::test]
async fn advisor_mode_runtime_failure_leaves_mode_unchanged() {
    #[derive(Debug)]
    struct ActiveRunError;

    impl std::fmt::Display for ActiveRunError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("advisor mode cannot change while a run is active")
        }
    }

    impl std::error::Error for ActiveRunError {}

    struct ActiveRunRuntime;

    impl AdvisorRuntime for ActiveRunRuntime {
        fn set_advisor(
            &mut self,
            _model: Option<InternalAgentModelConfig>,
        ) -> impl std::future::Future<Output = anyhow::Result<Option<String>>> + Send {
            std::future::ready(Err(anyhow::Error::new(ActiveRunError)))
        }

        fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
            Vec::new()
        }
    }

    let mut app = app_with_advisor_model();
    let mut agent = ActiveRunRuntime;
    let error = app
        .execute_advisor_command_with_runtime(invocation("/advisor on"), &mut agent)
        .await
        .expect_err("active-run failure should propagate");
    assert!(
        error.downcast_ref::<ActiveRunError>().is_some(),
        "expected typed ActiveRunError, got: {error:#}"
    );
    assert!(!app.info.runtime.advisor_mode);
    assert!(
        !app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .advisor_mode
    );
    assert_eq!(app.status(), "advisor mode change failed");
}

// Covers: editing the advisor model from /config does not claim it enables mode.
// Owner: advisor command
#[test]
fn advisor_model_config_row_uses_edit_status() {
    with_cached_openai_models(|| {
        let mut app = app_with_advisor_model();
        app.open_main_config_picker_selected(super::super::config_picker::ADVISOR_MODEL_VALUE)
            .unwrap();
        app.open_advisor_model_prompt(InternalAgentModelPickerOrigin::AdvisorModelConfigRow);
        assert_eq!(app.status(), "select an advisor model");
        assert_eq!(
            app.internal_agent_model_target
                .as_ref()
                .map(|target| target.origin),
            Some(InternalAgentModelPickerOrigin::AdvisorModelConfigRow)
        );
    });
}
