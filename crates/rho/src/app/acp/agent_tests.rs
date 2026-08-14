use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};

use agent_client_protocol::{
    schema::{
        v1::{
            CancelNotification, InitializeRequest, LoadSessionRequest, PromptCapabilities,
            PromptRequest, RequestPermissionRequest, RequestPermissionResponse, SessionId,
            SessionModeId, SessionNotification, SetSessionModeRequest,
        },
        ProtocolVersion,
    },
    Error as AcpError, ErrorCode,
};
use pretty_assertions::assert_eq;

use super::{LiveSession, PromptGate, RhoAcpAgent};
use crate::{
    agent::{AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy, ToolPolicy},
    app::acp::{AcpClientPort, AcpStartup},
    app::agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter,
};

struct NullPort;

impl AcpClientPort for NullPort {
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
        Box::pin(async { Err(AcpError::method_not_found()) })
    }
}

fn test_agent() -> Arc<RhoAcpAgent> {
    let config = Config::default();
    let bound = AgentBinder::bind(
        Arc::new(AgentDefinition {
            id: AgentId::new("rho").expect("test agent id"),
            description: "test".into(),
            prompt: PromptPolicy::Extend(String::new()),
            runtime: AgentRuntimeSpec::Rho {
                tools: ToolPolicy::All,
                model: ModelPolicy::Inherit,
                reasoning: None,
            },
        }),
        AgentInvocation {
            role: AgentRole::AutomationRoot,
            available_tools: crate::agent::AgentCapabilities::default(),
        },
        &config,
    )
    .expect("bind test agent");
    Arc::new(RhoAcpAgent::new(AcpStartup {
        config: config.clone(),
        config_path: PathBuf::from("/tmp/rho-acp-test-config.toml"),
        cwd: PathBuf::from("/tmp"),
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: bound,
        diagnostics: RuntimeDiagnostics::new(&config),
        herdr: HerdrReporter::default(),
    }))
}

// Covers: initialize must advertise loadSession, image, embeddedContext, and no audio.
// Owner: ACP agent handshake
#[test]
fn initialize_advertises_load_session_and_prompt_caps() {
    let request = InitializeRequest::new(ProtocolVersion::V1);
    let response = RhoAcpAgent::initialize(&request);

    assert_eq!(response.protocol_version, request.protocol_version);
    assert!(response.agent_capabilities.load_session);
    assert_eq!(
        response.agent_capabilities.prompt_capabilities,
        PromptCapabilities::new()
            .image(true)
            .audio(false)
            .embedded_context(true)
    );
    assert!(response.auth_methods.is_empty());
}

// Covers: session/set_mode must fail for a known mode without claiming the
// advertised method does not exist.
// Owner: ACP agent handshake
#[test]
fn set_session_mode_is_unsupported() {
    let error = RhoAcpAgent::set_session_mode(&SetSessionModeRequest::new(
        SessionId::new("missing"),
        SessionModeId::new("bypass"),
    ));

    assert_eq!(error.code, ErrorCode::InvalidRequest);
}

// Covers: session/set_mode must reject unknown ids instead of a generic not-found
// Owner: ACP agent handshake
#[test]
fn set_session_mode_rejects_unknown_mode() {
    let error = RhoAcpAgent::set_session_mode(&SetSessionModeRequest::new(
        SessionId::new("missing"),
        SessionModeId::new("yolo"),
    ));

    assert_eq!(error.code, ErrorCode::InvalidParams);
}

// Covers: session/cancel for an unknown id must not panic or fail the connection.
// Owner: ACP agent session map
#[tokio::test]
async fn cancel_unknown_session_is_safe() {
    test_agent()
        .cancel(CancelNotification::new(SessionId::new("missing")))
        .await;
}

// Covers: prompt and load must not succeed against a session the agent does not have.
// Owner: ACP agent session map
#[tokio::test]
async fn missing_session_prompt_and_load_return_errors() {
    let agent = test_agent();
    let port = NullPort;
    let missing = SessionId::new("missing");

    let prompt = agent
        .prompt(PromptRequest::new(missing.clone(), Vec::new()), &port)
        .await
        .expect_err("missing prompt session");
    assert_eq!(prompt.code, ErrorCode::ResourceNotFound);

    agent
        .load_session(
            LoadSessionRequest::new(missing, PathBuf::from("/tmp")),
            &port,
        )
        .await
        .expect_err("missing load session");
}

fn vacant_session() -> Arc<LiveSession> {
    Arc::new(LiveSession {
        host: tokio::sync::Mutex::new(None),
        cancel: Arc::new(PromptGate::new()),
    })
}

// Covers: a second session/prompt must report a busy session, not a missing one.
// Owner: ACP agent session map
#[tokio::test]
async fn prompt_on_a_locked_session_is_busy() {
    let live = vacant_session();
    let _held = live.host.lock().await;

    let error = live
        .try_lock_host(&SessionId::new("busy"))
        .err()
        .expect("busy session");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
}

// Covers: a later session/load must not publish until the earlier replacement
// has finished, so its success still names the host in the map.
// Owner: ACP agent session map
#[tokio::test]
async fn replacement_finishes_before_the_next_one_publishes() {
    let agent = test_agent();
    let session_id = SessionId::new("session");
    let previous = vacant_session();
    let first = vacant_session();
    let second = vacant_session();
    agent
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), Arc::clone(&previous));
    let held = previous.host.lock().await;

    let first_agent = Arc::clone(&agent);
    let first_id = session_id.clone();
    let first_live = Arc::clone(&first);
    let first_install = tokio::spawn(async move {
        first_agent.publish(first_id, first_live).await;
    });

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            {
                let sessions = agent.sessions.lock().await;
                if sessions
                    .get(&session_id)
                    .is_some_and(|live| Arc::ptr_eq(live, &first))
                {
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first replacement should publish");

    let second_agent = Arc::clone(&agent);
    let second_id = session_id.clone();
    let second_live = Arc::clone(&second);
    let second_install = tokio::spawn(async move {
        second_agent.publish(second_id, second_live).await;
    });

    assert!(
        agent
            .sessions
            .lock()
            .await
            .get(&session_id)
            .is_some_and(|live| Arc::ptr_eq(live, &first)),
        "later replacement must not publish while the earlier install still holds the slot"
    );

    drop(held);
    first_install.await.expect("first install");
    second_install.await.expect("second install");
    assert!(agent
        .sessions
        .lock()
        .await
        .get(&session_id)
        .is_some_and(|live| Arc::ptr_eq(live, &second)));
}
