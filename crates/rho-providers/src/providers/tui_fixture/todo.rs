//! Native checklist fixture with enough items to exercise preview overflow.

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed, completed_tool_call, release, tool_result};

const CALL_ID: &str = "tui-fixture-todo";
const UPDATE_ID: &str = "tui-fixture-todo-update";
const UPDATE_RELEASE: &str = ".rho-fixture-release-todo-update";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt == "fixture todo update" {
        return Some(update(request, events).await);
    }
    if prompt != "fixture todo" {
        return None;
    }
    if let Some(result) = tool_result(request, CALL_ID) {
        return Some(completed(if result.ok {
            "todo checklist complete"
        } else {
            "todo checklist failed"
        }));
    }
    // Twelve items exceed the generic collapsed card row budget. Keep completed and
    // active rows at the start so the collapsed card is useful as well.
    let todos = (1..=12)
        .map(|index| {
            let (content, status) = match index {
                1 => ("inspect requirements".to_owned(), "completed"),
                2 => ("implement checklist".to_owned(), "in_progress"),
                3 => ("check tool registry".to_owned(), "completed"),
                _ => (format!("remaining step {index}"), "pending"),
            };
            serde_json::json!({"content": content, "status": status})
        })
        .collect::<Vec<_>>();
    let arguments = serde_json::json!({"todos": todos});
    if let Err(error) = events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(CALL_ID.into()),
            name: Some("todo".into()),
            arguments: arguments.to_string(),
        })
        .await
    {
        return Some(Err(error));
    }
    Some(completed_tool_call(CALL_ID, "todo", arguments))
}

/// Wait for the PTY to open the overlay, then update through codemode without
/// printing the nested result. The overlay must read task state, not tool text.
async fn update(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    if let Some(result) = tool_result(request, UPDATE_ID) {
        return completed(if result.ok {
            "todo update complete"
        } else {
            "todo update failed"
        });
    }
    release::consume_release(UPDATE_RELEASE)?;
    events
        .send(ModelEvent::OutputDelta(
            "waiting to update checklist".into(),
        ))
        .await?;
    release::wait_for_release_or_cancel(UPDATE_RELEASE, &request.cancellation).await?;
    let todos = (1..=12)
        .map(|index| {
            let (content, status) = if index == 12 {
                (
                    "verify task retention\n  preserve exact task details\n\nfinish review"
                        .to_owned(),
                    "in_progress",
                )
            } else {
                (format!("finished step {index}"), "completed")
            };
            serde_json::json!({"content": content, "status": status})
        })
        .collect::<Vec<_>>();
    let arguments = serde_json::json!({"todos": todos});
    completed_tool_call(
        UPDATE_ID,
        "codemode",
        serde_json::json!({"script": format!("call_tool(\"todo\", {arguments})")}),
    )
}
