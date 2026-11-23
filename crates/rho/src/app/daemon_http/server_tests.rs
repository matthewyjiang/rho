use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use pretty_assertions::assert_eq;
use rho_protocol::{
    frame_channel, methods, BuildId, Client, ClientEnvironment, ClientHello, ClientInfo,
    CredentialStoreKind, CredentialStoreStatus, DaemonInfo, DaemonNotification, DaemonStatus,
    DetachReason, JobListResponse, JobState, ModelChoice, ModelOptions, ModelRef, PermissionMode,
    PermissionModeOptions, ProtocolVersion, Replay, RestartCost, RpcError, Seq, Service,
    SessionAttachParams, SessionAttachResponse, SessionEvent, SessionId, SessionNewParams,
    SessionNewResponse, SessionOptions, SettingsService, StatusParams, StoreSelectedBy, StreamId,
    Subscription, PROTOCOL_VERSION,
};
use tokio::{io::DuplexStream, net::TcpListener, sync::oneshot};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header, HeaderValue, StatusCode},
        protocol::{frame::coding::CloseCode, CloseFrame},
        Error as WsError, Message as WsMessage,
    },
};

use super::{accept_until, BoxFuture, DaemonConnector, ServerState, TokenLoader};
use crate::app::{
    daemon_http::tokens::DaemonTokenSet,
    http_guard::{AllowedHost, AllowedOrigin},
};

fn test_options() -> SessionOptions {
    let model = ModelRef {
        provider: "test".to_owned(),
        model: "model-1".to_owned(),
    };
    SessionOptions {
        model: ModelOptions {
            current: model.clone(),
            auth: "test-api-key".to_owned(),
            available: vec![ModelChoice {
                model,
                label: "model-1".to_owned(),
            }],
        },
        reasoning: None,
        fast_mode: None,
        permission_mode: PermissionModeOptions {
            current: PermissionMode::Supervised,
            available: vec![PermissionMode::Supervised],
        },
        commands: Vec::new(),
        added_dirs: Vec::new(),
    }
}

struct FakeDaemon {
    last_env: Mutex<Option<BTreeMap<String, String>>>,
    events_tx: Mutex<Option<rho_protocol::Publisher>>,
    /// When set, `rho/session/new` reports it started on `entered`, then
    /// answers only once `release` fires.
    held_session_new: Mutex<Option<HeldCall>>,
}

struct HeldCall {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

impl FakeDaemon {
    fn new() -> Self {
        Self {
            last_env: Mutex::new(None),
            events_tx: Mutex::new(None),
            held_session_new: Mutex::new(None),
        }
    }
}

impl SettingsService for FakeDaemon {}

impl Service for FakeDaemon {
    fn refused_newer(&self, _client: ProtocolVersion) -> RestartCost {
        RestartCost {
            in_progress: 0,
            clients: 0,
        }
    }

    fn skewed(&self, _client: &ClientInfo, _build: &BuildId) -> RestartCost {
        RestartCost {
            in_progress: 0,
            clients: 0,
        }
    }

    async fn status(&self) -> DaemonStatus {
        DaemonStatus {
            pid: 4242,
            version: "3.3.0-test".to_owned(),
            protocol: PROTOCOL_VERSION,
            started_at: 1_700_000_000,
            uptime_secs: 12,
            jobs: 0,
            in_progress: 0,
            socket: PathBuf::from("/tmp/fake.sock"),
            credential_store: CredentialStoreStatus {
                store: CredentialStoreKind::File,
                selected_by: StoreSelectedBy::Config,
                unreachable: None,
            },
            upgrade: None,
        }
    }

    async fn stop(&self) {}
    async fn restart(&self) {}

    async fn list_jobs(&self) -> JobListResponse {
        JobListResponse { jobs: Vec::new() }
    }

    async fn new_session(&self, params: SessionNewParams) -> Result<SessionNewResponse, RpcError> {
        let held = self.held_session_new.lock().unwrap().take();
        if let Some(HeldCall { entered, release }) = held {
            let _ = entered.send(());
            let _ = release.await;
        }
        let env: BTreeMap<String, String> = params.environment.into_iter().collect();
        *self.last_env.lock().unwrap() = Some(env);
        Ok(SessionNewResponse {
            session: SessionId::new("sess-1"),
            options: test_options(),
            worktree: None,
        })
    }

