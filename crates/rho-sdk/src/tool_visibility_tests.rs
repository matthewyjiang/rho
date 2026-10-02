use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    model::{
        ContentBlock, Message, ModelIdentity, ModelRequest, ModelResponse, ToolCall, ToolSpec,
    },
    provider::{ModelProvider, ProviderFuture, ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput, ToolVisibility},
    Rho, SessionOptions,
};

struct Visibility(AtomicBool);
impl ToolVisibility for Visibility {
    fn is_advertised(&self, name: &str) -> bool {
        name == "visible" && self.0.load(Ordering::SeqCst)
    }

    fn describe(&self, _spec: &ToolSpec) -> Option<String> {
        Some("advertised description".into())
    }
}

struct HideDuringRequest {
    provider: ScriptedProvider,
    visibility: Arc<Visibility>,
}
impl ModelProvider for HideDuringRequest {
    fn identity(&self) -> ModelIdentity {
        self.provider.identity()
    }
    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        self.visibility.0.store(false, Ordering::SeqCst);
        self.provider.send_turn(request)
    }
}

// Covers: visibility changes during a request must not revoke an advertised call;
// hidden calls remain unavailable and the next request sees the new subset.
// Owner: SDK orchestration.
#[tokio::test]
async fn calls_use_the_advertised_request_snapshot() {
    let visibility = Arc::new(Visibility(AtomicBool::new(true)));
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(
                ["visible", "hidden"]
                    .into_iter()
                    .map(|name| {
                        ContentBlock::ToolCall(ToolCall {
                            id: name.into(),
                            name: name.into(),
                            arguments: json!({}),
                        })
                    })
                    .collect(),
            )),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let mut builder = Rho::builder()
        .provider(HideDuringRequest {
            provider: provider.clone(),
            visibility: Arc::clone(&visibility),
        })
        .tool_visibility_shared(visibility);
    for name in ["visible", "hidden"] {
        builder = builder.tool(ScriptedTool::new(
            ToolSpec {
                name: name.into(),
                description: name.into(),
                input_schema: json!({"type": "object"}),
            },
            ScriptedToolOutcome::Success(ToolOutput::text("ran")),
        ));
    }
    let runtime = builder.build().unwrap();
    let expected_specs = vec![ToolSpec {
        name: "visible".into(),
        description: "advertised description".into(),
        input_schema: json!({"type": "object"}),
    }];
    // Provider requests, context estimates, and external host projections agree.
    assert_eq!(runtime.advertised_tool_specs(), expected_specs);
    assert_eq!(
        crate::tool::advertised_specs(
            &runtime.tools.specs(),
            runtime.tool_visibility.as_ref().unwrap().as_ref(),
        ),
        expected_specs,
    );
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    session.complete("go").await.unwrap();
    assert_eq!(
        provider
            .recorded_requests()
            .iter()
            .map(|request| request.tools.clone())
            .collect::<Vec<_>>(),
        vec![expected_specs, vec![]]
    );
    assert_eq!(
        session
            .history()
            .into_iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some((result.id, result.ok)),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![("visible".into(), true), ("hidden".into(), false)]
    );
}
