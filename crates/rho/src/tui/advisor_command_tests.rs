use pretty_assertions::assert_eq;

use super::*;
use crate::{
    commands::parse_command, config::InternalAgentModelConfig, tui::tests::test_app,
    tui::PickerAction,
};

fn invocation(command: &str) -> CommandInvocation {
    parse_command(command).unwrap().unwrap()
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
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        self.applied.push(model);
        std::future::ready(Ok(()))
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

fn open_picker(app: &App) -> &crate::tui::UiPicker {
    match app.input_ui.composer() {
        ComposerMode::Picker(picker) => picker,
        composer => panic!("expected an open picker, found {composer:?}"),
    }
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

// Covers: /advisor on without an advisor model asks for one instead of turning the mode on
// Owner: advisor command
#[tokio::test]
async fn enabling_advisor_mode_without_a_model_opens_the_model_picker() {
    let mut app = test_app();
    let mut agent = FakeAdvisorRuntime::default();

    app.execute_advisor_command_with_runtime(invocation("/advisor on"), &mut agent)
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
    let picker = open_picker(&app);
    assert_eq!(picker.action, PickerAction::SelectInternalAgentModel);
    assert_eq!(
        app.internal_agent_model_target
            .as_ref()
            .map(|target| (target.id.as_str(), target.origin)),
        Some((
            ADVISOR_AGENT_ID,
            InternalAgentModelPickerOrigin::AdvisorCommand
        ))
    );
    assert_eq!(
        app.status(),
        "select an advisor model to turn advisor mode on"
    );
}

// Covers: the advisor picker never offers the conversation model, which the advisor cannot use
// Owner: advisor model picker
#[tokio::test]
async fn advisor_model_picker_omits_the_conversation_model_row() {
    let mut app = test_app();
    let mut agent = FakeAdvisorRuntime::default();

    app.execute_advisor_command_with_runtime(invocation("/advisor on"), &mut agent)
        .await
        .unwrap();

    let picker = open_picker(&app);
    assert!(!picker
        .items
        .iter()
        .any(|item| item.value == super::super::model_picker::USE_CONVERSATION_MODEL));
}

// Covers: dismissing the advisor model prompt leaves the mode off and says why
// Owner: advisor command
#[tokio::test]
async fn dismissing_the_advisor_model_prompt_leaves_the_mode_off() {
    let mut app = test_app();
    let mut agent = FakeAdvisorRuntime::default();
    app.execute_advisor_command_with_runtime(invocation("/advisor on"), &mut agent)
        .await
        .unwrap();

    app.handle_picker_escape(/*running*/ false).unwrap();

    assert!(!app.info.runtime.advisor_mode);
    assert!(app.internal_agent_model_target.is_none());
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert_eq!(
        app.status(),
        "advisor mode stays off: no advisor model selected"
    );
}

// Covers: an unknown argument reports usage without changing the mode
// Owner: advisor command
#[tokio::test]
async fn unknown_advisor_argument_reports_usage() {
    let mut app = app_with_advisor_model();
    let mut agent = FakeAdvisorRuntime::default();

    app.execute_advisor_command_with_runtime(invocation("/advisor maybe"), &mut agent)
        .await
        .unwrap();

    assert!(!app.info.runtime.advisor_mode);
    assert_eq!(app.status(), "invalid advisor mode");
}
