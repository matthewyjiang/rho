//! The native tool that fronts one remote MCP tool.
//!
//! A `tools/call` is an RPC on a session the host already owns, so the tool
//! itself holds no process or network capability. What it does own is the live
//! link between one invocation and the server: the progress token the server
//! reports against, and the request handle that carries a real
//! `notifications/cancelled` when the turn is cancelled.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};

use rho_sdk::{
    model::ToolSpec,
    tool::{
        PreparedToolInvocation, Tool, ToolError, ToolErrorKind, ToolInvocation, ToolOutput,
        ToolPreparationContext, ToolPrepareFuture, ToolProgressSender, ToolSecurity,
    },
    CancellationToken,
};
use rmcp::{
    model::{CallToolRequest, CallToolRequestParams, ClientRequest, ServerResult},
    service::{PeerRequestOptions, RequestHandle, ServiceError, ServiceRole},
    Peer, RoleClient,
};

use super::{
    config::McpTransport,
    definition::McpToolDefinition,
    progress::McpProgressRouter,
    result::{self, RenderedResult},
};

// Bound in-flight tool calls so an unresponsive server cannot hang a turn.
pub(super) const MCP_TOOL_CALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

const CANCEL_REASON: &str = "Rho cancelled the turn";

/// The part of an exported MCP tool that can change while a session runs.
///
/// `tools/list_changed` lets a server revise a tool's description, schema,
/// output contract, and annotations, and withdraw it entirely. Rho reads the
/// definition when it builds each run, so a revision reaches the model on the
/// next turn without a restart. A withdrawn tool stays registered under its
/// exported name and fails with a clear reason, because the registry itself is
/// fixed for the session.
#[derive(Debug)]
pub(super) struct McpToolSlot {
    definition: RwLock<McpToolDefinition>,
    available: AtomicBool,
}

impl McpToolSlot {
    pub(super) fn new(definition: McpToolDefinition) -> Self {
        Self {
            definition: RwLock::new(definition),
            available: AtomicBool::new(true),
        }
    }

    pub(super) fn definition(&self) -> McpToolDefinition {
        self.read().clone()
    }

    /// Returns `true` when the incoming definition differs from the live one.
    pub(super) fn refresh(&self, definition: McpToolDefinition) -> bool {
        self.available.store(true, Ordering::Relaxed);
        let mut current = self.write();
        if *current == definition {
            return false;
        }
        *current = definition;
        true
    }

    pub(super) fn withdraw(&self) {
        self.available.store(false, Ordering::Relaxed);
    }

    fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    /// A poisoned lock still holds a complete definition, so recover rather
    /// than turn an unrelated panic into a failed tool call.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, McpToolDefinition> {
        self.definition
            .read()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, McpToolDefinition> {
        self.definition
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

pub(super) struct McpTool {
    pub(super) slot: Arc<McpToolSlot>,
    pub(super) identity: String,
    pub(super) remote_name: String,
    pub(super) peer: Peer<RoleClient>,
    pub(super) progress: McpProgressRouter,
    pub(super) transport: McpTransport,
    pub(super) max_output_bytes: usize,
}

impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.slot.definition().spec
    }

    fn security(&self) -> ToolSecurity {
        // Config is the trust boundary: enabling a server starts it at session
        // load. Tool calls are RPCs on that already-running host-owned session
        // and must not pretend to spawn a process or open a fresh network grant.
        ToolSecurity::built_in([])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let arguments = invocation.into_arguments();
        Box::pin(async move {
            if !self.slot.is_available() {
                return Err(ToolError::new(
                    ToolErrorKind::Execution,
                    format!(
                        "MCP server `{}` withdrew tool `{}`; restart the session to refresh its tools",
                        self.identity, self.remote_name
                    ),
                ));
            }
            let Some(arguments) = arguments.as_object().cloned() else {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "MCP tool arguments must be a JSON object",
                ));
            };
            let definition = self.slot.definition();
            let metadata = definition.presentation.metadata(&self.transport);
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                metadata.clone(),
                move |context| {
                    Box::pin(async move {
                        let rendered = call_remote_tool(
                            McpCall {
                                peer: &self.peer,
                                progress: &self.progress,
                                remote_name: self.remote_name.clone(),
                                arguments,
                                expectation: definition.expectation,
                            },
                            context.cancellation(),
                            Some(context.progress().clone()),
                            self.max_output_bytes,
                        )
                        .await?;
                        // Binary content the server returned rides on the card
                        // as an asset; the model reads the descriptor instead.
                        let mut metadata = metadata;
                        for asset in rendered.assets {
                            metadata = metadata.asset(asset);
                        }
                        Ok(ToolOutput::text(rendered.text).metadata(metadata))
                    })
                },
            ))
        })
    }
}