    async fn attach_session(
        &self,
        _params: SessionAttachParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        let (publisher, subscription) = rho_protocol::subscription(16, Vec::new());
        let _ = publisher.try_publish(
            Seq(1),
            SessionEvent::Notice {
                text: "hello over ws".to_owned(),
            },
        );
        *self.events_tx.lock().unwrap() = Some(publisher);
        Ok((
            SessionAttachResponse {
                stream: StreamId::new("stream-1"),
                workspace: PathBuf::from("/tmp"),
                replay: Replay::Tail,
                replay_events: 0,
                state: JobState::Idle,
                turn: None,
                turn_elapsed_ms: None,
                inputs: Vec::new(),
                background: Vec::new(),
                title: None,
                options: test_options(),
            },
            subscription,
        ))
    }
}

struct InMemoryConnector {
    service: Arc<FakeDaemon>,
}

impl DaemonConnector for InMemoryConnector {
    type Stream = DuplexStream;

    fn connect(&self) -> BoxFuture<anyhow::Result<Self::Stream>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            let (client_side, server_side) = tokio::io::duplex(64 * 1024);
            let info = DaemonInfo {
                version: "3.3.0-test".to_owned(),
                pid: 4242,
                build: BuildId::of_this_crate(),
            };
            tokio::spawn(async move {
                let _ = rho_protocol::serve_connection(
                    server_side,
                    &info,
                    service,
                    std::future::pending::<()>(),
                )
                .await;
            });
            Ok(client_side)
        })
    }
}

struct SharedTokenLoader {
    tokens: Arc<Mutex<DaemonTokenSet>>,
}

impl TokenLoader for SharedTokenLoader {
    fn load(&self) -> BoxFuture<anyhow::Result<DaemonTokenSet>> {
        let snapshot = self.tokens.lock().unwrap().clone();
        Box::pin(async move { Ok(snapshot) })
    }
}

struct TestHarness {
    addr: SocketAddr,
    secret: String,
    daemon: Arc<FakeDaemon>,
    tokens: Arc<Mutex<DaemonTokenSet>>,
    shutdown: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl TestHarness {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut set = DaemonTokenSet::default();
        let secret = set.rotate("test-client").unwrap();
        let tokens = Arc::new(Mutex::new(set));
        let daemon = Arc::new(FakeDaemon::new());
        let (shutdown, stopped) = oneshot::channel::<()>();
        let host_env = ClientEnvironment::from_iter([
            ("PATH".to_owned(), "/host/bin".to_owned()),
            ("HOST_ONLY".to_owned(), "yes".to_owned()),
        ]);
        let state = ServerState::new(
            ["localhost", "127.0.0.1", "::1", "tunnel.example.com"]
                .into_iter()
                .map(|host| AllowedHost::parse(host).unwrap())
                .collect(),
            vec![AllowedOrigin::parse("https://ui.example.com").unwrap()],
            host_env,
            InMemoryConnector {
                service: Arc::clone(&daemon),
            },
            SharedTokenLoader {
                tokens: Arc::clone(&tokens),
            },
        );
        let server = tokio::spawn(async move {
            accept_until(&listener, &state, async {
                let _ = stopped.await;
            })
            .await
        });
        Self {
            addr,
            secret,
            daemon,
            tokens,
            shutdown,
            server,
        }
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        self.server.await.unwrap().unwrap();
    }
}

/// A WebSocket request to `addr` carrying bearer `secret`.
fn authorized_request(
    addr: SocketAddr,
    secret: &str,
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut request = format!("ws://{addr}/ws").into_client_request().unwrap();
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {secret}")).unwrap(),
    );
    request
}

/// Connects a `rho_protocol::Client` over WebSocket `request`. The returned
/// task ends with the close frame the server sent, if any.
async fn connect_client(
    request: tokio_tungstenite::tungstenite::handshake::client::Request,
) -> (
    Client,
    rho_protocol::Notifications,
    tokio::task::JoinHandle<Option<CloseFrame>>,
) {
    let (ws, _) = connect_async(request).await.unwrap();
    let (mut ws_sink, mut ws_stream) = ws.split();
    let (frame_stream, incoming, mut outgoing) = frame_channel();
    let pump_in = tokio::spawn(async move {
        while let Some(Ok(message)) = ws_stream.next().await {
            match message {
                WsMessage::Text(text) => {
                    if incoming.send(text.to_string()).is_err() {
                        return None;
                    }
                }
                WsMessage::Close(frame) => return frame,
                _ => {}
            }
        }
        None
    });
    tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            if ws_sink.send(WsMessage::Text(frame.into())).await.is_err() {
                break;
            }
        }
        let _ = ws_sink.close().await;
    });
    let hello = ClientHello::frontend(ClientInfo {
        name: "ws-test".to_owned(),
        version: "0.1.0".to_owned(),
    });
    let (client, notifications, hello) = Client::handshake(frame_stream, hello).await.unwrap();
    assert_eq!(hello.daemon.pid, 4242);
    (client, notifications, pump_in)
}

