//! Real child tools with PTY-controlled barriers for parent wake policy.

use std::sync::atomic::{AtomicUsize, Ordering};

use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse},
    ProviderError,
};

use super::{completed, completed_tool_call, tool_result};

const SPAWN: &str = "quiet-parent-spawn";
const CHILD: &str = "fixture quiet notice child";
const RUNNING_CHILD: &str = "fixture quiet running child";
const STREAMING_CHILD: &str = "fixture streaming notice child";
const FIRST: &str = "quiet-cache-inspected";
const SECOND: &str = "quiet-routing-inspected";
const ACTION: &str = "quiet-decision-required";
const COMPLETION_CHILD: &str = "fixture quiet completion child";
const COMPLETION: &str = "quiet-child-final-result";
static PARENT_REQUESTS: AtomicUsize = AtomicUsize::new(0);

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &rho_sdk::provider::ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt == COMPLETION_CHILD {
        return Some(match barrier("completion", request).await {
            Ok(()) => completed(COMPLETION),
            Err(error) => Err(error),
        });
    }
    if prompt == "fixture quiet completion subagent" {
        if tool_result(request, SPAWN).is_none() {
            PARENT_REQUESTS.store(0, Ordering::SeqCst);
            return Some(completed_tool_call(
                SPAWN,
                "agent",
                serde_json::json!({
                    "agent_id": "worker",
                    "prompt": COMPLETION_CHILD,
                }),
            ));
        }
        return Some(completed("quiet completion child dispatched"));
    }
    if prompt.ends_with("fixture quiet completion count") {
        return Some(completed(format!(
            "quiet completion requests={} carried finals={}",
            PARENT_REQUESTS.load(Ordering::SeqCst),
            prompt.matches(COMPLETION).count(),
        )));
    }
    if prompt.contains(COMPLETION) {
        let requests = PARENT_REQUESTS.fetch_add(1, Ordering::SeqCst) + 1;
        return Some(completed(format!(
            "quiet final delivery requests={requests} occurrences={}",
            prompt.matches(COMPLETION).count(),
        )));
    }
    if prompt == CHILD || prompt == RUNNING_CHILD || prompt == STREAMING_CHILD {
        for (stage, tool, message) in [
            ("first", "message_parent", FIRST),
            ("second", "message_parent", SECOND),
            ("action", "request_parent_action", ACTION),
        ] {
            if (prompt == RUNNING_CHILD || prompt == STREAMING_CHILD) && stage != "first" {
                // The first tool result proves the notice was posted while
                // the parent provider request is still held at its barrier.
                if let Err(error) = barrier("posted", request).await {
                    return Some(Err(error));
                }
                request.cancellation.cancelled().await;
                return Some(Err(ProviderError::interrupted("quiet child stopped")));
            }
            if tool_result(request, message).is_none() {
                if let Err(error) = barrier(stage, request).await {
                    return Some(Err(error));
                }
                return Some(completed_tool_call(
                    message,
                    if prompt == STREAMING_CHILD {
                        "request_parent_action"
                    } else {
                        tool
                    },
                    serde_json::json!({"message": message}),
                ));
            }
        }
        // Completion must not be the reason the parent wakes.
        request.cancellation.cancelled().await;
        return Some(Err(ProviderError::interrupted("quiet child stopped")));
    }
    let running = prompt == "fixture quiet running subagent";
    let streaming = prompt == "fixture streaming notice";
    if prompt == "fixture quiet subagent" || running || streaming {
        if tool_result(request, SPAWN).is_none() {
            PARENT_REQUESTS.store(0, Ordering::SeqCst);
            return Some(completed_tool_call(
                SPAWN,
                "agent",
                serde_json::json!({
                    "agent_id": "worker",
                    "prompt": if streaming { STREAMING_CHILD } else if running { RUNNING_CHILD } else { CHILD },
                }),
            ));
        }
        if streaming {
            let prefix = "Streaming verification results:\nThe checks ha";
            if let Err(error) = events
                .send(rho_sdk::model::ModelEvent::OutputDelta(prefix.into()))
                .await
            {
                return Some(Err(error));
            }
            if let Err(error) = barrier("parent", request).await {
                return Some(Err(error));
            }
            let suffix = "ve passed without interruption.";
            if let Err(error) = events
                .send(rho_sdk::model::ModelEvent::OutputDelta(suffix.into()))
                .await
            {
                return Some(Err(error));
            }
            return Some(completed(format!("{prefix}{suffix}")));
        }
        if running {
            if let Err(error) = barrier("parent", request).await {
                return Some(Err(error));
            }
            return Some(completed("quiet running parent completed"));
        }
        return Some(completed("quiet child dispatched"));
    }
    if let Some(opening) =
        opening_input(request).filter(|input| input.contains("fixture quiet request count"))
    {
        return Some(completed(format!(
            "quiet extra requests={} carried notices={}",
            PARENT_REQUESTS.load(Ordering::SeqCst),
            opening.matches(FIRST).count(),
        )));
    }
    if prompt.contains(FIRST) || prompt.contains(SECOND) || prompt.contains(ACTION) {
        let requests = PARENT_REQUESTS.fetch_add(1, Ordering::SeqCst) + 1;
        let notices = [FIRST, SECOND, ACTION]
            .into_iter()
            .map(|token| prompt.matches(token).count())
            .collect::<Vec<_>>();
        return Some(completed(format!(
            "quiet delivery requests={requests} occurrences={notices:?}"
        )));
    }
    None
}

