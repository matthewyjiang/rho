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
    app::{
        acp::{AcpClientPort, AcpStartup},
        session_assembly::BuiltSession,
    },
    config::Config,
    diagnostics::RuntimeDiagnostics,
    display_transcript::{DisplayRow, DisplayTranscript},
    session::Session as StoredSession,
    tools::{sdk_registry::AppToolSet, workflow_tracker::WorkflowFinishedSnapshot},
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

/// A host on a fresh stored session whose model replies with `turns`.
/// `before_prompt` sees the tools first, to queue work from an earlier prompt.
async fn host(
    home: &std::path::Path,
    turns: Vec<ScriptedTurn>,
    before_prompt: impl FnOnce(&AppToolSet, &str),
) -> (SessionHost, AcpStartup, String) {
    let config = Config::default();
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        turns,
    ));
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let stored = StoredSession::create_in_root(home, home).unwrap();
    let id = stored.id().to_owned();
    let session = runtime
        .session(SessionOptions::new().id(rho_sdk::SessionId::from_string(&id).unwrap()))
        .await
        .unwrap();
    let tools = AppToolSet::disabled();
    before_prompt(&tools, &id);
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
    let startup = AcpStartup::for_tests(config.clone(), home);
    let host = SessionHost::from_built(
        SessionId::new(id.as_str()),
        built,
        stored,
        Default::default(),
        SessionPermission {
            mode: config.permission_mode,
            writes: Default::default(),
            workspace_root: home.to_path_buf(),
        },
        &startup,
        Vec::new(),
    );
    (host, startup, id)
}

/// Sends `text` as one prompt, expects the turn to fail, and shuts down.
/// Returns the session id to reopen.
async fn prompt_failing(
    (mut host, startup, id): (SessionHost, AcpStartup, String),
    text: &str,
) -> String {
    let request = PromptRequest::new(
        SessionId::new(id.as_str()),
        vec![AcpContentBlock::from(text)],
    );
    host.prompt(request, &SilentPort, &startup)
        .await
        .expect_err("the scripted provider fails");
    host.shutdown().await;
    id
}

fn provider_failure(message: &str) -> ScriptedTurn {
    ScriptedTurn::failed(ProviderError::new(
        ProviderErrorKind::InvalidResponse,
        message,
        Retryability::Permanent,
    ))
}

// Covers: a host that kills the agent mid-turn (T3 Code restarts) must still
// find the user's message and the model's started work on session/load, though
// the end-of-turn save never ran.
// Owner: ACP session host turn persistence
#[tokio::test]
async fn unfinished_turn_is_on_disk_for_session_load() {
    let home = tempfile::tempdir().unwrap();
    // Step one calls a tool; step two fails, so the turn never settles.
    let id = prompt_failing(
        host(
            home.path(),
            vec![
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: "call-1".into(),
                        name: "missing".into(),
                        arguments: serde_json::json!({}),
                    },
                )])),
                provider_failure("host went away"),
            ],
            |_, _| {},
        )
        .await,
        "the codeword is zebra",
    )
    .await;

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

// Covers: a background result shown to the host stays in the saved display
// history when the turn then fails, so session/load still replays it.
// Owner: ACP session host turn persistence
#[tokio::test]
async fn background_result_survives_a_failed_turn() {
    let home = tempfile::tempdir().unwrap();
    // A workflow from an earlier prompt finished while idle, so the first
    // boundary delivers it; the provider then fails before any reply.
    let id = prompt_failing(
        host(
            home.path(),
            vec![provider_failure("provider down")],
            |tools, id| {
                let workflows = tools.workflow_tracker();
                assert!(workflows.register_start("wf-run", "fixture", "digest", Some(id.into())));
                workflows.mark_finished(
                    "wf-run",
                    WorkflowFinishedSnapshot {
                        lifecycle: "completed".into(),
                        outcome: Some("success".into()),
                        nodes: Vec::new(),
                        error: None,
                        outputs: Vec::new(),
                    },
                );
            },
        )
        .await,
        "anything new?",
    )
    .await;

    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), &id).unwrap();
    let rows = histories
        .display
        .iter()
        .filter_map(|message| match message {
            Message::System(text) => DisplayTranscript::from_display(text),
            _ => None,
        })
        .flat_map(|transcript| transcript.0)
        .collect::<Vec<_>>();
    assert!(
        matches!(rows.as_slice(), [DisplayRow::Notice(_)]),
        "{:?}",
        histories.display
    );
}
