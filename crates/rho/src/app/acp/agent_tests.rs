use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use agent_client_protocol::{
    schema::v1::{
        CancelNotification, CloseSessionRequest, ContentBlock as AcpContentBlock,
        LoadSessionRequest, PromptRequest, RequestPermissionRequest, RequestPermissionResponse,
        SessionId, SessionModeId, SessionNotification, SetSessionConfigOptionRequest,
        SetSessionModeRequest, StopReason,
    },
    Error as AcpError, ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelIdentity, ModelRequest},
    provider::{ModelProvider, ProviderFuture},
    ProviderError,
};

use super::{HostWait, LiveSession, PromptGate, RhoAcpAgent, SessionHost};
use crate::{
    agent::{AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy, ToolPolicy},
    app::acp::{AcpClientPort, AcpStartup},
    app::agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter,
    session::{NoSessionFound, Session as StoredSession},
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
        host: crate::app::acp::AcpHost::Generic,
        config: config.clone(),
        config_path: PathBuf::from("/tmp/rho-acp-test-config.toml"),
        cwd: PathBuf::from("/tmp"),
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: bound,
        diagnostics: RuntimeDiagnostics::new(&config),
        herdr: HerdrReporter::default(),
        added_dirs: Default::default(),
    }))
}

// Covers: the initialize wire shape hosts parse: loadSession, image,
// embeddedContext, no audio, the MCP transports host `mcpServers` may use,
// additionalDirectories, session/close (Zed and T3 Code only close sessions
// when it is advertised), and `agentInfo` with the same version
// `rho --version` prints.
// Owner: ACP agent handshake
#[test]
fn initialize_response_wire_shape() {
    let response = serde_json::to_value(RhoAcpAgent::initialize()).expect("serialize initialize");

    assert_eq!(
        response,
        serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": { "image": true, "audio": false, "embeddedContext": true },
                "mcpCapabilities": { "http": true, "sse": false },
                "sessionCapabilities": { "additionalDirectories": {}, "close": {} },
                "auth": {},
            },
            "authMethods": [],
            "agentInfo": {
                "name": "rho",
                "title": "Rho",
                "version": env!("CARGO_PKG_VERSION"),
            },
        })
    );
}

// Covers: session/set_mode must reject unknown mode ids as invalid params
// before looking up the session, and a known mode on a missing session as
// not found.
// Owner: ACP agent handshake
#[tokio::test]
async fn set_session_mode_validates_mode_then_session() {
    let agent = test_agent();
    for (mode, expected) in [
        ("yolo", ErrorCode::InvalidParams),
        ("bypass", ErrorCode::ResourceNotFound),
    ] {
        let error = agent
            .set_session_mode(SetSessionModeRequest::new(
                SessionId::new("missing"),
                SessionModeId::new(mode),
            ))
            .await
            .expect_err("missing session");
        assert_eq!(error.code, expected, "mode {mode}");
    }
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
            LoadSessionRequest::new(missing.clone(), PathBuf::from("/tmp")),
            &port,
        )
        .await
        .expect_err("missing load session");

    let config = agent
        .set_config_option(SetSessionConfigOptionRequest::new(
            missing,
            "thought_level",
            "high",
        ))
        .await
        .expect_err("missing config session");
    assert_eq!(config.code, ErrorCode::ResourceNotFound);
}

fn vacant_session() -> Arc<LiveSession> {
    Arc::new(LiveSession {
        host: Arc::new(tokio::sync::Mutex::new(None)),
        cancel: Arc::new(PromptGate::new()),
        replaced: Arc::new(AtomicBool::new(false)),
    })
}

async fn wait_until_replaced(live: &LiveSession) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if live.replaced.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replacement should mark the previous session");
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

