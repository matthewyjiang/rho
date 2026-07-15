use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use rho_sdk::{
    model::{
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ToolCall,
    },
    provider::{ModelProvider, ProviderEventSender, ProviderFuture},
    CancellationToken, ProviderError, ProviderErrorKind, Retryability,
};

const MODE_ENV: &str = "RHO_TUI_TEST_MODE";
const MATRIX_MODE: &str = "matrix";
const TOOL_CALL_ID: &str = "tui-fixture-tool";
const QUESTIONNAIRE_CALL_ID: &str = "tui-fixture-questionnaire";
const PROGRESS_CALL_ID: &str = "tui-fixture-progress";
const GOAL_RETRY_CONDITION: &str = "fixture goal retry";
static GOAL_RETRY_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

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
}

async fn fixture_stream(
    request: ModelRequest<'_>,
    events: ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    let prompt = last_user_text(&request).unwrap_or_default();
    if is_goal_retry_prompt(&prompt) {
        if GOAL_RETRY_ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(ProviderError::new(
                ProviderErrorKind::Unavailable,
                "deterministic transient goal turn failure",
                Retryability::Retryable,
            ));
        }
        return completed("fixture goal retry completed after reusing the original prompt");
    }
    match prompt.as_str() {
        "fixture stream" => {
            events
                .send(ModelEvent::ReasoningDelta(
                    "deterministic reasoning phase one\n".into(),
                ))
                .await?;
            fixture_sleep(&request.cancellation, Duration::from_millis(250)).await?;
            events
                .send(ModelEvent::ReasoningDelta(
                    "deterministic reasoning phase two\n".into(),
                ))
                .await?;
            events
                .send(ModelEvent::OutputDelta("assistant stream part one ".into()))
                .await?;
            fixture_sleep(&request.cancellation, Duration::from_millis(350)).await?;
            events
                .send(ModelEvent::OutputDelta("part two".into()))
                .await?;
            completed("assistant stream part one part two")
        }
        "fixture tool" if tool_result(&request, TOOL_CALL_ID).is_none() => {
            let arguments = serde_json::json!({
                "path": ".rho-tui-fixture-output.txt",
                "content": "deterministic tool output\n",
            });
            events
                .send(ModelEvent::ToolCallDelta {
                    index: 0,
                    id: Some(TOOL_CALL_ID.into()),
                    name: Some("write_file".into()),
                    arguments: "{\"path\":\".rho-tui-fixture-output.txt\",".into(),
                })
                .await?;
            fixture_sleep(&request.cancellation, Duration::from_millis(300)).await?;
            events
                .send(ModelEvent::ToolCallDelta {
                    index: 0,
                    id: None,
                    name: None,
                    arguments: "\"content\":\"deterministic tool output\\n\"}".into(),
                })
                .await?;
            completed_tool_call(TOOL_CALL_ID, "write_file", arguments)
        }
        "fixture questionnaire" if tool_result(&request, QUESTIONNAIRE_CALL_ID).is_none() => {
            completed_tool_call(
                QUESTIONNAIRE_CALL_ID,
                "questionnaire",
                serde_json::json!({
                    "title": "Deterministic questionnaire",
                    "reason": "Validate exactly-once host input delivery.",
                    "questions": [{
                        "id": "color",
                        "question": "Choose one color",
                        "type": "choice",
                        "choices": ["red", "blue"],
                        "default": "red",
                        "required": true,
                    }],
                }),
            )
        }
        "fixture progress tool" if tool_result(&request, PROGRESS_CALL_ID).is_none() => {
            events
                .send(ModelEvent::ToolCallDelta {
                    index: 0,
                    id: Some(PROGRESS_CALL_ID.into()),
                    name: Some("tui_fixture_progress".into()),
                    arguments: "{}".into(),
                })
                .await?;
            fixture_sleep(&request.cancellation, Duration::from_millis(500)).await?;
            completed_tool_call(
                PROGRESS_CALL_ID,
                "tui_fixture_progress",
                serde_json::json!({}),
            )
        }
        "fixture steering" => {
            events
                .send(ModelEvent::OutputDelta(
                    "initial turn waiting for steering".into(),
                ))
                .await?;
            fixture_sleep(&request.cancellation, Duration::from_secs(2)).await?;
            completed("initial turn waiting for steering")
        }
        "fixture delay" => {
            events
                .send(ModelEvent::OutputDelta(
                    "partial assistant before cancellation".into(),
                ))
                .await?;
            fixture_sleep(&request.cancellation, Duration::from_secs(30)).await?;
            completed("delay unexpectedly completed")
        }
        "fixture stream failure" => {
            events
                .send(ModelEvent::OutputDelta(
                    "partial assistant before forced stream termination".into(),
                ))
                .await?;
            fixture_sleep(&request.cancellation, Duration::from_millis(300)).await?;
            Err(ProviderError::new(
                ProviderErrorKind::Other,
                "deterministic forced stream termination",
                Retryability::Permanent,
            ))
        }
        "fixture bulk one" | "fixture bulk two" => {
            let response = bulk_response(&prompt);
            events
                .send(ModelEvent::OutputDelta(response.clone()))
                .await?;
            completed(response)
        }
        _ => {
            let response = fixture_response(&request)?;
            let ModelResponse::Assistant(blocks) = &response;
            for block in blocks {
                if let ContentBlock::Text(text) = block {
                    events.send(ModelEvent::OutputDelta(text.clone())).await?;
                }
            }
            Ok(response)
        }
    }
}

fn is_goal_retry_prompt(prompt: &str) -> bool {
    prompt.contains("The user invoked Rho's `/goal` command")
        && prompt.contains(&format!("Goal:\n{GOAL_RETRY_CONDITION}"))
}

fn fixture_response(request: &ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
    if is_compaction_request(request) {
        return completed("deterministic compacted conversation summary");
    }
    if let Some(result) = tool_result(request, TOOL_CALL_ID) {
        return completed(format!(
            "tool lifecycle complete with one result: {}",
            result.content.lines().next().unwrap_or_default()
        ));
    }
    if let Some(result) = tool_result(request, PROGRESS_CALL_ID) {
        return completed(format!(
            "progress tool lifecycle complete with one result: {}",
            result.content
        ));
    }
    if let Some(result) = tool_result(request, QUESTIONNAIRE_CALL_ID) {
        let count = current_turn_tool_results(request)
            .filter(|result| result.id == QUESTIONNAIRE_CALL_ID)
            .count();
        return completed(format!(
            "questionnaire response observed exactly {count} time(s): {}",
            result.content
        ));
    }
    let prompt = last_user_text(request).unwrap_or_default();
    if prompt == "fixture steer detail" {
        return completed("steering applied exactly once: fixture steer detail");
    }
    completed(format!("fixture response: {prompt}"))
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

fn is_compaction_request(request: &ModelRequest<'_>) -> bool {
    matches!(
        request.messages.first(),
        Some(Message::System(message))
            if message.starts_with("Summarize the compacted conversation history")
    )
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

fn bulk_response(prompt: &str) -> String {
    (1..=180)
        .map(|line| {
            format!(
                "{prompt} line {line:03}: deterministic transcript payload {}\n",
                "x".repeat(64)
            )
        })
        .collect()
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

    #[test]
    fn bulk_response_is_long_and_deterministic_for_scroll_smokes() {
        let response = bulk_response("fixture bulk one");
        assert_eq!(response.lines().count(), 180);
        assert!(response.starts_with("fixture bulk one line 001:"));
        assert!(response.contains("fixture bulk one line 180:"));
    }
}
