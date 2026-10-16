use std::{future::Future, path::Path, pin::Pin, sync::Arc};

use agent_client_protocol::{
    schema::v1::{
        ContentBlock as AcpContentBlock, PromptRequest, RequestPermissionRequest,
        RequestPermissionResponse, SessionId, SessionNotification, SessionUpdate, StopReason,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelEvent, ModelIdentity, ModelResponse},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    SessionOptions,
};
use tokio::sync::mpsc;

use super::{mode::SessionPermission, SessionHost};
use crate::{
    app::{
        acp::AcpClientPort, agent_executor::AgentRunHandle, session_assembly::BuiltSession,
        SubagentManager,
    },
    config::Config,
    diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter,
    session::Session as StoredSession,
    subagent::{RunState, RunStatus},
    tools::sdk_registry::AppToolSet,
};

/// Forwards each agent text chunk so the test can act while a prompt waits.
struct ChunkPort(mpsc::UnboundedSender<String>);

impl AcpClientPort for ChunkPort {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        if let SessionUpdate::AgentMessageChunk(chunk) = notification.update {
            if let AcpContentBlock::Text(text) = chunk.content {
                let _ = self.0.send(text.text);
            }
        }
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        unreachable!("the scripted turns ask for no permission")
    }
}

/// A streamed text answer; ACP forwards only streamed deltas as chunks.
fn text_turn(text: &str) -> ScriptedTurn {
    ScriptedTurn::streaming(
        vec![ModelEvent::OutputDelta(text.into())],
        ModelResponse::Assistant(vec![ContentBlock::Text(text.into())]),
    )
}

async fn next_chunk(chunks: &mut mpsc::UnboundedReceiver<String>) -> String {
    chunks.recv().await.expect("the port outlives the prompt")
}

// Covers: T3 Code sends `session/cancel` before every mid-turn message, so a
// cancelled prompt must leave its running children alive, and the next prompt
// must wait for them and continue with their result. The TUI keeps children
// across cancelled turns the same way.
// Owner: ACP session host prompt lifecycle.
#[tokio::test]
async fn cancelled_prompt_keeps_children_for_the_next_prompt() {
    let home = tempfile::tempdir().unwrap();
    let config = Config::default();
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        ["agents started", "still running", "result incorporated"].map(text_turn),
    ));
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
    let id = stored.id().to_owned();
    let session = runtime
        .session(SessionOptions::new().id(rho_sdk::SessionId::from_string(&id).unwrap()))
        .await
        .unwrap();

    let manager = SubagentManager::new(
        config.clone(),
        home.path().join("config.toml"),
        home.path().to_path_buf(),
    );
    let (status, status_rx) = tokio::sync::watch::channel(RunStatus {
        state: RunState::Running,
        ..Default::default()
    });
    let (completion, completion_rx) = tokio::sync::watch::channel(false);
    let child_cancel = rho_tools::cancellation::RunCancellation::new();
    manager.insert_handle_for_test(
        "abc123",
        &id,
        AgentRunHandle::controlled_for_test(status_rx, completion_rx, child_cancel.clone()),
    );
    let mut tools = AppToolSet::disabled();
    tools.set_subagents_for_tests(manager.clone());

    let built = BuiltSession {
        runtime,
        session,
        provider,
        tools,
        hooks: None,
        approval_receiver: None,
        prompt_template: None,
        prompt: Default::default(),
        diagnostics: RuntimeDiagnostics::new(&config),
    };
    let mut host = SessionHost::from_built(
        SessionId::new(id.as_str()),
        built,
        stored,
        Default::default(),
        SessionPermission {
            mode: config.permission_mode,
            writes: Default::default(),
            workspace_root: home.path().to_path_buf(),
        },
        config.auth.clone(),
        HerdrReporter::default(),
    );
    let gate = host.cancel_handle();
    let (sender, mut chunks) = mpsc::unbounded_channel();
    let port = ChunkPort(sender);
    let config_path = Path::new("/nonexistent/rho-test-config.toml");
    let prompt = |text: &str| {
        PromptRequest::new(
            SessionId::new(id.as_str()),
            vec![AcpContentBlock::from(text)],
        )
    };

    // The first prompt answers, then holds natural completion for the child.
    // The user's next message cancels it.
    let (first, ()) = tokio::join!(
        host.prompt(prompt("run the experiments"), &port, &config, config_path),
        async {
            assert_eq!(next_chunk(&mut chunks).await, "agents started");
            gate.cancel();
        },
    );
    assert_eq!(first.unwrap().stop_reason, StopReason::Cancelled);

    // The follow-up waits for the surviving child and continues with its result.
    let (second, ()) = tokio::join!(
        host.prompt(prompt("status?"), &port, &config, config_path),
        async {
            assert_eq!(next_chunk(&mut chunks).await, "still running");
            status.send_replace(RunStatus {
                state: RunState::Ok,
                result: Some("child finding".into()),
                ..Default::default()
            });
            completion.send_replace(true);
        },
    );
    assert_eq!(
        (
            second.unwrap().stop_reason,
            chunks.try_recv().ok(),
            child_cancel.is_cancelled(),
            manager.status("abc123").unwrap().status.state,
        ),
        (
            StopReason::EndTurn,
            Some("result incorporated".into()),
            false,
            RunState::Ok
        ),
    );
    host.shutdown().await;
}
