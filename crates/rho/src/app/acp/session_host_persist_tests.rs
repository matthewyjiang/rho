use crate::app::acp::prompt_acceptance::PromptAcceptance;
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

use super::SessionHost;
use crate::{
    app::{
        acp::{AcpClientPort, AcpStartup},
        session_assembly::BuiltSession,
        session_runtime::{HostStorage, SessionPermission},
    },
    config::Config,
    diagnostics::RuntimeDiagnostics,
    display_transcript::{DisplayRow, DisplayTranscript},
    session::{NoSessionFound, Session as StoredSession},
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

/// A host on `provider` and `tools` whose session `id` is saved through
/// `storage`.
async fn host(
    workspace: &std::path::Path,
    id: &str,
    storage: HostStorage,
    provider: Arc<dyn ModelProvider>,
    tools: AppToolSet,
    startup: &AcpStartup,
) -> SessionHost {
    let config = &startup.config;
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let session = runtime
        .session(SessionOptions::new().id(rho_sdk::SessionId::from_string(id).unwrap()))
        .await
        .unwrap();
    let built = BuiltSession {
        runtime,
        session,
        provider,
        tools,
        hooks: None,
        approval_receiver: None,
        prompt_template: None,
        prompt: Default::default(),
        diagnostics: RuntimeDiagnostics::new(config),
    };
    SessionHost::from_built(
        crate::app::acp::client_io::ClientIo::local(SessionId::new(id)),
        built,
        storage,
        Default::default(),
        SessionPermission {
            mode: config.permission_mode,
            writes: Default::default(),
            workspace_root: workspace.to_path_buf(),
        },
        startup,
        Default::default(),
    )
}

async fn send(
    host: &mut SessionHost,
    id: &str,
    text: &str,
    startup: &AcpStartup,
) -> anyhow::Result<()> {
    let request = PromptRequest::new(SessionId::new(id), vec![AcpContentBlock::from(text)]);
    host.prompt(request, PromptAcceptance::Implicit, &SilentPort, startup)
        .await
        .map(drop)
}

fn provider_failure(message: &str) -> ScriptedTurn {
    ScriptedTurn::failed(ProviderError::new(
        ProviderErrorKind::InvalidResponse,
        message,
        Retryability::Permanent,
    ))
}

/// Storage for a new session `id` in `root`, created on first save.
fn unsaved_storage(root: &std::path::Path, id: &str) -> HostStorage {
    let (root, id) = (root.to_path_buf(), id.to_owned());
    HostStorage::unsaved(
        root.clone(),
        Box::new(move || StoredSession::create_with_id_in_root_for_test(&root, &root, &id)),
    )
}

/// The model history from the first user message on: what display should
/// hold when display and model text agree.
fn from_first_user(history: &[Message]) -> Vec<Message> {
    let start = history
        .iter()
        .position(|message| matches!(message, Message::User(_)))
        .expect("a user message");
    history[start..].to_vec()
}

// Covers: hosts open sessions they never prompt (T3 Code's provider refresh),
// and each must not leave an empty, unnamed row in /resume. The first prompt
// creates the file with the whole turn.
// Owner: ACP session host turn persistence
#[tokio::test]
async fn new_session_saves_nothing_until_its_first_prompt() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().to_path_buf();
    let startup = AcpStartup::for_tests(Config::default(), &root);
    let id = rho_sdk::SessionId::new().to_string();
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        vec![ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("hello back".into()),
        ]))],
    ));
    let mut host = host(
        &root,
        &id,
        unsaved_storage(&root, &id),
        provider,
        AppToolSet::disabled(),
        &startup,
    )
    .await;

    let unsaved = StoredSession::open_by_id_with_histories_in_root(&root, &root, &id)
        .expect_err("no file before the first prompt");
    assert!(unsaved.is::<NoSessionFound>(), "{unsaved:#}");

    send(&mut host, &id, "hello", &startup).await.unwrap();
    host.shutdown().await;

    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(&root, &root, &id).unwrap();
    assert_eq!(histories.display, from_first_user(&histories.model));
    assert_eq!(histories.display.len(), 2, "{:?}", histories.display);
}

