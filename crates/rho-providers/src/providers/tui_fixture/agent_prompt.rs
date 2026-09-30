//! Hold agent arguments at durable streaming checkpoints until the PTY releases them.

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse, ModelUsage},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed, completed_tool_call, release, tool_result};

const CALL: &str = "agent-prompt-prefix";
const FINISHED_CALL: &str = "agent-prompt-finished";
const RELEASE: &str = ".rho-fixture-release-agent-prompt";
// Exceeds the old 400-character / eight-line tail preview while fitting the
// expanded PTY viewport. Losing either prefix budget must fail the scenario.
const PREFIX: &str = "prompt-prefix inspect the parser and preserve the initial instructions while wrapped-prefix stays readable.\nsecond instruction: inspect the decoding boundary\nthird instruction: preserve the initial constraints\nfourth instruction: compare partial and final inputs\nfifth instruction: keep cancellation behavior intact\nsixth instruction: inspect the argument accumulator\nseventh instruction: check escaped newline handling\neighth instruction: retain the complete task context\nninth instruction: report the focused result\nprompt-tail-one";
const SUFFIX: &str = "\nprompt-tail-two";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt.starts_with("prompt-prefix inspect the parser") {
        // Keep the launch card durable without child output or completion
        // notifications moving it out of the viewport.
        request.cancellation.cancelled().await;
        return Some(Err(ProviderError::interrupted(
            "agent prompt child stopped",
        )));
    }
    if prompt == "fixture agent prompt finished" {
        if tool_result(request, FINISHED_CALL).is_some() {
            return Some(completed("agent prompt result received"));
        }
        // A missing catalog entry finishes the actual agent ToolCall with an
        // error instead of a background launch. This exercises the finished
        // card's plain-text failure fallback without inventing a foreground agent mode.
        let prompt = PREFIX
            .replace("prompt-prefix", "finished-prompt-prefix")
            .replace("wrapped-prefix", "wrapped-finished-prefix")
            .replace("prompt-tail-one", "finished-prompt-tail");
        return Some(completed_tool_call(
            FINISHED_CALL,
            "agent",
            serde_json::json!({"agent_id": "absent", "prompt": prompt}),
        ));
    }
    if prompt != "fixture agent prompt" {
        return None;
    }
    if tool_result(request, CALL).is_some() {
        return Some(completed("agent prompt launched"));
    }
    Some(stream(request, events).await)
}

async fn stream(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    release::consume_release(RELEASE)?;
    let encoded = serde_json::to_string(PREFIX)
        .map_err(|error| ProviderError::interrupted(format!("encode fixture prompt: {error}")))?;
    let initial = format!(
        "{{\"agent_id\":\"worker\",\"prompt\":{}",
        &encoded[..encoded.len() - 1]
    );
    for (index, arguments) in [initial, "\\nprompt-tail-two\"}".into()]
        .into_iter()
        .enumerate()
    {
        events
            .send(ModelEvent::ToolCallDelta {
                index: 0,
                id: (index == 0).then(|| CALL.into()),
                name: (index == 0).then(|| "agent".into()),
                arguments,
            })
            .await?;
        // Ordered usage is the visible receipt that arguments have arrived;
        // the marker prevents either phase from disappearing under slow CI.
        events
            .send(ModelEvent::Usage(ModelUsage {
                input_tokens: Some(11_000),
                context_window: Some(100_000),
                cost_usd_micros: Some(1_000_000),
                ..ModelUsage::default()
            }))
            .await?;
        release::wait_for_release_or_cancel(RELEASE, &request.cancellation).await?;
    }
    completed_tool_call(
        CALL,
        "agent",
        serde_json::json!({"agent_id": "worker", "prompt": format!("{PREFIX}{SUFFIX}")}),
    )
}
