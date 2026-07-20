use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelRequest, ModelResponse, ToolCall},
    provider::{ModelProvider, ProviderFuture},
    ProviderError, ProviderErrorKind, Retryability,
};

const MODE_ENV: &str = "RHO_AUTOMATION_TEST_MODE";
const RESPONSE_ENV: &str = "RHO_AUTOMATION_TEST_RESPONSE";
const COMMAND_ENV: &str = "RHO_AUTOMATION_TEST_COMMAND";

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
    Ok(Some(std::sync::Arc::new(AutomationFixtureProvider {
        identity: ModelIdentity::new(provider, "automation-test-fixture", model),
        mode,
        turn: AtomicUsize::new(0),
    })))
}

#[derive(Clone, Copy)]
enum Mode {
    Inspect,
    Fixed,
    Fail,
    Delay,
    ToolFailure,
    ReadFile,
    ProcessThenDelay,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "inspect" => Ok(Self::Inspect),
            "fixed" => Ok(Self::Fixed),
            "fail" => Ok(Self::Fail),
            "delay" => Ok(Self::Delay),
            "tool-failure" => Ok(Self::ToolFailure),
            "read-file" => Ok(Self::ReadFile),
            "process-then-delay" => Ok(Self::ProcessThenDelay),
            _ => Err(format!("unknown {MODE_ENV} value '{value}'")),
        }
    }
}

struct AutomationFixtureProvider {
    identity: ModelIdentity,
    mode: Mode,
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
                )),
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
