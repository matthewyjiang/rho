//! Native checklist fixture with enough items to exercise preview overflow.

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed, completed_tool_call, tool_result};

const CALL_ID: &str = "tui-fixture-todo";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
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
