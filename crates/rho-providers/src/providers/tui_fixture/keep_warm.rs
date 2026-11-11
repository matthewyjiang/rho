//! Keep-warm: a `process start` that asks to hold the prompt cache, and the
//! capped pings the host then sends while the session idles.
//!
//! The first ping reads the whole prefix from cache; every later one
//! rewrites it, so a scenario sees one counted ping and then the halt.

use std::sync::atomic::{AtomicU32, Ordering};

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse, ModelUsage},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed, completed_tool_call, tool_result};

const PROMPT: &str = "fixture keep warm";
const CALL_ID: &str = "tui-fixture-keep-warm";
/// The trailing user message of the host's ping request.
const PING_TEXT: &str = "(prompt cache keep-alive; no reply needed)";
const PREFIX_TOKENS: u64 = 4_096;
const PING_COST_USD_MICROS: u64 = 1_000;

/// Pings this process has answered. One session per fixture process pings.
static PINGS: AtomicU32 = AtomicU32::new(0);

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    match prompt {
        PING_TEXT => Some(ping(events).await),
        PROMPT if tool_result(request, CALL_ID).is_some() => {
            Some(completed("keep warm fixture dispatched"))
        }
        PROMPT if offers_keep_warm(request) => Some(completed_tool_call(
            CALL_ID,
            "process",
            serde_json::json!({
                "action": "start",
                "command": "sleep 60",
                "keep_warm": true,
            }),
        )),
        PROMPT => Some(completed("keep warm fixture: keep_warm not offered")),
        _ => None,
    }
}

fn offers_keep_warm(request: &ModelRequest<'_>) -> bool {
    request.tools.iter().any(|tool| {
        tool.name == "process" && tool.input_schema["properties"].get("keep_warm").is_some()
    })
}

async fn ping(events: &ProviderEventSender) -> Result<ModelResponse, ProviderError> {
    let read = PINGS.fetch_add(1, Ordering::Relaxed) == 0;
    let (cache_read_tokens, cache_write_tokens) = if read {
        (Some(PREFIX_TOKENS), None)
    } else {
        (Some(0), Some(PREFIX_TOKENS))
    };
    events
        .send(ModelEvent::Usage(ModelUsage {
            input_tokens: Some(8),
            output_tokens: Some(1),
            cache_read_tokens,
            cache_write_tokens,
            cost_usd_micros: Some(PING_COST_USD_MICROS),
            ..ModelUsage::default()
        }))
        .await?;
    completed("")
}
