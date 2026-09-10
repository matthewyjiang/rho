use pretty_assertions::assert_eq;
use rho_sdk::{model::ModelResponse, provider::ScriptedTurn, UserInput};

use crate::{
    agent::{AgentCapabilities, ToolCapability},
    config::{Config, SearchBackend, WebSearchMode},
    diagnostics::RuntimeDiagnostics,
    tools::sdk_registry::{AppToolSet, ToolSetOptions},
};

// Covers: Off must remove live search, enabling it must respect the host ceiling,
// and an active run must retain its provider and route until the next boundary.
// Owner: interactive runtime configuration transition.
#[tokio::test]
async fn web_search_rebind_preserves_session_and_active_turn() {
    for capable in [false, true] {
        let mut runtime = super::super::tests::test_runtime(vec![ScriptedTurn::completed(
            ModelResponse::Assistant(vec![rho_sdk::model::ContentBlock::Text("done".into())]),
        )])
        .await;
        let mut config = Config::default();
        config.web_search.mode = WebSearchMode::Off;
        runtime.config.web_search.mode = WebSearchMode::Off;
        runtime.tools = AppToolSet::new(
            &config,
            RuntimeDiagnostics::new(&config),
            ToolSetOptions::new(AgentCapabilities::new(if capable {
                [ToolCapability::WebSearch].into_iter().collect()
            } else {
                Default::default()
            })),
        );
        let id = runtime.sessions.session().id().clone();
        let history = runtime.history();
        config.web_search.mode = WebSearchMode::Backend;
        config.web_search.backend = SearchBackend::Firecrawl;
        config.web_search.firecrawl.api_base_url = Some("http://127.0.0.1:3002/proxy".into());
        runtime
            .apply_web_search(config.clone(), None)
            .await
            .unwrap();
        assert_eq!(
            runtime
                .runtime
                .diagnostics()
                .tools()
                .iter()
                .any(|tool| tool.name() == "web_search"),
            capable
        );
        assert_eq!(runtime.sessions.session().id(), &id);
        assert_eq!(runtime.history(), history);
        assert_eq!(runtime.config.web_search, config.web_search);
        runtime
            .start(UserInput::text("busy"), /*display_user*/ None)
            .await
            .unwrap();
        config.web_search.mode = WebSearchMode::Off;
        assert!(runtime
            .apply_web_search(config.clone(), None)
            .await
            .is_err());
        assert_eq!(runtime.config.web_search.mode, WebSearchMode::Backend);
        while runtime.next_event().await.is_some() {}
        runtime.finish_run().await.unwrap();
        runtime.apply_web_search(config, None).await.unwrap();
        assert!(!runtime
            .runtime
            .diagnostics()
            .tools()
            .iter()
            .any(|tool| tool.name() == "web_search"));
        assert_eq!(runtime.sessions.session().id(), &id);
    }
}
