use std::time::Duration;

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed_tool_call, fixture_sleep, tool_result};

const PROMPT: &str = "fixture edit";
const CANCEL_PROMPT: &str = "fixture cancel edit";
const CALL_ID: &str = "tui-fixture-edit";
const CANCEL_CALL_ID: &str = "tui-fixture-cancel-edit";
const TOOL_NAME: &str = "str_replace";
const ORIGINAL_LINE: &str = "original line";
const ORIGINAL: &str = "original line\n";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt == PROMPT && is_pending(request) {
        return Some(stream(request, events).await);
    }
    if prompt == CANCEL_PROMPT {
        return Some(stream_until_cancelled(request, events).await);
    }
    None
}

fn is_pending(request: &ModelRequest<'_>) -> bool {
    tool_result(request, CALL_ID).is_none()
}

async fn stream(
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

async fn stream_until_cancelled(
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
    // Seed the target so the real str_replace tool can apply against it.
    let cwd = std::env::current_dir().map_err(|error| {
        ProviderError::new(
            rho_sdk::ProviderErrorKind::Other,
            format!("fixture setup: current_dir failed: {error}"),
            rho_sdk::Retryability::Permanent,
        )
    })?;
    let target = cwd.join(file_name);
    std::fs::write(&target, ORIGINAL).map_err(|error| {
        ProviderError::new(
            rho_sdk::ProviderErrorKind::Other,
            format!(
                "fixture setup: could not write '{}': {error}",
                target.display()
            ),
            rho_sdk::Retryability::Permanent,
        )
    })?;
    let arguments = serde_json::json!({
        "path": file_name,
        "old_string": ORIGINAL_LINE,
        "new_string": edit_line,
    });
    // Stream everything but the closing quote and brace so the card renders
    // from partial JSON until completion.
    let complete = arguments.to_string();
    let open = complete[..complete.len() - 2].to_string();
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: Some(TOOL_NAME.into()),
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
    completed_tool_call(call_id, TOOL_NAME, arguments)
}

pub(super) fn completion_text(request: &ModelRequest<'_>) -> Option<String> {
    let result = tool_result(request, CALL_ID)?;
    Some(format!(
        "edit lifecycle complete with one result: {}",
        result.content.lines().next().unwrap_or_default()
    ))
}