fn session_new_params(environment: ClientEnvironment) -> SessionNewParams {
    SessionNewParams {
        cwd: PathBuf::from("/tmp"),
        environment,
        agent: None,
        setup: Default::default(),
        parent: None,
        grant: None,
        worktree: false,
    }
}

// Covers: HTTP guard checks enforce path (404), Host (403), Origin (403),
// Bearer auth (401 + WWW-Authenticate), method (405), and Upgrade (426), and
// token revocation applies on the next upgrade without restart.
// Owner: daemon_http::server.
#[tokio::test]
async fn http_guards_enforce_auth_host_origin_and_upgrade_rules() {
    let harness = TestHarness::start().await;
    let client = crate::reqwest_client();
    let base = format!("http://{}", harness.addr);

    // 404 on wrong path.
    let resp = client.get(format!("{base}/other")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // 401 without bearer token.
    let resp = client.get(format!("{base}/ws")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer")
    );

    // 401 with invalid bearer token.
    let resp = client
        .get(format!("{base}/ws"))
        .header(header::AUTHORIZATION, "Bearer wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 403 on disallowed Host (DNS rebinding).
    let resp = client
        .get(format!("{base}/ws"))
        .header(header::HOST, "evil.example.com")
        .header(header::AUTHORIZATION, format!("Bearer {}", harness.secret))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 403 on disallowed Origin (CSWSH).
    let resp = client
        .get(format!("{base}/ws"))
        .header(header::ORIGIN, "https://evil.example.com")
        .header(header::AUTHORIZATION, format!("Bearer {}", harness.secret))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 405 on POST to /ws with valid token.
    let resp = client
        .post(format!("{base}/ws"))
        .header(header::AUTHORIZATION, format!("Bearer {}", harness.secret))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);

    // 426 on plain GET without WebSocket upgrade headers.
    let resp = client
        .get(format!("{base}/ws"))
        .header(header::AUTHORIZATION, format!("Bearer {}", harness.secret))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UPGRADE_REQUIRED);

    // Revoking the token immediately rejects the next WebSocket upgrade.
    harness.tokens.lock().unwrap().revoke("test-client");
    let mut request = format!("ws://{}/ws", harness.addr)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", harness.secret)).unwrap(),
    );
    let err = connect_async(request).await.unwrap_err();
    match err {
        WsError::Http(resp) => assert_eq!(resp.status(), StatusCode::UNAUTHORIZED),
        other => panic!("expected 401 HTTP refusal, got {other:?}"),
    }

    harness.stop().await;
}

