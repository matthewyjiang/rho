//! Drive the real sessions tool, retaining search handles for subsequent reads.

use rho_sdk::{
    model::{Message, ModelRequest, ModelResponse},
    ProviderError,
};
use serde_json::json;

use super::{completed, completed_tool_call, tool_result};

pub(super) fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    let action = prompt.strip_prefix("fixture sessions ")?;
    let (id, arguments) = match action {
        "search" => (
            "fixture-sessions-search",
            json!({"action": "search", "query": "quartz"}),
        ),
        "empty" => (
            "fixture-sessions-empty",
            json!({"action": "search", "query": "absentneedle"}),
        ),
        "error" => (
            "fixture-sessions-error",
            json!({"action": "search", "query": " "}),
        ),
        "read" | "read partial" => {
            let id = if action == "read" {
                "fixture-sessions-read"
            } else {
                "fixture-sessions-read-partial"
            };
            if tool_result(request, id).is_some() {
                return Some(completed(format!("sessions {action} complete")));
            }
            let search = request.messages.iter().rev().find_map(|message| {
                let Message::ToolResult(result) = message else {
                    return None;
                };
                (result.id == "fixture-sessions-search")
                    .then(|| serde_json::from_str::<serde_json::Value>(&result.content).ok())
                    .flatten()
            });
            let Some(search) = search else {
                return Some(completed("sessions fixture missing search result"));
            };
            (
                id,
                json!({
                    "action": "read",
                    "session": search["sessions"][0]["session"],
                    "anchor": search["sessions"][0]["excerpts"][0]["anchor"],
                    "chars": if action == "read partial" { 16 } else { 4096 },
                }),
            )
        }
        _ => return None,
    };
    Some(if tool_result(request, id).is_some() {
        completed(format!("sessions {action} complete"))
    } else {
        completed_tool_call(id, "sessions", arguments)
    })
}
