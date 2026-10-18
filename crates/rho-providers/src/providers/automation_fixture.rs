use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use rho_sdk::{
    model::{
        ContentBlock, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage, ToolCall,
    },
    provider::{ModelProvider, ProviderEventSender, ProviderFuture},
    ProviderError, ProviderErrorKind, Retryability,
};

const MODE_ENV: &str = "RHO_AUTOMATION_TEST_MODE";
const RESPONSE_ENV: &str = "RHO_AUTOMATION_TEST_RESPONSE";
const COMMAND_ENV: &str = "RHO_AUTOMATION_TEST_COMMAND";
const PATH_ENV: &str = "RHO_AUTOMATION_TEST_PATH";
/// When set, every response reports this provider cost in USD micros.
const COST_ENV: &str = "RHO_AUTOMATION_TEST_COST_USD_MICROS";

pub(super) fn from_env(
    provider: &str,
    model: &str,
) -> Result<Option<std::sync::Arc<dyn ModelProvider>>, String> {
    let Some(mode) = std::env::var_os(MODE_ENV) else {
        return Ok(None);
    };
    let mode = mode
        .into_string()
        .map_err(|_| format!("{MODE_ENV} must be valid UTF-8"))?;
    let mode = Mode::parse(&mode)?;
    let cost_usd_micros = std::env::var_os(COST_ENV)
        .map(|cost| {
            cost.to_str()
                .and_then(|cost| cost.parse::<u64>().ok())
                .ok_or_else(|| format!("{COST_ENV} must be a whole number of USD micros"))
        })
        .transpose()?;
    Ok(Some(std::sync::Arc::new(AutomationFixtureProvider {
        identity: ModelIdentity::new(provider, "automation-test-fixture", model),
        mode,
        cost_usd_micros,
        turn: AtomicUsize::new(0),
    })))
}

#[derive(Clone, Copy)]
enum Mode {
    Inspect,
    Fixed,
    Fail,
    AuthFailure,
    Retry,
    Delay,
    ToolFailure,
    ReadFile,
    ReadPath,
    WritePath,
    /// A prompt that is a JSON tool call makes that call, then replies with
    /// its result; any other prompt, such as a host follow-up, gets the
    /// fixed response.
    ToolCall,
    ProcessThenDelay,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "inspect" => Ok(Self::Inspect),
            "fixed" => Ok(Self::Fixed),
            "fail" => Ok(Self::Fail),
            "auth-failure" => Ok(Self::AuthFailure),
            "retry" => Ok(Self::Retry),
            "delay" => Ok(Self::Delay),
            "tool-failure" => Ok(Self::ToolFailure),
            "read-file" => Ok(Self::ReadFile),
            "read-path" => Ok(Self::ReadPath),
            "write-path" => Ok(Self::WritePath),
            "tool-call" => Ok(Self::ToolCall),
            "process-then-delay" => Ok(Self::ProcessThenDelay),
            _ => Err(format!("unknown {MODE_ENV} value '{value}'")),
        }
    }
}

struct AutomationFixtureProvider {
    identity: ModelIdentity,
    mode: Mode,
    cost_usd_micros: Option<u64>,
    turn: AtomicUsize,
}

