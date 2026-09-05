use std::{sync::Arc, time::Duration};

mod advisor;
mod agent_message;
mod attach;
mod boundary_notifications;
mod compact;
mod docs_demo;
mod edit;
mod goal;
mod quiet_subagent;
mod response_scenarios;
mod stream_scenarios;

use rho_sdk::{
    model::{
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ToolCall,
    },
    provider::{
        ModelProvider, ModelRequestOptions, NativeCompactionFuture, ProviderEventSender,
        ProviderFuture, ProviderSteeringReceiver,
    },
    CancellationToken, ProviderError,
};

const MODE_ENV: &str = "RHO_TUI_TEST_MODE";
const MATRIX_MODE: &str = "matrix";
const TOOL_CALL_ID: &str = "tui-fixture-tool";
const HOVER_TOOL_CALL_ID: &str = "tui-fixture-hover-tool";
const LONG_APPROVAL_CALL_ID: &str = "tui-fixture-long-approval";
const QUESTIONNAIRE_CALL_ID: &str = "tui-fixture-questionnaire";
const PROGRESS_CALL_ID: &str = "tui-fixture-progress";
const CONCURRENT_SLOW_CALL_ID: &str = "tui-fixture-concurrent-slow";
const CONCURRENT_FAST_CALL_ID: &str = "tui-fixture-concurrent-fast";
const BACKGROUND_AGENT_CALL_ID: &str = "tui-fixture-background-agent";
const SUBAGENT_RAIL_AGENT_CALL_ID: &str = "tui-fixture-subagent-rail-agent";
const PROCESS_RAIL_CALL_ID: &str = "tui-fixture-process-rail";
const BACKGROUND_QUESTIONNAIRE_AGENT_CALL_ID: &str = "tui-fixture-background-questionnaire-agent";
const CLAUDE_AGENT_CALL_ID: &str = "tui-fixture-claude-agent";
const CLAUDE_AGENT_ERROR_CALL_ID: &str = "tui-fixture-claude-agent-error";
const BACKGROUND_CLAUDE_AGENT_CALL_ID: &str = "tui-fixture-background-claude-agent";
const GOAL_RETRY_AGENT_CALL_ID: &str = "tui-fixture-goal-retry-agent";
const AGENTS_LIST_CALL_ID: &str = "tui-fixture-agents-list";
const BACKGROUND_QUESTIONNAIRE_COMPLETION: &str =
    "background agent questionnaire completed with color blue";

pub(super) fn from_env(
    provider: &str,
    model: &str,
) -> Result<Option<Arc<dyn ModelProvider>>, String> {
    let Some(mode) = std::env::var_os(MODE_ENV) else {
        return Ok(None);
    };
    let mode = mode
        .into_string()
        .map_err(|_| format!("{MODE_ENV} must be valid UTF-8"))?;
    if mode != MATRIX_MODE {
        return Err(format!("unknown {MODE_ENV} value '{mode}'"));
    }
    Ok(Some(Arc::new(TuiFixtureProvider {
        identity: ModelIdentity::new(provider, "tui-test-fixture", model),
    })))
}

struct TuiFixtureProvider {
    identity: ModelIdentity,
}

impl ModelProvider for TuiFixtureProvider {
    fn identity(&self) -> ModelIdentity {
        self.identity.clone()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move { fixture_response(&request) })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move { fixture_stream(request, events).await })
    }

    fn send_turn_stream_steerable<'a>(
        &'a self,
        request: ModelRequest<'a>,
        _options: ModelRequestOptions,
        events: ProviderEventSender,
        mut steering: ProviderSteeringReceiver,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            let prompt = last_user_text(&request).unwrap_or_default();
            if prompt == "fixture mid-turn steer" {
                return stream_mid_turn_steer(request, events, &mut steering).await;
            }
            drop(steering);
            fixture_stream(request, events).await
        })
    }

    fn native_compact<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> Option<NativeCompactionFuture<'a>> {
        compact::native_compact(request)
    }
}

async fn fixture_stream(
    request: ModelRequest<'_>,
    events: ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    let prompt = last_user_text(&request).unwrap_or_default();
    if let Some(response) = boundary_notifications::intercept(&request) {
        return response;
    }
    if is_subagent_title_request(&request) {
        if agent_message::is_untitled_task(&prompt) {
            return completed("");
        }
        return completed("Fixture run title");
    }
    if let Some(response) = agent_message::intercept(&prompt, &request).await {
        return response;
    }
    if let Some(response) = quiet_subagent::intercept(&prompt, &request).await {
        return response;
    }
    if let Some(response) = docs_demo::intercept(&prompt, &request, &events).await {
        return response;
    }
    if let Some(response) = attach::intercept(&prompt, &request, &events).await {
        return response;
    }
    if let Some(response) = goal::intercept(&prompt, &request, &events).await {
        return response;
    }
    if let Some(response) = stream_scenarios::intercept(&prompt, &request, &events).await {
        return response;
    }
    if let Some(response) = edit::intercept(&prompt, &request, &events).await {
        return response;
    }
    if let Some(response) = advisor::intercept(&prompt, &request, &events).await {
        return response;
    }
    if prompt == "fixture background questionnaire" {
        fixture_sleep(&request.cancellation, Duration::from_secs(2)).await?;
    }
    let response = fixture_response(&request)?;
    let ModelResponse::Assistant(blocks) = &response;
    for block in blocks {
        if let ContentBlock::Text(text) = block {
            events.send(ModelEvent::OutputDelta(text.clone())).await?;
        }
    }
    Ok(response)
}