// Covers: a host that kills the agent mid-turn (T3 Code restarts) must still
// find the user's message and the model's started work, tool calls included,
// on session/load and TUI resume, though the end-of-turn save never ran.
// Owner: ACP session host turn persistence
#[tokio::test]
async fn unfinished_turn_is_on_disk_for_session_load() {
    let home = tempfile::tempdir().unwrap();
    let startup = AcpStartup::for_tests(Config::default(), home.path());
    // Step one calls a tool; step two fails, so the turn never settles.
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        vec![
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall::new("call-1".into(), "missing".into(), serde_json::json!({})),
            )])),
            provider_failure("host went away"),
        ],
    ));
    let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
    let id = stored.id().to_owned();
    let mut host = host(
        home.path(),
        &id,
        HostStorage::saved(stored),
        provider,
        AppToolSet::disabled(),
        &startup,
    )
    .await;

    send(&mut host, &id, "the codeword is zebra", &startup)
        .await
        .expect_err("the second step fails");
    host.shutdown().await;

    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), &id).unwrap();
    assert_eq!(histories.display, from_first_user(&histories.model));
    assert_eq!(
        histories.display.first(),
        Some(&Message::user_text("the codeword is zebra"))
    );
    assert!(
        histories
            .display
            .iter()
            .any(|message| message
                .completed_assistant_content()
                .is_some_and(|blocks| blocks.iter().any(
                    |block| matches!(block, ContentBlock::ToolCall(call) if call.id == "call-1")
                ))),
        "{:?}",
        histories.display
    );
}

// Covers: a refusal rollback whose durable leaf commit fails must leave the
// live session on the refusal branch the file still selects, so the next turn
// saves model history and replay that agree.
// Owner: ACP session host turn persistence
#[tokio::test]
async fn failed_refusal_rollback_keeps_live_and_saved_history_together() {
    let home = tempfile::tempdir().unwrap();
    let startup = AcpStartup::for_tests(Config::default(), home.path());
    let reply = |text: &str| ModelResponse::Assistant(vec![ContentBlock::Text(text.into())]);
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        vec![
            ScriptedTurn::streaming(
                vec![rho_sdk::model::ModelEvent::Stopped(
                    rho_sdk::model::ModelStopReason::Refusal,
                )],
                reply("refused"),
            ),
            ScriptedTurn::completed(reply("answered")),
        ],
    ));
    let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
    let id = stored.id().to_owned();
    stored.fail_next_leaf_commit_for_tests();
    let mut host = host(
        home.path(),
        &id,
        HostStorage::saved(stored),
        provider,
        AppToolSet::disabled(),
        &startup,
    )
    .await;

    send(&mut host, &id, "refused ask", &startup)
        .await
        .expect_err("the rollback's leaf commit fails");
    send(&mut host, &id, "follow-up", &startup).await.unwrap();
    let live = host.runtime.built().session.history();
    host.shutdown().await;

    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), &id).unwrap();
    assert_eq!(histories.model, live);
    assert_eq!(histories.display, from_first_user(&histories.model));
    let prompts = histories
        .display
        .iter()
        .filter(|message| matches!(message, Message::User(_)))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        prompts,
        [
            Message::user_text("refused ask"),
            Message::user_text("follow-up")
        ]
    );
}