// Covers: a real rho_protocol::Client completes rho/hello, calls RPCs (with
// host environment overlay), and receives live rho/session/event notifications
// over WebSocket via rho_protocol::frame_channel().
// Owner: daemon_http server + bridge + rho-protocol frame_stream.
#[tokio::test]
async fn rho_protocol_client_runs_rpc_and_notifications_over_websocket() {
    let harness = TestHarness::start().await;
    let mut request = authorized_request(harness.addr, &harness.secret);
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://ui.example.com"),
    );
    let (client, mut notifications, pump_in) = connect_client(request).await;

    let status = client
        .request::<methods::Status>(&StatusParams {})
        .await
        .unwrap();
    assert_eq!(status.pid, 4242);

    // Empty/partial client environment inherits host_env with client overrides.
    let created = client
        .request::<methods::SessionNew>(&session_new_params(ClientEnvironment::from_iter([(
            "CLIENT_VAR".to_owned(),
            "42".to_owned(),
        )])))
        .await
        .unwrap();
    assert_eq!(created.session, SessionId::new("sess-1"));
    assert_eq!(
        harness.daemon.last_env.lock().unwrap().clone(),
        Some(BTreeMap::from([
            ("CLIENT_VAR".to_owned(), "42".to_owned()),
            ("HOST_ONLY".to_owned(), "yes".to_owned()),
            ("PATH".to_owned(), "/host/bin".to_owned()),
        ]))
    );

    // Attach and receive streamed notification + detach reason on close.
    let attached = client
        .request::<methods::SessionAttach>(&SessionAttachParams {
            session: SessionId::new("sess-1"),
            after: None,
            environment: ClientEnvironment::default(),
            setup: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(attached.stream, StreamId::new("stream-1"));

    let note = notifications.recv().await.unwrap();
    match note {
        DaemonNotification::SessionEvent(params) => {
            assert_eq!(
                params.event,
                SessionEvent::Notice {
                    text: "hello over ws".to_owned()
                }
            );
        }
        other => panic!("expected SessionEvent, got {other:?}"),
    }

    if let Some(publisher) = harness.daemon.events_tx.lock().unwrap().take() {
        publisher.close(DetachReason::DaemonStopping);
    }
    let detached = notifications.recv().await.unwrap();
    match detached {
        DaemonNotification::SessionDetached(params) => {
            assert_eq!(params.reason, DetachReason::DaemonStopping);
        }
        other => panic!("expected SessionDetached, got {other:?}"),
    }

    drop(client);
    let _ = pump_in.await;
    harness.stop().await;
}

// Covers: non-Rust clients sending binary WebSocket frames are closed cleanly
// with CloseCode::Unsupported (1003).
// Owner: daemon_http::bridge.
#[tokio::test]
async fn binary_websocket_frames_are_refused_with_unsupported() {
    let harness = TestHarness::start().await;
    let mut request = format!("ws://{}/ws", harness.addr)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", harness.secret)).unwrap(),
    );

    let (mut ws, _) = connect_async(request).await.unwrap();
    ws.send(WsMessage::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();

    let closed = loop {
        match ws.next().await {
            Some(Ok(WsMessage::Close(frame))) => break frame,
            Some(Ok(_)) => continue,
            other => panic!("expected Close frame, got {other:?}"),
        }
    };
    assert_eq!(closed.map(|frame| frame.code), Some(CloseCode::Unsupported));

    harness.stop().await;
}

// Covers: an in-place upgrade stops accepting and hands back the listening
// socket, and each open WebSocket gets the daemon's answer to a request
// already under way before it closes with `Away`; the drain ends once it has.
// Owner: daemon_http::server in-place upgrade + bridge close ordering.
#[tokio::test]
async fn upgrade_delivers_answers_under_way_then_closes_with_away() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut set = DaemonTokenSet::default();
    let secret = set.rotate("upgrade-test").unwrap();
    let daemon = Arc::new(FakeDaemon::new());
    let (entered_tx, entered) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    *daemon.held_session_new.lock().unwrap() = Some(HeldCall {
        entered: entered_tx,
        release: release_rx,
    });
    let state = ServerState::new(
        vec![AllowedHost::parse("127.0.0.1").unwrap()],
        Vec::new(),
        ClientEnvironment::default(),
        InMemoryConnector { service: daemon },
        SharedTokenLoader {
            tokens: Arc::new(Mutex::new(set)),
        },
    );
    let (upgrade_tx, replaced) = oneshot::channel::<String>();
    let server = tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            let version = accept_until(&listener, &state, async { replaced.await.unwrap() })
                .await
                .unwrap();
            (version, listener)
        }
    });

    let (client, _notifications, closed) = connect_client(authorized_request(addr, &secret)).await;
    let created = tokio::spawn(async move {
        client
            .request::<methods::SessionNew>(&session_new_params(ClientEnvironment::default()))
            .await
    });
    entered.await.unwrap();

    upgrade_tx.send("9.9.9".to_owned()).unwrap();
    let (version, listener) = server.await.unwrap();
    assert_eq!(version, "9.9.9");
    assert_eq!(listener.local_addr().unwrap(), addr);

    let settle = state.drain.settle(Duration::from_secs(30));
    tokio::pin!(settle);
    // The first poll begins the drain, which waits on the open WebSocket.
    assert!(futures_util::poll!(&mut settle).is_pending());
    release.send(()).unwrap();

    assert_eq!(
        created.await.unwrap().unwrap().session,
        SessionId::new("sess-1")
    );
    assert_eq!(
        closed.await.unwrap().map(|frame| frame.code),
        Some(CloseCode::Away)
    );
    assert_eq!(settle.await, 0);
}