// Covers: a prompt sent while the previous one is cancelled and winding down
// waits for the host instead of failing as busy, so hosts that steer by cancel
// and resend (T3 Code) keep the user's message. A prompt behind an uncancelled
// one is still refused at once.
// Owner: ACP agent session map
#[tokio::test]
async fn prompt_waits_only_behind_a_cancelled_prompt() {
    for (label, cancelled, waits) in [
        ("uncancelled prompt", false, false),
        ("cancelled prompt", true, true),
    ] {
        let live = vacant_session();
        live.cancel.begin();
        if cancelled {
            live.cancel.cancel();
        }
        let held = live.host.lock().await;
        let session_id = SessionId::new("steer");
        let next = live.lock_host(&session_id, HostWait::Prompt);
        tokio::pin!(next);
        let (waited, result) = match futures_util::poll!(&mut next) {
            std::task::Poll::Ready(result) => (false, result),
            std::task::Poll::Pending => {
                drop(held);
                (true, next.await)
            }
        };
        // The fixture slot is vacant, so the prompt ends busy either way;
        // only whether it waited for the host differs.
        let error = result.err().expect("vacant slot");
        assert_eq!(
            (waited, error.code),
            (waits, ErrorCode::InvalidRequest),
            "{label}"
        );
    }
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

    wait_until_replaced(&previous).await;

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
            .is_some_and(|live| Arc::ptr_eq(live, &previous)),
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

// Covers: a prompt queued behind a cancelled turn while a reload replaces the
// session must run on the successor, not fail busy on the emptied old slot.
// Owner: ACP agent session map
#[tokio::test]
async fn prompt_queued_during_replacement_moves_to_the_successor() {
    let agent = test_agent();
    let session_id = SessionId::new("steered");
    let previous = vacant_session();
    let replacement = vacant_session();
    agent
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), Arc::clone(&previous));
    previous.cancel.begin();
    previous.cancel.cancel();
    let held = previous.host.lock().await;
    let successor_held = replacement.host.lock().await;

    let port = NullPort;
    let prompt = agent.prompt(PromptRequest::new(session_id.clone(), Vec::new()), &port);
    tokio::pin!(prompt);
    assert!(futures_util::poll!(&mut prompt).is_pending());

    let publish_agent = Arc::clone(&agent);
    let publish_id = session_id.clone();
    let publish_live = Arc::clone(&replacement);
    let publish = tokio::spawn(async move {
        publish_agent.publish(publish_id, publish_live).await;
    });
    wait_until_replaced(&previous).await;
    drop(held);
    assert!(
        futures_util::poll!(&mut prompt).is_pending(),
        "the queued prompt must wait for the successor, not fail on the emptied slot"
    );
    publish.await.expect("publish");
    assert!(
        futures_util::poll!(&mut prompt).is_pending(),
        "the queued prompt must wait on the successor's host"
    );

    drop(successor_held);
    // The fixture successor is vacant, so the prompt ends busy there.
    assert_eq!(
        prompt.await.expect_err("vacant successor").code,
        ErrorCode::InvalidRequest
    );
}

// Covers: a settings change that reaches a host only after a replacement
// claimed it must fail, neither applying to the discarded host nor following
// to the successor, where requests sent after the reload would already have
// overtaken it.
// Owner: ACP agent session map
#[tokio::test]
async fn settings_change_queued_during_replacement_fails_instead_of_moving() {
    let agent = test_agent();
    let session_id = SessionId::new("reloaded");
    let previous = vacant_session();
    let replacement = vacant_session();
    agent
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), Arc::clone(&previous));
    let held = previous.host.lock().await;
    // Held for the whole test: a change that moved to the successor would wait here.
    let _successor_held = replacement.host.lock().await;

    let change = agent.set_config_option(SetSessionConfigOptionRequest::new(
        session_id.clone(),
        "mode",
        "plan",
    ));
    tokio::pin!(change);
    assert!(
        futures_util::poll!(&mut change).is_pending(),
        "the change must queue on the old host before the replacement does"
    );

    let publish_agent = Arc::clone(&agent);
    let publish_id = session_id.clone();
    let publish_live = Arc::clone(&replacement);
    let publish = tokio::spawn(async move {
        publish_agent.publish(publish_id, publish_live).await;
    });
    wait_until_replaced(&previous).await;
    drop(held);
    // The change is first in line for the old host, so it must finish before
    // the replacement can take that host and publish.
    let result = tokio::time::timeout(Duration::from_secs(5), change)
        .await
        .expect("the queued change must not wait on the successor");
    assert_eq!(
        result.expect_err("replaced session").code,
        ErrorCode::InvalidRequest
    );
    publish.await.expect("publish");
}

