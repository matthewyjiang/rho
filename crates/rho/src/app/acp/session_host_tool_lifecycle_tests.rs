use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    schema::v1::{
        ContentBlock as AcpContentBlock, PromptRequest, RequestPermissionRequest,
        RequestPermissionResponse, SessionId, SessionNotification, SessionUpdate, StopReason,
        ToolCallStatus,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolError, ToolFuture, ToolInvocation},
    SessionOptions,
};
use tokio::sync::Notify;

use super::{mode::SessionPermission, persist::HostStorage, SessionHost};
use crate::{
    app::{acp::AcpClientPort, session_assembly::BuiltSession},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    session::Session as StoredSession,
    tools::sdk_registry::AppToolSet,
};

const BLOCKER: &str = "blocker";

/// Tool that signals when it starts, then runs until the run is cancelled.
struct Blocker(Arc<Notify>);

impl Tool for Blocker {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: BLOCKER.into(),
            description: "test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            self.0.notify_one();
            context.cancellation().cancelled().await;
            Err(ToolError::cancelled())
        })
    }
}

/// Host that records every session update it receives.
#[derive(Default)]
struct RecordingPort(Mutex<Vec<SessionUpdate>>);

impl AcpClientPort for RecordingPort {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        self.0.lock().unwrap().push(notification.update);
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        unreachable!("the test tools ask for no permission")
    }
}

/// How one tool call reached the host: a full `tool_call` the host can
/// render, with its title, or a `tool_call_update` that needs an earlier
/// `tool_call`.
#[derive(Debug, PartialEq)]
enum Card {
    Announced(String, ToolCallStatus),
    Updated(Option<ToolCallStatus>),
}

impl RecordingPort {
    /// Tool call notifications since the last take, in arrival order.
    fn take_cards(&self) -> Vec<(String, Card)> {
        std::mem::take(&mut *self.0.lock().unwrap())
            .into_iter()
            .filter_map(|update| match update {
                SessionUpdate::ToolCall(call) => Some((
                    call.tool_call_id.0.to_string(),
                    Card::Announced(call.title, call.status),
                )),
                SessionUpdate::ToolCallUpdate(update) => Some((
                    update.tool_call_id.0.to_string(),
                    Card::Updated(update.fields.status),
                )),
                _ => None,
            })
            .collect()
    }
}

fn tool_turn(id: &str, name: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: serde_json::json!({}),
        },
    )]))
}

// Covers: every tool call a host hears about must open with a `tool_call` and
// end in a terminal status. A call to a tool that does not exist finishes
// without starting, and Zed renders a bare update for an unknown id as a
// failed "Tool call not found" card. Cancel interrupts a running call without
// finishing it, and ACP asks agents to flush pending updates before the
// `cancelled` prompt response, so the card must not stay in progress.
// Owner: ACP session host prompt lifecycle.
#[tokio::test]
async fn tool_calls_open_with_a_card_and_end_in_a_terminal_status() {
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        permission_mode: PermissionMode::Bypass,
        ..Config::default()
    };
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        vec![
            tool_turn("call-missing", "no_such_tool"),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
            tool_turn("call-blocked", BLOCKER),
        ],
    ));
    let started = Arc::new(Notify::new());
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .tool(Blocker(Arc::clone(&started)))
        .build()
        .unwrap();
    let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
    let session = runtime
        .session(SessionOptions::new().id(rho_sdk::SessionId::from_string(stored.id()).unwrap()))
        .await
        .unwrap();
    let built = BuiltSession {
        runtime,
        session,
        provider,
        tools: AppToolSet::disabled(),
        hooks: None,
        approval_receiver: None,
        prompt_template: None,
        prompt: Default::default(),
        diagnostics: RuntimeDiagnostics::new(&config),
    };
    let startup = crate::app::acp::AcpStartup::for_tests(config.clone(), home.path());
    let acp_session_id = SessionId::new(stored.id());
    let mut host = SessionHost::from_built(
        acp_session_id.clone(),
        built,
        HostStorage::saved(stored),
        Default::default(),
        SessionPermission {
            mode: config.permission_mode,
            writes: Default::default(),
            workspace_root: home.path().to_path_buf(),
        },
        &startup,
        Vec::new(),
    );
    let gate = host.cancel_handle();
    let port = RecordingPort::default();
    let prompt =
        |text: &str| PromptRequest::new(acp_session_id.clone(), vec![AcpContentBlock::from(text)]);

    let missing = host
        .prompt(prompt("call a tool that does not exist"), &port, &startup)
        .await
        .unwrap();
    let missing_cards = port.take_cards();

    let (blocked, ()) = tokio::join!(
        host.prompt(prompt("run the blocker"), &port, &startup),
        async {
            started.notified().await;
            gate.cancel();
        },
    );
    let blocked_cards = port.take_cards();

    assert_eq!(
        (
            (missing.stop_reason, missing_cards),
            (blocked.unwrap().stop_reason, blocked_cards),
        ),
        (
            (
                StopReason::EndTurn,
                vec![(
                    "call-missing".to_owned(),
                    Card::Announced("no_such_tool".into(), ToolCallStatus::Failed)
                )],
            ),
            (
                StopReason::Cancelled,
                vec![
                    (
                        "call-blocked".to_owned(),
                        Card::Announced(BLOCKER.into(), ToolCallStatus::InProgress)
                    ),
                    (
                        "call-blocked".to_owned(),
                        Card::Updated(Some(ToolCallStatus::Failed))
                    ),
                ],
            ),
        ),
    );
    host.shutdown().await;
}
