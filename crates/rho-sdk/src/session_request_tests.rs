use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;

use crate::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput, ToolVisibility},
    CancellationToken, ContextEstimate, ReasoningLevel, RequestContext, Rho, SessionId,
    SessionOptions,
};

#[derive(Clone, Default)]
struct PreparedContext(Arc<Mutex<Vec<Message>>>);

impl RequestContext for PreparedContext {
    fn prepare(
        &self,
        _session_id: &SessionId,
        _history: &[Message],
        _tools: &[ToolSpec],
        _estimate: ContextEstimate,
    ) {
        *self.0.lock().unwrap() = vec![Message::model_context("prepared context")];
    }

    fn messages(&self, _session_id: &SessionId) -> Vec<Message> {
        self.0.lock().unwrap().clone()
    }
}

struct Advertised;

impl ToolVisibility for Advertised {
    fn is_advertised(&self, name: &str) -> bool {
        name != "hidden"
    }

    fn describe(&self, _spec: &ToolSpec) -> Option<String> {
        Some("projected description".into())
    }
}

// Covers: host side requests must use the real turn's prepared context, registry
// order, advertised schemas, reasoning, and opaque stored cache key on resume.
// Owner: SDK request projection; existing tests never compare side requests to turns.
#[tokio::test]
async fn side_request_prefix_matches_real_turn_projection() {
    let reply = || {
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "done".into(),
        )]))
    };
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [reply(), reply()],
    );
    let source = PreparedContext::default();
    let mut builder = Rho::builder()
        .provider(provider.clone())
        .reasoning_level(ReasoningLevel::High)
        .request_context(source.clone())
        .tool_visibility_shared(Arc::new(Advertised));
    // Deliberately register in a different order from the SDK's BTreeMap.
    for name in ["zebra", "hidden", "alpha"] {
        builder = builder.tool(ScriptedTool::new(
            ToolSpec {
                name: name.into(),
                description: "registered description".into(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ScriptedToolOutcome::Success(ToolOutput::text("unused")),
        ));
    }
    let runtime = builder.build().unwrap();
    let original = runtime
        .session(
            SessionOptions::new()
                .history(vec![Message::user_text("committed history")])
                .prompt_cache_key("legacy opaque cache key"),
        )
        .await
        .unwrap();
    let session = runtime
        .session(SessionOptions::from_snapshot(original.snapshot()))
        .await
        .unwrap();
    let history = session.history();
    source.0.lock().unwrap().clear();
    let prefix = session
        .request_prefix()
        .unwrap()
        .with_trailing_message(Message::user_text("side request"));
    assert_eq!(session.history(), history);
    // No run owns the side request, so idle accounting must stay committed-only.
    assert!(session.core.data.lock().unwrap().working_context.is_none());
    provider
        .send_turn(prefix.as_request(CancellationToken::new()))
        .await
        .unwrap();
    // Require preparation again rather than accidentally sampling stale context.
    source.0.lock().unwrap().clear();
    session
        .continue_history()
        .await
        .unwrap()
        .outcome()
        .await
        .unwrap();
    let mut requests = provider.recorded_requests();
    assert_eq!(
        requests[0].messages.pop(),
        Some(Message::user_text("side request"))
    );
    assert_eq!(requests[0], requests[1]);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zebra"]
    );
    assert_eq!(
        requests[0].messages.last(),
        Some(&Message::model_context("prepared context"))
    );
    assert_eq!(
        requests[0].prompt_cache_key.as_deref(),
        Some("legacy opaque cache key")
    );
}
