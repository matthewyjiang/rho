use std::{future::Future, pin::Pin, sync::Arc};

use agent_client_protocol::{
    schema::v1::{
        ContentBlock as AcpContentBlock, PromptRequest, RequestPermissionRequest,
        RequestPermissionResponse, SessionId, SessionNotification,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    ProviderError, ProviderErrorKind, Retryability, SessionOptions,
};

use super::super::{mode::SessionPermission, SessionHost};
use crate::{
    app::{acp::AcpClientPort, session_assembly::BuiltSession},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter,
    session::Session as StoredSession,
    tools::sdk_registry::AppToolSet,
};

struct SilentPort;

impl AcpClientPort for SilentPort {
    fn send_session_notification(
        &self,
        _notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        unreachable!("the scripted turn asks for no permission")
    }
}

// Covers: a host that kills the agent mid-turn (T3 Code restarts) must still
// find the user's message and the model's started work on session/load, though
// the end-of-turn save never ran.
// Owner: ACP session host turn persistence
#[tokio::test]
async fn unfinished_turn_is_on_disk_for_session_load() {
    let home = tempfile::tempdir().unwrap();
    let config = Config::default();
    // Step one calls a tool; step two fails, so the turn never settles.
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        vec![
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "missing".into(),
                    arguments: serde_json::json!({}),
                },
            )])),
            ScriptedTurn::failed(ProviderError::new(
                ProviderErrorKind::InvalidResponse,
                "host went away",
                Retryability::Permanent,
            )),
        ],
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

    let request = PromptRequest::new(
        SessionId::new(id.as_str()),
        vec![AcpContentBlock::from("the codeword is zebra")],
    );
    host.prompt(
        request,
        &SilentPort,
        &config,
        std::path::Path::new("/nonexistent/rho-test-config.toml"),
    )
    .await
    .expect_err("the second step fails");
    host.shutdown().await;

    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), &id).unwrap();
    let user = Message::user_text("the codeword is zebra");
    assert_eq!(histories.display, vec![user.clone()]);
    assert!(histories.model.contains(&user), "{:?}", histories.model);
    assert!(
        histories
            .model
            .iter()
            .any(|message| message
                .completed_assistant_content()
                .is_some_and(|blocks| blocks.iter().any(
                    |block| matches!(block, ContentBlock::ToolCall(call) if call.id == "call-1")
                ))),
        "{:?}",
        histories.model
    );
}