// Covers: session/load of a live session must not read the file until the old
// host stops saving, and a failed load leaves the old session saving.
// Owner: ACP agent session map
#[tokio::test]
async fn reload_reads_storage_only_after_the_old_host_quiesces() {
    let agent = test_agent();
    let session_id = SessionId::new("reloaded");
    let previous = vacant_session();
    agent
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), Arc::clone(&previous));
    let held = previous.host.lock().await;

    let port = NullPort;
    // No file backs this ID, so the load fails once it reads storage.
    let load = agent.load_session(
        LoadSessionRequest::new(session_id.clone(), PathBuf::from("/tmp")),
        &port,
    );
    tokio::pin!(load);
    assert!(
        futures_util::poll!(&mut load).is_pending(),
        "load must wait for the old prompt to release its host"
    );

    drop(held);
    load.await.expect_err("no stored session");
    assert!(!previous.replaced.load(std::sync::atomic::Ordering::Acquire));
    assert!(agent
        .sessions
        .lock()
        .await
        .get(&session_id)
        .is_some_and(|live| Arc::ptr_eq(live, &previous)));
}

// Covers: tearing down a replaced session must not delay publication of another ID.
// Owner: ACP agent session map
#[tokio::test]
async fn blocked_replacement_does_not_block_unrelated_session_publication() {
    let agent = test_agent();
    let blocked_id = SessionId::new("blocked");
    let other_id = SessionId::new("other");
    let previous = vacant_session();
    let replacement = vacant_session();
    let other = vacant_session();
    agent
        .sessions
        .lock()
        .await
        .insert(blocked_id.clone(), Arc::clone(&previous));
    let held = previous.host.lock().await;

    let blocked_agent = Arc::clone(&agent);
    let blocked_session = blocked_id.clone();
    let blocked_live = Arc::clone(&replacement);
    let blocked_install = tokio::spawn(async move {
        blocked_agent.publish(blocked_session, blocked_live).await;
    });

    wait_until_replaced(&previous).await;

    tokio::time::timeout(
        Duration::from_secs(1),
        agent.publish(other_id.clone(), Arc::clone(&other)),
    )
    .await
    .expect("unrelated session publication must not wait for another session's teardown");

    assert!(
        agent
            .sessions
            .lock()
            .await
            .get(&other_id)
            .is_some_and(|live| Arc::ptr_eq(live, &other)),
        "unrelated session must be visible while the blocked replacement is still tearing down"
    );
    assert!(
        agent
            .sessions
            .lock()
            .await
            .get(&blocked_id)
            .is_some_and(|live| Arc::ptr_eq(live, &previous)),
        "blocked replacement must stay unpublished until the old prompt releases"
    );

    drop(held);
    blocked_install.await.expect("blocked install");
}

