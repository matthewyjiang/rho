use super::*;
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    app::{config_repository::ConfigRepository, interactive_runtime::test_edit_tool_runtime},
    config::{EditTool, InternalAgentModelConfig},
    tui::tests::test_app,
};
use pretty_assertions::assert_eq;

fn classifier_model() -> InternalAgentModelConfig {
    InternalAgentModelConfig::new("openai".into(), "gpt-5.5".into(), "api-key".into())
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