fn fixture_response(request: &ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
    if is_subagent_title_request(request) {
        if last_user_text(request).is_some_and(|prompt| agent_message::is_untitled_task(&prompt)) {
            return completed("");
        }
        return completed("Fixture run title");
    }
    if let Some(review) = advisor::review(request) {
        return review;
    }
    if let Some(response) = response_scenarios::compaction(request) {
        return response;
    }
    if let Some(response) = goal::intercept_response(request) {
        return response;
    }
    if let Some(response) = response_scenarios::intercept(request) {
        return response;
    }
    let prompt = last_user_text(request).unwrap_or_default();
    completed(format!("fixture response: {prompt}"))
}

fn is_subagent_title_request(request: &ModelRequest<'_>) -> bool {
    let is_title_agent = request.messages.iter().any(|message| {
        matches!(
            message,
            Message::System(text) if text.contains("Generate a concise title for this chat session")
        )
    });
    is_title_agent && last_user_text(request).is_some_and(|text| !text.starts_with("First turn:"))
}

fn last_user_text(request: &ModelRequest<'_>) -> Option<String> {
    request.messages.iter().rev().find_map(|message| {
        let Message::User(content) = message else {
            return None;
        };
        Some(
            content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
                })
                .collect::<String>(),
        )
    })
}

fn tool_result_for_name<'a>(
    request: &'a ModelRequest<'_>,
    name: &str,
) -> Option<&'a rho_sdk::model::ToolResult> {
    let current_turn = request
        .messages
        .iter()
        .rev()
        .take_while(|message| !matches!(message, Message::User(_)))
        .collect::<Vec<_>>();
    let call_id = current_turn.iter().find_map(|message| {
        message
            .completed_assistant_content()?
            .iter()
            .find_map(|block| match block {
                ContentBlock::ToolCall(call) if call.name == name => Some(call.id.as_str()),
                ContentBlock::Text(_) | ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
            })
    })?;
    current_turn.iter().find_map(|message| match message {
        Message::ToolResult(result) if result.id == call_id => Some(result),
        _ => None,
    })
}

fn current_turn_tool_results<'a>(
    request: &'a ModelRequest<'_>,
) -> impl Iterator<Item = &'a rho_sdk::model::ToolResult> + 'a {
    request
        .messages
        .iter()
        .rev()
        .take_while(|message| !matches!(message, Message::User(_)))
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
}

fn tool_result<'a>(
    request: &'a ModelRequest<'_>,
    id: &str,
) -> Option<&'a rho_sdk::model::ToolResult> {
    current_turn_tool_results(request).find(|result| result.id == id)
}

fn completed(text: impl Into<String>) -> Result<ModelResponse, ProviderError> {
    Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

fn completed_tool_call(
    id: &str,
    name: &str,
    arguments: serde_json::Value,
) -> Result<ModelResponse, ProviderError> {
    Ok(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        },
    )]))
}

async fn stream_mid_turn_steer(
    request: ModelRequest<'_>,
    events: ProviderEventSender,
    steering: &mut ProviderSteeringReceiver,
) -> Result<ModelResponse, ProviderError> {
    events
        .send(ModelEvent::OutputDelta("waiting for mid-turn steer".into()))
        .await?;
    if let Some(request) = steering.recv().await {
        if request.claim() {
            request.accept();
        }
    }
    fixture_sleep(&request.cancellation, Duration::from_secs(2)).await?;
    completed("waiting for mid-turn steer")
}

async fn fixture_sleep(
    cancellation: &CancellationToken,
    duration: Duration,
) -> Result<(), ProviderError> {
    tokio::select! {
        () = tokio::time::sleep(duration) => Ok(()),
        () = cancellation.cancelled() => {
            Err(ProviderError::interrupted("fixture provider cancelled"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_steering_and_compaction_requests_without_network_access() {
        let cancellation = CancellationToken::new();
        let steering_messages = [Message::user_text("fixture steer detail")];
        let steering = ModelRequest {
            messages: &steering_messages,
            tools: &[],
            cancellation: cancellation.clone(),
            reasoning_level: rho_sdk::ReasoningLevel::Medium,
            prompt_cache_key: None,
        };
        assert_eq!(
            fixture_response(&steering).unwrap(),
            ModelResponse::Assistant(vec![ContentBlock::Text(
                "steering applied exactly once: fixture steer detail".into()
            )])
        );

        let compaction_messages = [Message::System(
            "Summarize the compacted conversation history for continuation.".into(),
        )];
        let compaction = ModelRequest {
            messages: &compaction_messages,
            tools: &[],
            cancellation,
            reasoning_level: rho_sdk::ReasoningLevel::Medium,
            prompt_cache_key: None,
        };
        assert_eq!(
            fixture_response(&compaction).unwrap(),
            ModelResponse::Assistant(vec![ContentBlock::Text(
                "deterministic compacted conversation summary".into()
            )])
        );
    }

    #[test]
    fn questionnaire_count_is_scoped_to_the_current_user_turn() {
        let messages = vec![
            Message::user_text("fixture questionnaire"),
            Message::ToolResult(rho_sdk::model::ToolResult {
                id: QUESTIONNAIRE_CALL_ID.into(),
                ok: true,
                content: "old answer".into(),
            }),
            Message::user_text("fixture questionnaire"),
            Message::ToolResult(rho_sdk::model::ToolResult {
                id: QUESTIONNAIRE_CALL_ID.into(),
                ok: true,
                content: "current answer".into(),
            }),
        ];
        let request = ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: CancellationToken::new(),
            reasoning_level: rho_sdk::ReasoningLevel::Medium,
            prompt_cache_key: None,
        };

        assert_eq!(
            fixture_response(&request).unwrap(),
            ModelResponse::Assistant(vec![ContentBlock::Text(
                "questionnaire response observed exactly 1 time(s): current answer".into()
            )])
        );
    }
}
