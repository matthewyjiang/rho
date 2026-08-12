use super::*;
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    app::{config_repository::ConfigRepository, interactive_runtime::test_edit_tool_runtime},
    config::{EditTool, InternalAgentModelConfig},
    permission::PermissionMode,
    tui::{
        agent_picker::InternalAgentModelPickerOrigin, tests::test_app, ComposerMode, PickerAction,
    },
};
use pretty_assertions::assert_eq;
use rho_providers::model::{
    provider_models::{
        replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
        ProviderModel,
    },
    ReasoningCapabilities,
};

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

fn open_picker(app: &App) -> &crate::tui::UiPicker {
    match app.input_ui.composer() {
        ComposerMode::Picker(picker) => picker,
        composer => panic!("expected an open picker, found {composer:?}"),
    }
}

fn block_on<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

async fn open_permission_mode_picker(app: &mut App, agent: &mut InteractiveRuntime) {
    app.open_main_config_picker_selected(config_picker::PERMISSION_MODE_VALUE)
        .unwrap();
    app.submit_config_selection(config_picker::PERMISSION_MODE_VALUE, agent)
        .await
        .unwrap();
}

fn set_test_permission_mode(app: &mut App, mode: PermissionMode) {
    app.info.runtime.permission_mode = mode;
    app.info
        .services
        .config_repository
        .update(|config| config.permission_mode = mode)
        .unwrap();
}

fn classifier_model() -> InternalAgentModelConfig {
    InternalAgentModelConfig::new("openai".into(), "gpt-5.5".into(), "api-key".into())
}

fn set_test_classifier_model(app: &mut App) {
    let model = classifier_model();
    app.info
        .runtime
        .internal_agents
        .insert(PERMISSION_CLASSIFIER_AGENT_ID.into(), model.clone());
    app.info
        .services
        .config_repository
        .update(|config| {
            config.set_internal_agent_model_config(PERMISSION_CLASSIFIER_AGENT_ID, model);
        })
        .unwrap();
}

