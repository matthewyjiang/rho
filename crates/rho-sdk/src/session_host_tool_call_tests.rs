use std::sync::Arc;

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
    Error, Rho, SessionOptions, UserInput,
};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

#[tokio::test]
async fn host_requested_tool_call_runs_before_the_first_model_request() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let tool = Arc::new(ScriptedTool::new(
        ToolSpec {
            name: "lookup".into(),
            description: "lookup".into(),
            input_schema: json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("tool output")),
    ));
    let runtime = Rho::builder()
        .provider(provider.clone())
        .tool_shared(tool)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let call = ToolCall {
        id: "host-call-1".into(),
        name: "lookup".into(),
        arguments: json!({"key": "value"}),
    };

    let mut run = session
        .start_with_tool_call(UserInput::text("use the selected tool"), call.clone())
        .await
        .unwrap();
    while run.next_event().await.is_some() {}
    let outcome = run.outcome().await.unwrap();

    assert_eq!(outcome.text(), "done");
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].messages,
        vec![
            Message::user_text("use the selected tool"),
            Message::Assistant(vec![ContentBlock::ToolCall(call)]),
            Message::ToolResult(crate::model::ToolResult {
                id: "host-call-1".into(),
                ok: true,
                content: "tool output".into(),
            }),
        ]
    );
}

#[tokio::test]
async fn host_requested_tool_call_rejects_invalid_protocol_fields() {
    let provider = ScriptedProvider::new(identity(), Vec::<ScriptedTurn>::new());
    let runtime = Rho::builder().provider(provider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let invalid_calls = [
        ToolCall {
            id: String::new(),
            name: "lookup".into(),
            arguments: json!({}),
        },
        ToolCall {
            id: "host-call-1".into(),
            name: String::new(),
            arguments: json!({}),
        },
        ToolCall {
            id: "host-call-1".into(),
            name: "lookup".into(),
            arguments: json!([]),
        },
    ];

    for call in invalid_calls {
        let result = session
            .start_with_tool_call(UserInput::text("invalid"), call)
            .await;
        assert!(matches!(result, Err(Error::InvalidConfiguration { .. })));
    }
    assert!(session.history().is_empty());
}