// Covers: a session/new or session/load that is still building when shutdown
// starts must be published into the drain, not left live after shutdown returns.
// Owner: ACP agent session map
#[tokio::test]
async fn shutdown_all_waits_for_in_flight_install() {
    let agent = test_agent();
    let session_id = SessionId::new("in-flight");
    let live = vacant_session();
    let (building_tx, building_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let (published_tx, published_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();

    let install_agent = Arc::clone(&agent);
    let install_id = session_id.clone();
    let install_live = Arc::clone(&live);
    let install = tokio::spawn(async move {
        let _gate = install_agent
            .begin_install()
            .await
            .expect("install should start before shutdown");
        building_tx.send(()).expect("building");
        finish_rx.await.expect("finish");
        install_agent
            .publish(install_id, Arc::clone(&install_live))
            .await;
        published_tx.send(()).expect("published");
        release_rx.await.expect("release");
    });

    building_rx.await.expect("install started");
    let shutdown_agent = Arc::clone(&agent);
    let shutdown = tokio::spawn(async move {
        shutdown_agent.shutdown_all().await;
    });

    finish_tx.send(()).expect("allow publish");
    published_rx
        .await
        .expect("published under the install gate");
    assert!(
        agent
            .sessions
            .lock()
            .await
            .get(&session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &live)),
        "shutdown must still be waiting so the in-flight publish is visible to drain"
    );

    release_tx.send(()).expect("release gate");
    install.await.expect("install");
    shutdown.await.expect("shutdown");
    assert!(
        agent.sessions.lock().await.is_empty(),
        "in-flight install must be included in shutdown"
    );
}

// Covers: once shutdown_all returns, a later publish must not install a live host.
// Owner: ACP agent session map
#[tokio::test]
async fn shutdown_all_prevents_later_publication() {
    let agent = test_agent();
    let session_id = SessionId::new("late");
    let live = vacant_session();

    agent.shutdown_all().await;
    agent.publish(session_id.clone(), Arc::clone(&live)).await;

    assert!(
        agent.sessions.lock().await.is_empty(),
        "publication after shutdown_all must not leave a live host"
    );
    let load = agent
        .load_session(
            LoadSessionRequest::new(session_id, PathBuf::from("/tmp")),
            &NullPort,
        )
        .await
        .expect_err("load after shutdown");
    assert_eq!(load.code, ErrorCode::InternalError);
}

// Covers: session/set_config_option on an unknown id must not invent a session
// Owner: ACP agent session map
#[tokio::test]
async fn set_config_option_on_missing_session_is_not_found() {
    let error = test_agent()
        .set_config_option(SetSessionConfigOptionRequest::new(
            SessionId::new("missing"),
            "model",
            "xai/grok-3",
        ))
        .await
        .expect_err("missing session");
    assert_eq!(error.code, ErrorCode::ResourceNotFound);
}

// Covers: a same-ID replacement must not become promptable until the old host
// lock is released, so the cancelled old prompt and a new prompt cannot run
// on two hosts at once.
// Owner: ACP agent session map
#[tokio::test]
async fn replacement_is_not_visible_until_the_old_prompt_releases() {
    let agent = test_agent();
    let session_id = SessionId::new("session");
    let previous = vacant_session();
    let replacement = vacant_session();
    agent
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), Arc::clone(&previous));
    let held = previous.host.lock().await;

    let publish_agent = Arc::clone(&agent);
    let publish_id = session_id.clone();
    let publish_live = Arc::clone(&replacement);
    let publish = tokio::spawn(async move {
        publish_agent.publish(publish_id, publish_live).await;
    });

    wait_until_replaced(&previous).await;
    assert!(
        agent
            .sessions
            .lock()
            .await
            .get(&session_id)
            .is_some_and(|live| Arc::ptr_eq(live, &previous)),
        "replacement must not be published while the old prompt still holds the host"
    );
    let current = agent
        .sessions
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .expect("session stays mapped to the old host");
    assert_eq!(
        current
            .try_lock_host(&session_id)
            .err()
            .expect("old prompt still owns the host")
            .code,
        ErrorCode::InvalidRequest
    );

    drop(held);
    publish.await.expect("publish");
    assert!(agent
        .sessions
        .lock()
        .await
        .get(&session_id)
        .is_some_and(|live| Arc::ptr_eq(live, &replacement)));
}

/// Signals when its turn starts, then holds it until the run is cancelled.
struct HoldUntilCancel(Arc<tokio::sync::Notify>);

impl ModelProvider for HoldUntilCancel {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("test", "test", "model")
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.0.notify_one();
            request.cancellation.cancelled().await;
            Err(ProviderError::interrupted("cancelled"))
        })
    }
}

/// A published `session/new`-style host under `root` whose turns hold until
/// cancelled, and the notify each turn start signals.
async fn install_held_session(
    agent: &Arc<RhoAcpAgent>,
    root: &std::path::Path,
) -> (rho_sdk::SessionId, Arc<tokio::sync::Notify>) {
    let id = rho_sdk::SessionId::new();
    let started = Arc::new(tokio::sync::Notify::new());
    let host = SessionHost::unsaved_for_tests(
        &agent.startup,
        root,
        id.clone(),
        Arc::new(HoldUntilCancel(Arc::clone(&started))),
    )
    .await;
    agent.install(SessionId::new(id.as_str()), host).await;
    (id, started)
}

fn text_prompt(session_id: &SessionId, text: &str) -> PromptRequest {
    PromptRequest::new(session_id.clone(), vec![AcpContentBlock::from(text)])
}

// Covers: session/close must cancel the running prompt as session/cancel
// would, save that turn so a later session/load finds it, and free the
// session, so a thread closed in Zed or T3 Code stops holding its host.
// Closing it again is not found.
// Owner: ACP agent session map
#[tokio::test]
async fn close_cancels_the_running_prompt_saves_it_and_frees_the_session() {
    let home = tempfile::tempdir().unwrap();
    let agent = test_agent();
    let (id, started) = install_held_session(&agent, home.path()).await;
    let session_id = SessionId::new(id.as_str());

    let prompt_agent = Arc::clone(&agent);
    let request = text_prompt(&session_id, "the codeword is zebra");
    let prompt = tokio::spawn(async move { prompt_agent.prompt(request, &NullPort).await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("the turn should reach the model");

    tokio::time::timeout(
        Duration::from_secs(5),
        agent.close_session(CloseSessionRequest::new(session_id.clone())),
    )
    .await
    .expect("close should cancel the turn instead of waiting it out")
    .expect("close the live session");

    let response = prompt.await.unwrap().expect("cancelled prompt");
    assert_eq!(response.stop_reason, StopReason::Cancelled);
    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), id.as_str())
            .expect("the cancelled turn should be saved");
    assert_eq!(
        histories.display.first(),
        Some(&Message::user_text("the codeword is zebra"))
    );
    let again = agent
        .close_session(CloseSessionRequest::new(session_id))
        .await
        .expect_err("already closed");
    assert_eq!(again.code, ErrorCode::ResourceNotFound);
}

