use std::time::Duration;

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed_tool_call, fixture_sleep, tool_result};

pub(super) const PROMPT: &str = "fixture edit";
pub(super) const CANCEL_PROMPT: &str = "fixture cancel edit";
const CALL_ID: &str = "tui-fixture-edit";
const CANCEL_CALL_ID: &str = "tui-fixture-cancel-edit";
const ORIGINAL: &str = "original line\n";
// FNV-1a tag for ORIGINAL via rho_tools::hashline::compute_file_hash.
const ORIGINAL_TAG: &str = "8022";

pub(super) fn is_pending(request: &ModelRequest<'_>) -> bool {
    tool_result(request, CALL_ID).is_none()
}

pub(super) async fn stream(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    stream_edit(
        request,
        events,
        CALL_ID,
        ".rho-tui-fixture-edit.txt",
        "streamed edit line",
        Duration::from_millis(750),
        /*complete_after_sleep*/ true,
    )
    .await
}

pub(super) async fn stream_until_cancelled(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    stream_edit(
        request,
        events,
        CANCEL_CALL_ID,
        ".rho-tui-fixture-cancelled-edit.txt",
        "cancelled edit line",
        Duration::from_secs(30),
        /*complete_after_sleep*/ false,
    )
    .await
}

async fn stream_edit(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
    call_id: &str,
    file_name: &str,
    edit_line: &str,
    sleep: Duration,
    complete_after_sleep: bool,
) -> Result<ModelResponse, ProviderError> {
    // Seed the target so the real edit tool can apply against a known tag.
    if let Ok(cwd) = std::env::current_dir() {
        let _ = std::fs::write(cwd.join(file_name), ORIGINAL);
    }
    let input = format!("[{file_name}#{ORIGINAL_TAG}]\nPUT 1.=1:\n+{edit_line}\n");
    let open = format!("{{\"input\":\"[{file_name}#{ORIGINAL_TAG}]\\nPUT 1.=1:\\n+{edit_line}\\n");
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: Some("edit".into()),
            arguments: open,
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
                arguments: r#""}"#.into(),
            })
            .await?;
    }
    completed_tool_call(call_id, "edit", serde_json::json!({"input": input}))
}

pub(super) fn completion_text(request: &ModelRequest<'_>) -> Option<String> {
    let result = tool_result(request, CALL_ID)?;
    Some(format!(
        "edit lifecycle complete with one result: {}",
        result.content.lines().next().unwrap_or_default()
    ))
}
