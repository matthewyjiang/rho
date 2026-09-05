//! Real child tools with PTY-controlled barriers for parent wake policy.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rho_sdk::{
    model::{ModelRequest, ModelResponse},
    ProviderError, ProviderErrorKind, Retryability,
};

use super::{completed, completed_tool_call, tool_result};

const SPAWN: &str = "quiet-parent-spawn";
const CHILD: &str = "fixture quiet notice child";
const GOAL_CHILD: &str = "fixture quiet goal child";
const FIRST: &str = "quiet-cache-inspected";
const SECOND: &str = "quiet-routing-inspected";
const ACTION: &str = "quiet-decision-required";
static PARENT_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static GOAL_RETRY: AtomicBool = AtomicBool::new(false);

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt == CHILD || prompt == GOAL_CHILD {
        for (stage, tool, message) in [
            ("first", "message_parent", FIRST),
            ("second", "message_parent", SECOND),
            ("action", "request_parent_action", ACTION),
        ] {
            if prompt == GOAL_CHILD && tool == "message_parent" {
                continue;
            }
            if tool_result(request, message).is_none() {
                if let Err(error) = barrier(stage, request).await {
                    return Some(Err(error));
                }
                return Some(completed_tool_call(
                    message,
                    tool,
                    serde_json::json!({"message": message}),
                ));
            }
        }
        // Completion must not be the reason the parent wakes.
        request.cancellation.cancelled().await;
        return Some(Err(ProviderError::interrupted("quiet child stopped")));
    }
    let goal_retry = prompt.contains("Goal:\nfixture quiet action retry\n");
    if prompt == "fixture quiet subagent" || goal_retry {
        if tool_result(request, SPAWN).is_none() {
            PARENT_REQUESTS.store(0, Ordering::SeqCst);
            GOAL_RETRY.store(goal_retry, Ordering::SeqCst);
            return Some(completed_tool_call(
                SPAWN,
                "agent",
                serde_json::json!({
                    "agent_id": "worker",
                    "prompt": if goal_retry { GOAL_CHILD } else { CHILD },
                    "background": true,
                }),
            ));
        }
        return Some(completed("quiet child dispatched"));
    }
    if prompt.contains(FIRST) || prompt.contains(SECOND) || prompt.contains(ACTION) {
        let requests = PARENT_REQUESTS.fetch_add(1, Ordering::SeqCst) + 1;
        if GOAL_RETRY.load(Ordering::SeqCst) && requests == 1 {
            return Some(Err(ProviderError::new(
                ProviderErrorKind::Unavailable,
                "deterministic parent-action failure",
                Retryability::Permanent,
            )));
        }
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

#[cfg(unix)]
async fn barrier(stage: &str, request: &ModelRequest<'_>) -> Result<(), ProviderError> {
    let socket = tokio::net::UnixDatagram::bind(format!(".quiet-child-{stage}.sock"))
        .map_err(|error| ProviderError::interrupted(format!("quiet barrier bind: {error}")))?;
    socket
        .send_to(stage.as_bytes(), ".quiet-parent-pty.sock")
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