// Covers: a prompt queued behind a cancelled turn (T3 Code steers by cancel
// and resend) when session/close arrives must end cancelled, not start a turn
// on the closing host and hold the close open until that turn ends. A
// settings change queued with it must fail as not found, not report success
// for a host the close tears down.
// Owner: ACP agent session map
#[tokio::test]
async fn prompt_queued_before_close_ends_cancelled_without_a_turn() {
    let home = tempfile::tempdir().unwrap();
    let agent = test_agent();
    let (id, _started) = install_held_session(&agent, home.path()).await;
    let session_id = SessionId::new(id.as_str());
    let live = agent.live_session(&session_id).await.unwrap();
    // A cancelled turn still winding down holds the host.
    live.cancel.begin();
    live.cancel.cancel();
    let held = live.host.lock().await;

    let prompt = agent.prompt(text_prompt(&session_id, "steer"), &NullPort);
    tokio::pin!(prompt);
    assert!(futures_util::poll!(&mut prompt).is_pending());
    let change = agent.set_config_option(SetSessionConfigOptionRequest::new(
        session_id.clone(),
        "mode",
        "plan",
    ));
    tokio::pin!(change);
    assert!(futures_util::poll!(&mut change).is_pending());

    let close_agent = Arc::clone(&agent);
    let close_id = session_id.clone();
    let close = tokio::spawn(async move {
        close_agent
            .close_session(CloseSessionRequest::new(close_id))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while agent.sessions.lock().await.contains_key(&session_id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("close should unpublish the session before waiting for its host");

    drop(held);
    let response = tokio::time::timeout(Duration::from_secs(5), prompt)
        .await
        .expect("the queued prompt must not run a turn")
        .expect("queued prompt");
    assert_eq!(response.stop_reason, StopReason::Cancelled);
    let change = tokio::time::timeout(Duration::from_secs(5), change)
        .await
        .expect("the queued change must not wait on a torn-down host")
        .expect_err("closed session");
    assert_eq!(change.code, ErrorCode::ResourceNotFound);
    close.await.unwrap().expect("close");
    let unsaved =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), id.as_str())
            .expect_err("no turn ran, so nothing was saved");
    assert!(unsaved.is::<NoSessionFound>(), "{unsaved:#}");
}

// Covers: a prompt that already holds the host when session/close arrives,
// but has not started its turn, must start cancelled. Otherwise its fresh
// turn escapes the close's cancel and holds close, same-ID load, and
// shutdown open until it ends.
// Owner: ACP agent session map
#[tokio::test]
async fn prompt_starting_as_close_lands_runs_no_turn() {
    let home = tempfile::tempdir().unwrap();
    let agent = test_agent();
    let (id, _started) = install_held_session(&agent, home.path()).await;
    let session_id = SessionId::new(id.as_str());
    let live = agent.live_session(&session_id).await.unwrap();
    // The prompt has the host and is about to begin its turn.
    let mut held = live.host.lock().await;

    let close_agent = Arc::clone(&agent);
    let close_id = session_id.clone();
    let close = tokio::spawn(async move {
        close_agent
            .close_session(CloseSessionRequest::new(close_id))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !live.cancel.is_closed() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("close should close the gate before waiting for the host");

    let host = held.as_mut().expect("the host is still in its slot");
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        host.prompt(text_prompt(&session_id, "late"), &NullPort, &agent.startup),
    )
    .await
    .expect("the prompt must not run a turn the close cannot cancel")
    .expect("late prompt");
    assert_eq!(response.stop_reason, StopReason::Cancelled);
    drop(held);
    close.await.unwrap().expect("close");
    let unsaved =
        StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), id.as_str())
            .expect_err("no turn ran, so nothing was saved");
    assert!(unsaved.is::<NoSessionFound>(), "{unsaved:#}");
}
