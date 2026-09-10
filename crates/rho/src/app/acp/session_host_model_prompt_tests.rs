use super::*;
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    app::active_prompt::ActivePrompt,
    diagnostics::RuntimeDiagnostics,
    model_identity::PromptModel,
    prompt,
    tools::{
        advisor::AdvisorSessionStore,
        sdk_registry::{AppToolSet, ToolSetOptions},
    },
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ModelIdentity,
    provider::{ModelProvider, ScriptedProvider},
};

// Covers: ACP model switches must replace the advisor's cached executor prompt
// together with its live history, including model-specific replacements.
// Owner: ACP session transaction, without credential/network discovery
#[tokio::test]
async fn model_prompt_switch_rebinds_advisor_with_live_history() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".rho/model-prompts");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("next.md"),
        "---\nprovider: test\nmodel: next\nmode: replace\n---\nnext behavior",
    )
    .unwrap();
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "first"),
        Vec::new(),
    ));
    let running = PromptModel::from_sdk_identity(&provider.identity());
    let template = prompt::system_prompt_template_with_home_and_models(
        &[],
        home.path(),
        Some(home.path()),
        prompt::PromptModels {
            running: &running,
            advisor: None,
        },
    );
    let prepared = template.build(&running).unwrap();
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(provider.clone())
        .system_prompt(rho_sdk::SystemPrompt::Custom(prepared.text.clone()))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::new()).await.unwrap();
    session
        .append_message(Message::user_text("retained conversation"))
        .unwrap();
    let config = Config {
        provider: "test".into(),
        model: "next".into(),
        ..Config::default()
    };
    let diagnostics = RuntimeDiagnostics::new(&config);
    let advisor = AdvisorSessionStore::new();
    advisor.bind_session(session.clone());
    let tools = AppToolSet::new(
        &config,
        diagnostics.clone(),
        ToolSetOptions::new(AgentCapabilities::new(
            [ToolCapability::Advisor].into_iter().collect(),
        ))
        .advisor(advisor.clone()),
    );
    let mut active = ActivePrompt::default();
    active.adopt(
        ActivePrompt::from_prepared(prepared),
        &diagnostics,
        Some(&advisor),
    );
    let built = BuiltSession {
        runtime,
        session,
        provider,
        tools,
        hooks: None,
        approval_receiver: None,
        prompt_template: Some(template),
        prompt: active,
        diagnostics,
    };
    let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
    let mut host = SessionHost::from_built(
        SessionId::new(stored.id()),
        built,
        stored,
        config.auth.clone(),
        HerdrReporter::default(),
    );
    let next: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "next"),
        Vec::new(),
    ));
    let expected = host
        .built
        .prompt_template
        .as_ref()
        .unwrap()
        .build(&PromptModel::from_sdk_identity(&next.identity()))
        .unwrap();
    host.switch_provider(next, &config).unwrap();
    assert_eq!(advisor.system_prompt(), Some(expected.text.clone()));
    assert_eq!(
        &host.built.session.history()[..2],
        &[
            Message::System(expected.text),
            Message::user_text("retained conversation"),
        ]
    );
    assert_eq!(
        host.built.prompt.provenance,
        expected.model_prompt.as_ref().map(Into::into)
    );
    assert_eq!(host.built.diagnostics.prompt_sources(), expected.sources);
    host.shutdown().await;
}