/// Everything one `tools/call` needs from the owning session.
pub(super) struct McpCall<'a> {
    pub(super) peer: &'a Peer<RoleClient>,
    pub(super) progress: &'a McpProgressRouter,
    pub(super) remote_name: String,
    pub(super) arguments: serde_json::Map<String, serde_json::Value>,
    /// What the tool's declaration says the result must contain.
    pub(super) expectation: super::result::ResultExpectation,
}

/// Issue one `tools/call` and return the serialized MCP result.
///
/// The request goes out as a cancellable handle rather than a plain await so
/// two things hold: the server's progress token is known before the response
/// arrives, and a cancelled turn tells the server to stop instead of silently
/// abandoning work it keeps doing.
pub(super) async fn call_remote_tool(
    call: McpCall<'_>,
    cancellation: &CancellationToken,
    progress_sender: Option<ToolProgressSender>,
    max_output_bytes: usize,
) -> Result<RenderedResult, ToolError> {
    let McpCall {
        peer,
        progress,
        remote_name,
        arguments,
        expectation,
    } = call;
    let params = CallToolRequestParams::new(remote_name).with_arguments(arguments);
    let mut handle = peer
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            PeerRequestOptions::no_options(),
        )
        .await
        .map_err(execution_error)?;

    // Subscribe before awaiting: the server may report progress immediately.
    let _subscription =
        progress_sender.map(|sender| progress.subscribe(handle.progress_token.clone(), sender));

    // The response channel is awaited by reference so the handle survives the
    // select and can still carry a cancellation to the server.
    let outcome = tokio::select! {
        response = &mut handle.rx => CallOutcome::Answered(response),
        () = cancellation.cancelled() => CallOutcome::Cancelled,
        () = tokio::time::sleep(MCP_TOOL_CALL_BUDGET) => CallOutcome::TimedOut,
    };
    let response = match outcome {
        CallOutcome::Answered(response) => response,
        CallOutcome::Cancelled => {
            cancel_handle(handle).await;
            return Err(ToolError::cancelled());
        }
        CallOutcome::TimedOut => {
            cancel_handle(handle).await;
            return Err(ToolError::new(
                ToolErrorKind::Execution,
                format!(
                    "MCP tool call exceeded its {}s budget",
                    MCP_TOOL_CALL_BUDGET.as_secs()
                ),
            ));
        }
    };
    match response {
        Ok(Ok(ServerResult::CallToolResult(result))) => {
            result::render(&result, &expectation, max_output_bytes)
        }
        Ok(Ok(_)) => Err(ToolError::new(
            ToolErrorKind::Execution,
            "MCP server answered tools/call with an unexpected result",
        )),
        Ok(Err(error)) => Err(execution_error(error)),
        // The oneshot closed without a value: the session's transport is gone.
        Err(_) => Err(ToolError::new(
            ToolErrorKind::Execution,
            "MCP session closed before the tool call returned",
        )),
    }
}

enum CallOutcome<T> {
    Answered(T),
    Cancelled,
    TimedOut,
}

fn execution_error(error: ServiceError) -> ToolError {
    ToolError::new(ToolErrorKind::Execution, error.to_string())
}

/// Cancel an in-flight handle so the server learns the turn ended.
///
/// Kept separate from the select arm above because `RequestHandle::cancel`
/// consumes the handle, and the select borrows it.
async fn cancel_handle<R: ServiceRole>(handle: RequestHandle<R>) {
    if let Err(error) = handle.cancel(Some(CANCEL_REASON.into())).await {
        tracing::debug!(error = %error, "could not notify MCP server of cancellation");
    }
}

pub(super) fn namespaced_tool_name(server: &str, tool: &str) -> String {
    fn component(value: &str) -> String {
        const ESCAPE_PREFIX: &str = "_rho_";
        let already_safe = value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if already_safe && !value.starts_with(ESCAPE_PREFIX) {
            return value.to_string();
        }

        let mut encoded = String::with_capacity(ESCAPE_PREFIX.len() + value.len() * 2);
        encoded.push_str(ESCAPE_PREFIX);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in value.bytes() {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
        encoded
    }
    format!("mcp__{}__{}", component(server), component(tool))
}