impl ModelProvider for AutomationFixtureProvider {
    fn identity(&self) -> ModelIdentity {
        self.identity.clone()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            let turn = self.turn.fetch_add(1, Ordering::SeqCst);
            match self.mode {
                Mode::Inspect => completed(inspect_request(&self.identity, &request)),
                Mode::Fixed => completed(fixed_response()),
                Mode::Fail => Err(ProviderError::new(
                    ProviderErrorKind::Other,
                    "deterministic provider failure",
                    Retryability::Permanent,
                )
                .with_diagnostic("fixture provider diagnostic")),
                Mode::AuthFailure => Err(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "deterministic authentication failure",
                    Retryability::Permanent,
                )
                .with_diagnostic("Authorization: Bearer fixture-secret")),
                Mode::Retry if turn == 0 => Err(ProviderError::new(
                    ProviderErrorKind::Unavailable,
                    "deterministic retryable failure",
                    Retryability::Retryable,
                )),
                Mode::Retry => completed(fixed_response()),
                Mode::Delay => delayed(request).await,
                Mode::ToolFailure if turn == 0 => completed_tool_call(
                    "fixture-tool-failure",
                    "read_file",
                    serde_json::json!({"path": "../outside-workspace"}),
                ),
                Mode::ToolFailure => completed(fixed_response()),
                Mode::ReadFile if turn == 0 => completed_tool_call(
                    "fixture-read-file",
                    "read_file",
                    serde_json::json!({"path": "large.txt"}),
                ),
                Mode::ReadFile => completed(last_tool_result(&request)?),
                Mode::ReadPath if turn == 0 => completed_tool_call(
                    "fixture-read-path",
                    "read_file",
                    serde_json::json!({"path": required_env(PATH_ENV)?}),
                ),
                Mode::ReadPath => completed(last_tool_result(&request)?),
                Mode::WritePath if turn == 0 => completed_tool_call(
                    "fixture-write-path",
                    "write",
                    serde_json::json!({
                        "path": required_env(PATH_ENV)?,
                        "content": "written outside the working directory",
                    }),
                ),
                Mode::WritePath => completed(fixed_response()),
                Mode::ToolCall
                    if matches!(
                        request.messages.last(),
                        Some(rho_sdk::model::Message::User(_))
                    ) =>
                {
                    let text = request
                        .messages
                        .iter()
                        .rev()
                        .find_map(|message| match message {
                            rho_sdk::model::Message::User(blocks) => {
                                blocks.iter().find_map(|block| match block {
                                    ContentBlock::Text(text) => Some(text),
                                    _ => None,
                                })
                            }
                            _ => None,
                        })
                        .expect("tool-call fixture needs a user message");
                    match serde_json::from_str::<ToolCall>(text) {
                        Ok(call) => completed_tool_call(&call.id, &call.name, call.arguments),
                        Err(_) => completed(fixed_response()),
                    }
                }
                Mode::ToolCall => completed(last_tool_result(&request)?),
                Mode::ProcessThenDelay if turn == 0 => completed_tool_call(
                    "fixture-process",
                    "process",
                    serde_json::json!({
                        "action": "start",
                        "command": required_env(COMMAND_ENV)?,
                    }),
                ),
                Mode::ProcessThenDelay => delayed(request).await,
            }
        })
    }

    /// The SDK default stream, plus one usage report when a cost is set.
    /// Deltas go through the public `send`, so unlike the default they carry
    /// observation times; fixture turns have no meaningful timing either way.
    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            let cancellation = request.cancellation.clone();
            let response = tokio::select! {
                response = self.send_turn(request) => response?,
                () = cancellation.cancelled() => {
                    return Err(ProviderError::interrupted("provider request cancelled"));
                }
            };
            let ModelResponse::Assistant(blocks) = &response;
            for block in blocks {
                if let ContentBlock::Text(text) = block {
                    events.send(ModelEvent::OutputDelta(text.clone())).await?;
                }
            }
            if let Some(cost_usd_micros) = self.cost_usd_micros {
                let usage = ModelUsage {
                    cost_usd_micros: Some(cost_usd_micros),
                    ..ModelUsage::default()
                };
                events.send(ModelEvent::Usage(usage)).await?;
            }
            Ok(response)
        })
    }
}

fn completed(text: String) -> Result<ModelResponse, ProviderError> {
    Ok(ModelResponse::Assistant(vec![ContentBlock::Text(text)]))
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

fn last_tool_result(request: &ModelRequest<'_>) -> Result<String, ProviderError> {
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            rho_sdk::model::Message::ToolResult(result) => Some(result.content.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::InvalidResponse,
                "fixture expected a tool result",
                Retryability::Permanent,
            )
        })
}

fn fixed_response() -> String {
    std::env::var(RESPONSE_ENV).unwrap_or_else(|_| "fixture response".into())
}

fn required_env(name: &str) -> Result<String, ProviderError> {
    std::env::var(name).map_err(|_| {
        ProviderError::new(
            ProviderErrorKind::Other,
            format!("{name} is required for this fixture mode"),
            Retryability::Permanent,
        )
    })
}

fn inspect_request(identity: &ModelIdentity, request: &ModelRequest<'_>) -> String {
    serde_json::json!({
        "identity": identity,
        "messages": request.messages,
        "tools": request.tools,
        "reasoning": request.reasoning_level.to_string(),
    })
    .to_string()
}

fn delayed(
    request: ModelRequest<'_>,
) -> Pin<Box<dyn Future<Output = Result<ModelResponse, ProviderError>> + Send + '_>> {
    Box::pin(async move {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(30)) => completed(fixed_response()),
            () = request.cancellation.cancelled() => {
                Err(ProviderError::interrupted("fixture provider cancelled"))
            }
        }
    })
}
