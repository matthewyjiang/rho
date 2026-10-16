use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelIdentity, ModelRequest, ModelResponse, ToolSpec},
    provider::{ModelProvider, ProviderFuture, ScriptedProvider},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput, ToolVisibility},
    SessionOptions,
};

use super::tests::{compactor, RecordingUsage};
use crate::{config::CodemodeMode, tools::code_mode::CodeModeSurface};

struct ClearingProvider(Arc<CodeModeSurface>);

impl ModelProvider for ClearingProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("test", "test", "test")
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            // Simulate live state changing after the compaction boundary.
            self.0.clear_loaded();
            Ok(ModelResponse::Assistant(vec![
                rho_sdk::model::ContentBlock::Text("short summary".into()),
            ]))
        })
    }
}

// Covers: a compaction checkpoint must retain its own tool loadout, not stale
// session metadata or live state from after the compaction boundary.
// Owner: host compactor metadata committed by the SDK.
#[tokio::test]
async fn compaction_commits_the_tool_loadout_from_its_boundary() {
    let surface = Arc::new(CodeModeSurface::default());
    surface.set_mode(CodemodeMode::Only);
    let name = "mcp__docs__lookup";
    surface.sync(&[Arc::new(ScriptedTool::new(
        ToolSpec {
            name: name.into(),
            description: "docs".into(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("docs")),
    ))]);
    surface.search_and_load(name, 1);
    let provider = Arc::new(ClearingProvider(surface.clone()));
    let mut compactor = compactor(
        ScriptedProvider::new(provider.identity(), []),
        RecordingUsage::default(),
        /*context_window*/ None,
    );
    compactor.provider = provider.clone();
    compactor.code_mode = Some(surface.clone());
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(provider)
        .compactor(compactor)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::new()).await.unwrap();
    for message in [
        Message::user_text("first request"),
        Message::assistant_text("old answer".repeat(100)),
        Message::user_text("latest instruction"),
    ] {
        session.append_message(message).unwrap();
    }
    session.compact().await.unwrap();
    assert_eq!(surface.is_advertised(name), false);
    surface.restore(&session.snapshot());
    assert_eq!(surface.is_advertised(name), true);
    runtime.shutdown();
}
