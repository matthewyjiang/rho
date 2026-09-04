//! Actual delegated runs and parent-to-child delivery for message-card PTY coverage.

use rho_sdk::{
    model::{ModelRequest, ModelResponse},
    ProviderError,
};

use super::{completed, completed_tool_call, tool_result};

const FIRST_CALL: &str = "fixture-message-first-agent";
const SECOND_CALL: &str = "fixture-message-second-agent";
const FIRST_MESSAGE: &str = "fixture-message-first-delivery";
const SECOND_MESSAGE: &str = "fixture-message-second-delivery";
const FIRST_TASK: &str = "Inspect message delivery cache invalidation across resumed sessions";
const SECOND_TASK: &str = "Review message delivery routing";

static FIRST_READY: tokio::sync::Notify = tokio::sync::Notify::const_new();
static SECOND_READY: tokio::sync::Notify = tokio::sync::Notify::const_new();

pub(super) fn is_untitled_task(prompt: &str) -> bool {
    prompt.contains(FIRST_TASK) || prompt.contains(SECOND_TASK)
}

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt == FIRST_TASK || prompt == SECOND_TASK {
        // No timed race with delivery: the child stays available until shutdown.
        if prompt == FIRST_TASK {
            FIRST_READY.notify_one();
        } else {
            SECOND_READY.notify_one();
        }
        request.cancellation.cancelled().await;
        return Some(Err(ProviderError::interrupted("message fixture stopped")));
    }
    if prompt != "fixture agent messages" {
        return None;
    }
    for (id, task) in [(FIRST_CALL, FIRST_TASK), (SECOND_CALL, SECOND_TASK)] {
        if tool_result(request, id).is_none() {
            return Some(completed_tool_call(
                id,
                "agent",
                serde_json::json!({
                    "agent_id": "worker", "prompt": task, "background": true,
                }),
            ));
        }
    }
    if tool_result(request, FIRST_MESSAGE).is_none() {
        // A spawn receipt precedes Running. Wait for both real child provider turns,
        // not a sleep or an agents(status) polling loop.
        tokio::select! {
            () = async { FIRST_READY.notified().await; SECOND_READY.notified().await; } => {},
            () = request.cancellation.cancelled() => return Some(Err(ProviderError::interrupted("message fixture stopped"))),
        }
    }
    for (id, launch, message) in [
        (
            FIRST_MESSAGE,
            FIRST_CALL,
            "Keep cache changes isolated from routing.".to_string(),
        ),
        (
            SECOND_MESSAGE,
            SECOND_CALL,
            [
                "Check the parent-to-child route before changing the renderer.",
                "Keep the original message available when expanded.",
                "Delivery detail beyond the collapsed preview.",
            ]
            .join("\n"),
        ),
    ] {
        if tool_result(request, id).is_none() {
            let run_id = tool_result(request, launch)?
                .content
                .split_whitespace()
                .nth(1)?;
            return Some(completed_tool_call(
                id,
                "agents",
                serde_json::json!({
                    "action": "message", "id": run_id, "message": message,
                }),
            ));
        }
    }
    Some(completed("message deliveries queued"))
}
