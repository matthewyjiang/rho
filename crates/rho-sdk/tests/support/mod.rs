use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use rho_sdk::{
    model::{
        ContentBlock, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ToolCall, ToolSpec,
    },
    provider::{ModelProvider, ProviderEventSender, ProviderFuture},
    tool::{Tool, ToolContext, ToolFuture, ToolInvocation, ToolOutput},
    ProviderError, Rho, Session, SessionOptions,
};
use serde_json::json;

pub const TEST_TIMEOUT: Duration = Duration::from_secs(2);

pub fn identity() -> ModelIdentity {
    ModelIdentity::new("reliability", "scripted", "v1")
}

pub fn text_response(text: impl Into<String>) -> ModelResponse {
    ModelResponse::Assistant(vec![ContentBlock::Text(text.into())])
}

pub fn tool_call_response(id: &str, name: &str) -> ModelResponse {
    ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: json!({}),
    })])
}

pub async fn session_with<P>(provider: P) -> Session
where
    P: ModelProvider + 'static,
{
    Rho::builder()
        .provider(provider)
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap()
}

#[derive(Clone, Default)]
pub struct Probe {
    pub started: Arc<AtomicBool>,
    pub dropped: Arc<AtomicBool>,
    pub produced: Arc<AtomicUsize>,
}

impl Probe {
    pub async fn wait_started(&self) {
        tokio::time::timeout(TEST_TIMEOUT, async {
            while !self.started.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("operation did not reach the probed await boundary");
    }

    pub async fn wait_dropped(&self) {
        tokio::time::timeout(TEST_TIMEOUT, async {
            while !self.dropped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled operation retained an orphan future");
    }
}

pub struct DropGuard(Arc<AtomicBool>);

impl DropGuard {
    pub fn new(probe: &Probe) -> Self {
        Self(Arc::clone(&probe.dropped))
    }
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct PendingProvider {
    pub probe: Probe,
}

impl ModelProvider for PendingProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            let _guard = DropGuard::new(&self.probe);
            self.probe.started.store(true, Ordering::Release);
            request.cancellation.cancelled().await;
            Err(ProviderError::interrupted("provider request cancelled"))
        })
    }
}

#[derive(Clone)]
pub struct FloodProvider {
    pub events: usize,
    pub probe: Probe,
}

impl ModelProvider for FloodProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async { Ok(text_response("done")) })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            let _guard = DropGuard::new(&self.probe);
            self.probe.started.store(true, Ordering::Release);
            for index in 0..self.events {
                tokio::select! {
                    result = events.send(ModelEvent::OutputDelta(format!("{index},"))) => {
                        result?;
                        self.probe.produced.fetch_add(1, Ordering::AcqRel);
                    }
                    () = request.cancellation.cancelled() => {
                        return Err(ProviderError::interrupted("provider request cancelled"));
                    }
                }
            }
            Ok(text_response("done"))
        })
    }
}

#[derive(Clone)]
pub struct PendingTool {
    pub probe: Probe,
    pub request_host_input: bool,
}

impl Tool for PendingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "pending".into(),
            description: "wait at a cancellation boundary".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let _guard = DropGuard::new(&self.probe);
            self.probe.started.store(true, Ordering::Release);
            if self.request_host_input {
                let question = rho_sdk::HostQuestion::new(
                    "continue",
                    "continue?",
                    vec![rho_sdk::HostChoice::new("yes", "yes")],
                    rho_sdk::SelectionMode::One,
                )
                .unwrap();
                let request =
                    rho_sdk::HostInputRequest::questionnaire("pending", vec![question]).unwrap();
                context.request_host_input(request).await.map_err(|error| {
                    rho_sdk::tool::ToolError::new(
                        rho_sdk::tool::ToolErrorKind::Cancelled,
                        error.to_string(),
                    )
                })?;
            } else {
                context.cancellation().cancelled().await;
            }
            Err(rho_sdk::tool::ToolError::cancelled())
        })
    }
}

#[derive(Clone)]
pub struct LargeOutputTool {
    pub bytes: usize,
}

impl Tool for LargeOutputTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "large".into(),
            description: "return a deterministic large result".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        let bytes = self.bytes;
        Box::pin(async move { Ok(ToolOutput::text("x".repeat(bytes))) })
    }
}