// Covers: a failed edit-tool config save must roll runtime back and leave the
// transcript describing the same forward+reverse transition sequence already
// written to model-visible and persisted display history.
// Owner: tui config edit-tool apply path
#[tokio::test]
async fn failed_edit_tool_save_keeps_rollback_histories_aligned() {
    let mut app = test_app();
    // Instance-scoped injection keeps load working and fails only this
    // repository's next durable write, including across Tokio worker hops.
    let repository = ConfigRepository::temporary_for_tests().unwrap();
    repository.fail_next_save_for_tests();
    app.info.services.config_repository = repository;

    let mut agent = test_edit_tool_runtime(EditTool::Pinned(rho_tools::EditFormat::Hashline)).await;
    assert!(agent.has_tool("edit"));
    assert!(!agent.has_tool("str_replace"));
    let history_before = agent.history().len();
    let diagnostics_before = app
        .info
        .services
        .diagnostics
        .response("config")
        .expect("diagnostics config");

    let result = app
        .apply_edit_tool(
            EditTool::Pinned(rho_tools::EditFormat::StrReplace),
            &mut agent,
        )
        .await;

    result.expect("save failure should stay Ok after successful runtime rollback");

    // Live runtime restored.
    assert!(agent.has_tool("edit"), "runtime must roll back to hashline");
    assert!(!agent.has_tool("str_replace"));

    // Model history recorded both transitions.
    let history = agent.history();
    assert_eq!(history.len(), history_before + 2);
    let notices: Vec<String> = history[history_before..]
        .iter()
        .map(|message| match message {
            rho_sdk::model::Message::User(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    rho_sdk::model::ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            other => panic!("expected user switch notice, got {other:?}"),
        })
        .collect();
    assert!(
        notices[0].contains("[edit tool switched]") && notices[0].contains("`str_replace`"),
        "forward model notice missing: {}",
        notices[0]
    );
    assert!(
        notices[1].contains("[edit tool switched]") && notices[1].contains("`edit`"),
        "rollback model notice missing: {}",
        notices[1]
    );

    // Transcript mirrors the same sequence, then the injected save error.
    // If save succeeded, status would be the success label and this error
    // entry would be missing — fail closed on the injection path.
    let entries = app.history.entries();
    assert_eq!(entries.len(), 3);
    assert!(matches!(
        &entries[0],
        Entry::Notice(text) if text == "edit tool switched to str_replace"
    ));
    assert!(matches!(
        &entries[1],
        Entry::Notice(text) if text == "edit tool switched to hashline"
    ));
    match &entries[2] {
        Entry::Error(text) => {
            assert!(
                text.contains("could not save edit tool setting"),
                "save-failure notice missing: {text}"
            );
            assert!(
                text.contains("injected config save failure"),
                "expected injected save failure path, got successful save UI: {text}"
            );
        }
        other => panic!("expected save-failure error entry, got {other:?}"),
    }
    assert_eq!(app.status(), "config save failed");

    // Preference diagnostics stay on the pre-save value.
    let diagnostics_after = app
        .info
        .services
        .diagnostics
        .response("config")
        .expect("diagnostics config");
    assert_eq!(diagnostics_before, diagnostics_after);

    agent.shutdown().await;
}

// Covers: selecting Auto without a classifier model must ask for one without
// committing Auto to memory or config.
// Owner: tui permission-mode config gate
#[test]
fn selecting_auto_without_classifier_model_opens_model_picker_without_saving_auto() {
    with_cached_openai_models(|| {
        let mut app = test_app();
        set_test_permission_mode(&mut app, PermissionMode::Bypass);
        let mut agent = block_on(test_edit_tool_runtime(EditTool::default()));
        block_on(agent.set_permission_mode(PermissionMode::Bypass)).unwrap();

        block_on(async {
            open_permission_mode_picker(&mut app, &mut agent).await;
            app.submit_config_selection(
                &format!(
                    "{}{}",
                    config_picker::PERMISSION_MODE_PREFIX,
                    PermissionMode::Auto.as_str()
                ),
                &mut agent,
            )
            .await
            .unwrap();
        });

        assert_eq!(app.info.runtime.permission_mode, PermissionMode::Bypass);
        assert_eq!(agent.permission_mode(), PermissionMode::Bypass);
        assert_eq!(
            app.info
                .services
                .config_repository
                .load()
                .unwrap()
                .permission_mode,
            PermissionMode::Bypass
        );
        let picker = open_picker(&app);
        assert_eq!(picker.action, PickerAction::SelectInternalAgentModel);
        assert_eq!(
            app.internal_agent_model_target
                .as_ref()
                .map(|target| (target.id.as_str(), target.origin)),
            Some((
                PERMISSION_CLASSIFIER_AGENT_ID,
                InternalAgentModelPickerOrigin::PermissionModeConfigRow,
            ))
        );
        assert_eq!(
            app.status(),
            "select a permission classifier model to turn Auto mode on"
        );

        block_on(agent.shutdown());
    });
}

// Covers: dismissing Auto's classifier model prompt restores the prior
// permission mode instead of persisting a half-enabled Auto mode.
// Owner: tui permission-mode config gate
#[test]
fn dismissing_auto_classifier_model_prompt_keeps_previous_permission_mode() {
    with_cached_openai_models(|| {
        let mut app = test_app();
        set_test_permission_mode(&mut app, PermissionMode::Bypass);
        let mut agent = block_on(test_edit_tool_runtime(EditTool::default()));
        block_on(agent.set_permission_mode(PermissionMode::Bypass)).unwrap();

        block_on(async {
            open_permission_mode_picker(&mut app, &mut agent).await;
            app.submit_config_selection(
                &format!(
                    "{}{}",
                    config_picker::PERMISSION_MODE_PREFIX,
                    PermissionMode::Auto.as_str()
                ),
                &mut agent,
            )
            .await
            .unwrap();
        });

        app.handle_picker_escape(/*running*/ false).unwrap();

        assert_eq!(app.info.runtime.permission_mode, PermissionMode::Bypass);
        assert_eq!(agent.permission_mode(), PermissionMode::Bypass);
        assert!(app.internal_agent_model_target.is_none());
        assert_eq!(open_picker(&app).title, "Permission mode");
        assert_eq!(
            app.status(),
            "permission mode stays bypass: no classifier model selected"
        );

        block_on(agent.shutdown());
    });
}

// Covers: choosing the classifier model after the Auto gate completes the
// permission mode transition and persists Auto.
// Owner: tui permission-mode config gate
#[test]
fn selecting_classifier_model_from_auto_gate_applies_auto_mode() {
    let mut app = test_app();
    set_test_permission_mode(&mut app, PermissionMode::Bypass);
    set_test_classifier_model(&mut app);
    let mut agent = block_on(test_edit_tool_runtime(EditTool::default()));
    block_on(agent.set_permission_mode(PermissionMode::Bypass)).unwrap();

    block_on(async {
        app.finish_permission_classifier_model_selection(/*selected*/ true, &mut agent)
            .await
            .unwrap();
    });

    assert_eq!(app.info.runtime.permission_mode, PermissionMode::Auto);
    assert_eq!(agent.permission_mode(), PermissionMode::Auto);
    assert_eq!(
        app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID)
            .map(InternalAgentModelConfig::display_reference),
        Some("openai/gpt-5.5".into())
    );
    assert_eq!(
        app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .permission_mode,
        PermissionMode::Auto
    );
    block_on(agent.shutdown());
}

// Covers: Agent behavior exposes the classifier model row, and exposes
// classifier reasoning only after a model exists.
// Owner: tui config picker rows
#[test]
fn agent_behavior_config_rows_include_classifier_model_and_optional_reasoning() {
    let mut app = test_app();
    let config = app.info.services.config_repository.load().unwrap();

    let picker = config_picker::category_picker(
        config_picker::AGENT_CATEGORY_VALUE,
        &app.info.runtime,
        &config,
    )
    .unwrap();
    assert!(picker
        .items
        .iter()
        .any(|item| item.value == config_picker::PERMISSION_CLASSIFIER_MODEL_VALUE));
    assert!(!picker
        .items
        .iter()
        .any(|item| item.value == config_picker::PERMISSION_CLASSIFIER_REASONING_VALUE));

    app.info
        .runtime
        .internal_agents
        .insert(PERMISSION_CLASSIFIER_AGENT_ID.into(), classifier_model());
    let picker = config_picker::category_picker(
        config_picker::AGENT_CATEGORY_VALUE,
        &app.info.runtime,
        &config,
    )
    .unwrap();
    let classifier_model_row = picker
        .items
        .iter()
        .find(|item| item.value == config_picker::PERMISSION_CLASSIFIER_MODEL_VALUE)
        .expect("classifier model row");
    assert_eq!(
        classifier_model_row
            .badge
            .as_ref()
            .map(|badge| badge.text.as_str()),
        Some("openai/gpt-5.5")
    );
    assert!(picker
        .items
        .iter()
        .any(|item| item.value == config_picker::PERMISSION_CLASSIFIER_REASONING_VALUE));
}