/// What opens the turn this request starts: the human prompt and whatever
/// was delivered with it. An in-process TUI folds queued notices into the
/// prompt; a daemon session delivers them as a boundary input after it.
/// `None` once the turn has replied or called a tool.
fn opening_input(request: &ModelRequest<'_>) -> Option<String> {
    let prompt = request
        .messages
        .iter()
        .rposition(|message| matches!(message, Message::User(_)))?;
    let mut input = String::new();
    for message in &request.messages[prompt..] {
        match message {
            Message::User(content) | Message::BoundaryInput(content) => {
                for block in content {
                    if let ContentBlock::Text(text) = block {
                        input.push_str(text);
                        input.push('\n');
                    }
                }
            }
            Message::Assistant(_)
            | Message::EnrichedAssistant(_)
            | Message::AbortedAssistant(_)
            | Message::ToolResult(_) => return None,
            Message::System(_)
            | Message::CompactionSummary(_)
            | Message::ModelContext(_)
            | Message::ReasoningShift(_) => {}
        }
    }
    Some(input)
}

#[cfg(unix)]
async fn barrier(stage: &str, request: &ModelRequest<'_>) -> Result<(), ProviderError> {
    // In the scenario's workspace, which is not where a daemon runs.
    let socket = tokio::net::UnixDatagram::bind(super::release::workspace_path(&format!(
        ".quiet-child-{stage}.sock"
    )))
    .map_err(|error| ProviderError::interrupted(format!("quiet barrier bind: {error}")))?;
    socket
        .send_to(
            stage.as_bytes(),
            super::release::workspace_path(".quiet-parent-pty.sock"),
        )
        .await
        .map_err(|error| ProviderError::interrupted(format!("quiet barrier ready: {error}")))?;
    let mut command = [0_u8; 1];
    tokio::select! {
        result = socket.recv(&mut command) => {
            result.map_err(|error| ProviderError::interrupted(format!("quiet barrier receive: {error}")))?;
            Ok(())
        }
        () = request.cancellation.cancelled() => Err(ProviderError::interrupted("quiet barrier stopped")),
    }
}

#[cfg(not(unix))]
async fn barrier(_stage: &str, _request: &ModelRequest<'_>) -> Result<(), ProviderError> {
    Err(ProviderError::interrupted(
        "quiet PTY fixture requires Unix",
    ))
}
