//! Marker-gated write input previews; the tool cannot run until JSON closes.

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed, completed_tool_call, release, tool_result};

const PROMPT: &str = "fixture write input stream";
const CALL_ID: &str = "tui-fixture-write-input-stream";
const TARGET: &str = ".rho-tui-fixture-streamed-write.txt";
const NEXT_FRAGMENT: &str = ".rho-fixture-release-write-next";
const COMPLETE: &str = ".rho-fixture-release-write-complete";
const CONTENT: &str = "FIRST_WRITE_FRAGMENT\nSECOND_WRITE_FRAGMENT\n";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt != PROMPT {
        return None;
    }
    if let Some(result) = tool_result(request, CALL_ID) {
        return Some(completed(if result.ok {
            "streamed write completed successfully"
        } else {
            "streamed write failed"
        }));
    }
    Some(stream(request, events).await)
}

async fn stream(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    release::consume_release(NEXT_FRAGMENT)?;
    release::consume_release(COMPLETE)?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(CALL_ID.into()),
            name: Some("write".into()),
            arguments: format!("{{\"path\":\"{TARGET}\",\"content\":\"FIRST_WRITE_FRAGMENT"),
        })
        .await?;
    release::wait_for_release_or_cancel(NEXT_FRAGMENT, &request.cancellation).await?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: "\\nSECOND_WRITE_FRAGMENT\\n".into(),
        })
        .await?;
    release::wait_for_release_or_cancel(COMPLETE, &request.cancellation).await?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: "\"}".into(),
        })
        .await?;
    completed_tool_call(
        CALL_ID,
        "write",
        serde_json::json!({"path": TARGET, "content": CONTENT}),
    )
}
