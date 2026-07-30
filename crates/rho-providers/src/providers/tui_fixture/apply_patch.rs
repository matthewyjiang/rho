use std::time::Duration;

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed_tool_call, fixture_sleep, tool_result};

pub(super) const PROMPT: &str = "fixture apply patch";
pub(super) const CANCEL_PROMPT: &str = "fixture cancel apply patch";
const CALL_ID: &str = "tui-fixture-apply-patch";
const CANCEL_CALL_ID: &str = "tui-fixture-cancel-apply-patch";

pub(super) fn is_pending(request: &ModelRequest<'_>) -> bool {
    tool_result(request, CALL_ID).is_none()
}

pub(super) async fn stream(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    stream_patch(
        request,
        events,
        CALL_ID,
        ".rho-tui-fixture-patch.txt",
        "streamed patch line",
        Duration::from_millis(750),
        /*complete_after_sleep*/ true,
    )
    .await
}

pub(super) async fn stream_until_cancelled(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    stream_patch(
        request,
        events,
        CANCEL_CALL_ID,
        ".rho-tui-fixture-cancelled-patch.txt",
        "cancelled patch line",
        Duration::from_secs(30),
        /*complete_after_sleep*/ false,
    )
    .await
}

async fn stream_patch(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
    call_id: &str,
    file_name: &str,
    patch_line: &str,
    sleep: Duration,
    complete_after_sleep: bool,
) -> Result<ModelResponse, ProviderError> {
    let input = format!("*** Begin Patch\n*** Add File: {file_name}\n+{patch_line}\n*** End Patch");
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: Some("apply_patch".into()),
            arguments: format!(
                "{{\"input\":\"*** Begin Patch\\n*** Add File: {file_name}\\n+{patch_line}\\n"
            ),
        })
        .await?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(call_id.into()),
            name: None,
            arguments: String::new(),
        })
        .await?;
    fixture_sleep(&request.cancellation, sleep).await?;
    if complete_after_sleep {
        events
            .send(ModelEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                arguments: r#"*** End Patch"}"#.into(),
            })
            .await?;
    }
    completed_tool_call(call_id, "apply_patch", serde_json::json!({"input": input}))
}

pub(super) fn completion_text(request: &ModelRequest<'_>) -> Option<String> {
    let result = tool_result(request, CALL_ID)?;
    Some(format!(
        "patch lifecycle complete with one result: {}",
        result.content.lines().next().unwrap_or_default()
    ))
}