// Covers: a background result shown to the host stays in the saved display
// history when the turn then fails, so session/load still replays it.
// Owner: ACP session host turn persistence
#[tokio::test]
async fn background_result_survives_a_failed_turn() {
    let home = tempfile::tempdir().unwrap();
    let startup = AcpStartup::for_tests(Config::default(), home.path());
    let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
    let id = stored.id().to_owned();
    // A workflow from an earlier prompt finished while idle, so the first
    // boundary delivers it; the provider then fails before any reply.
    let tools = AppToolSet::disabled();
    let workflows = tools.workflow_tracker();
    assert!(workflows.register_start("wf-run", "fixture", "digest", Some(id.clone())));
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
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        vec![provider_failure("provider down")],
    ));
    let mut host = host(
        home.path(),
        &id,
        HostStorage::saved(stored),
        provider,
        tools,
        &startup,
    )
    .await;

    send(&mut host, &id, "anything new?", &startup)
        .await
        .expect_err("the scripted provider fails");
    host.shutdown().await;

    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), &id).unwrap();
    let [user, Message::System(card)] = histories.display.as_slice() else {
        panic!("{:?}", histories.display);
    };
    assert_eq!(user, &Message::user_text("anything new?"));
    assert!(
        matches!(
            DisplayTranscript::from_display(card)
                .map(|transcript| transcript.0)
                .as_deref(),
            Some([DisplayRow::Notice(_)])
        ),
        "{card}"
    );
}

/// Acceptance that records each ID a prompt is answered with.
fn recording(ids: &Arc<std::sync::Mutex<Vec<String>>>) -> PromptAcceptance {
    let ids = Arc::clone(ids);
    PromptAcceptance::Report(Box::new(move |id| {
        ids.lock().unwrap().push(id.0.to_string())
    }))
}

// Covers: ACP v2 requires a replayed user message to keep the ID its prompt
// was answered with: model turns, including one that ran a tool, and a
// workflow start, which saves its prompt without a turn. A prompt retried
// with the same text after a refusal rolled it back is a new message.
// Owner: ACP session host prompt insertion
#[tokio::test]
async fn accepted_user_message_ids_survive_replay() {
    let home = tempfile::tempdir().unwrap();
    let startup = AcpStartup::for_tests(Config::default(), home.path());
    let reply = |text: &str| {
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            text.into(),
        )]))
    };
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        vec![
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall::new("call-1".into(), "missing".into(), serde_json::json!({})),
            )])),
            reply("first"),
            reply("second"),
            ScriptedTurn::streaming(
                vec![rho_sdk::model::ModelEvent::Stopped(
                    rho_sdk::model::ModelStopReason::Refusal,
                )],
                ModelResponse::Assistant(vec![ContentBlock::Text("refused".into())]),
            ),
            reply("answered"),
        ],
    ));
    let id = "replay-ids".to_owned();
    let mut host = host(
        home.path(),
        &id,
        unsaved_storage(home.path(), &id),
        provider,
        AppToolSet::disabled(),
        &startup,
    )
    .await;

    let accepted = Arc::new(std::sync::Mutex::new(Vec::new()));
    for text in ["call a tool", "and again", "retry", "retry"] {
        let request = PromptRequest::new(SessionId::new(&*id), vec![AcpContentBlock::from(text)]);
        host.prompt(request, recording(&accepted), &SilentPort, &startup)
            .await
            .unwrap();
    }
    let inserted = host
        .runtime
        .save_prompt_rows(
            Message::user_text("/workflow:nightly"),
            &[Message::assistant_text("started")],
        )
        .unwrap();
    recording(&accepted).accept(inserted);
    host.shutdown().await;

    let (stored, _) =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), &id).unwrap();
    let mut replayed: Vec<String> = Vec::new();
    for notification in super::EventMapper::replay_history(
        &SessionId::new(&*id),
        &stored.active_display_history().unwrap(),
        home.path(),
        &AppToolSet::disabled(),
    ) {
        if let agent_client_protocol::schema::v1::SessionUpdate::UserMessageChunk(chunk) =
            notification.update
        {
            let message = chunk
                .message_id
                .expect("replayed chunks are named")
                .0
                .to_string();
            if replayed.last() != Some(&message) {
                replayed.push(message);
            }
        }
    }
    let mut accepted = accepted.lock().unwrap().clone();
    // The first "retry" was refused and rolled back, so it is not replayed.
    let refused = accepted.remove(2);
    assert_eq!(replayed, accepted);
    assert_eq!(replayed.len(), 4);
    assert!(!replayed.contains(&refused));
}
