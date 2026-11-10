use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::{
    io::{duplex, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf},
    sync::{oneshot, Notify},
    task::JoinHandle,
};

use super::*;
use crate::{
    login_relay,
    methods::{
        AuthLoginFinishedParams, AuthLoginPromptParams, DetachReason, JobState, LoginId,
        LoginOutcome, LoginPrompt, ModelOptions, ModelRef, PermissionMode, PermissionModeOptions,
        Replay, Seq, SessionDetachedParams, SessionEvent, SessionEventParams, SessionId,
        SessionOptions, StreamId, AUTH_LOGIN_FINISHED, AUTH_LOGIN_PROMPT, CONFIG_CHANGED,
        SESSION_DETACHED, SESSION_EVENT,
    },
    subscription, AuthChooseStoreParams, AuthChooseStoreResponse, ConfigChangedParams,
    ConfigGetResponse, ConfigNamespace, ConfigRevision, CredentialStoreKind, CredentialStoreStatus,
    LoginRelay, ProtocolVersion, Published, Publisher, RequestId, SettingEntry, SettingKey,
    SettingValue, StoreSelectedBy, ValueSource,
};

/// Failure bound for each response a test waits for. The tests synchronize
/// on the responses themselves, never on time.
const RESPONSE_DEADLINE: Duration = Duration::from_secs(10);

fn test_status() -> DaemonStatus {
    DaemonStatus {
        pid: 1,
        version: "test".into(),
        protocol: PROTOCOL_VERSION,
        started_at: 0,
        uptime_secs: 0,
        jobs: 0,
        socket: "/tmp/rho-test.sock".into(),
        credential_store: CredentialStoreStatus {
            store: CredentialStoreKind::File,
            selected_by: StoreSelectedBy::Config,
            unreachable: None,
        },
        upgrade: None,
    }
}

fn test_info() -> ServerInfo {
    ServerInfo {
        daemon: DaemonInfo {
            version: "test".into(),
            pid: 1,
        },
        capabilities: Vec::new(),
    }
}

/// The error every test service answers session requests with.
fn no_such_session() -> RpcError {
    RpcError::new(ErrorCode::SessionNotFound, "no such session")
}

/// The session, job, and credential methods of a test service that hosts no
/// sessions.
macro_rules! hosts_no_sessions {
    () => {
        async fn restart(&self) {}

        async fn new_session(&self, _: SessionNewParams) -> Result<SessionNewResponse, RpcError> {
            Err(no_such_session())
        }

        async fn prompt_session(
            &self,
            _: SessionPromptParams,
        ) -> Result<SessionPromptResponse, RpcError> {
            Err(no_such_session())
        }

        async fn cancel_session(
            &self,
            _: SessionCancelParams,
        ) -> Result<SessionCancelResponse, RpcError> {
            Err(no_such_session())
        }

        async fn steer_session(
            &self,
            _: SessionSteerParams,
        ) -> Result<SessionSteerResponse, RpcError> {
            Err(no_such_session())
        }

        async fn unsteer_session(
            &self,
            _: SessionUnsteerParams,
        ) -> Result<SessionUnsteerResponse, RpcError> {
            Err(no_such_session())
        }

        async fn answer_session(
            &self,
            _: SessionAnswerParams,
        ) -> Result<SessionAnswerResponse, RpcError> {
            Err(no_such_session())
        }

        async fn hold_session(
            &self,
            _: SessionHoldParams,
        ) -> Result<SessionHoldResponse, RpcError> {
            Err(no_such_session())
        }

        async fn session_mcp(&self, _: SessionMcpParams) -> Result<SessionMcpInventory, RpcError> {
            Err(no_such_session())
        }

        async fn configure_session(
            &self,
            _: SessionConfigureParams,
        ) -> Result<SessionConfigureResponse, RpcError> {
            Err(no_such_session())
        }

        async fn list_sessions(
            &self,
            _: SessionListParams,
        ) -> Result<SessionListResponse, RpcError> {
            Ok(SessionListResponse {
                sessions: Vec::new(),
                next_cursor: None,
            })
        }

        async fn rename_session(
            &self,
            _: SessionRenameParams,
        ) -> Result<SessionRenameResponse, RpcError> {
            Err(no_such_session())
        }

        async fn delete_session(
            &self,
            _: SessionDeleteParams,
        ) -> Result<SessionDeleteResponse, RpcError> {
            Err(no_such_session())
        }

        async fn list_jobs(&self) -> JobListResponse {
            JobListResponse { jobs: Vec::new() }
        }

        async fn save_auth_key(&self, _: AuthKeyParams) -> Result<AuthKeyResponse, RpcError> {
            Err(no_credential_store())
        }

        async fn logout_auth(&self, _: AuthLogoutParams) -> Result<AuthLogoutResponse, RpcError> {
            Err(no_credential_store())
        }

        async fn list_auths(&self) -> Result<AuthListResponse, RpcError> {
            Err(no_credential_store())
        }

        async fn cancel_login(
            &self,
            _: AuthLoginCancelParams,
        ) -> Result<AuthLoginCancelResponse, RpcError> {
            Err(no_credential_store())
        }

        async fn choose_store(
            &self,
            _: AuthChooseStoreParams,
        ) -> Result<AuthChooseStoreResponse, RpcError> {
            Err(no_credential_store())
        }
    };
}

/// The error every test service answers credential requests with.
fn no_credential_store() -> RpcError {
    RpcError::new(ErrorCode::CredentialStoreUnavailable, "no credential store")
}

/// The config methods of a test service that keeps no settings.
macro_rules! serves_no_config {
    () => {
        async fn config_schema(&self) -> Result<ConfigSchema, RpcError> {
            Err(no_config())
        }

        async fn register_config(
            &self,
            _: ConfigRegisterParams,
        ) -> Result<ConfigRegisterResponse, RpcError> {
            Err(no_config())
        }

        async fn get_config(&self, _: ConfigGetParams) -> Result<ConfigWatch, RpcError> {
            Err(no_config())
        }

        async fn set_config(&self, _: ConfigSetParams) -> Result<ConfigSetResponse, RpcError> {
            Err(no_config())
        }
    };
}

/// The error every test service without settings answers with.
fn no_config() -> RpcError {
    RpcError::new(ErrorCode::MethodNotFound, "no settings")
}

/// The attach method of a test service that hosts no sessions.
macro_rules! attaches_nothing {
    () => {
        async fn attach_session(
            &self,
            _: SessionAttachParams,
        ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
            Err(no_such_session())
        }
    };
}

/// The login method of a test service that runs no logins.
macro_rules! logs_in_nothing {
    () => {
        async fn login(
            &self,
            _: AuthLoginParams,
        ) -> Result<(AuthLoginResponse, LoginStream), RpcError> {
            Err(no_credential_store())
        }
    };
}

/// What a restart costs a test service nobody uses.
const UNUSED: RestartCost = RestartCost {
    in_progress: 0,
    clients: 0,
};

/// The refusal of a test service nobody uses.
macro_rules! nobody_uses_it {
    () => {
        fn refused_newer(&self, _: ProtocolVersion) -> RestartCost {
            UNUSED
        }
    };
}

/// Answers every request at once, and reports that restarting it costs
/// `restart`.
struct FakeService {
    restart: RestartCost,
}

impl Service for FakeService {
    fn refused_newer(&self, _: ProtocolVersion) -> RestartCost {
        self.restart
    }

    async fn status(&self) -> DaemonStatus {
        test_status()
    }

    async fn stop(&self) {}

    hosts_no_sessions!();
    serves_no_config!();
    attaches_nothing!();
    logs_in_nothing!();
}

/// Panics in `status`, as a handler bug would.
struct PanickingService;

fn buggy_status() -> DaemonStatus {
    panic!("a status handler bug")
}

impl Service for PanickingService {
    nobody_uses_it!();

    async fn status(&self) -> DaemonStatus {
        buggy_status()
    }

    async fn stop(&self) {}

    hosts_no_sessions!();
    serves_no_config!();
    attaches_nothing!();
    logs_in_nothing!();
}

/// `status` answers only once the test releases it. `stop` resolves the
/// connection's shutdown (see [`GatedService::shutdown`]) and returns only
/// after the connection has seen it, so the stop is still in flight when the
/// connection begins to close.
#[derive(Default)]
struct GatedService {
    release_status: Notify,
    stop_requested: Notify,
    shutdown_seen: Notify,
}

impl GatedService {
    /// The `shutdown` future to serve with.
    async fn shutdown(self: Arc<Self>) {
        self.stop_requested.notified().await;
        self.shutdown_seen.notify_one();
    }
}

impl Service for GatedService {
    nobody_uses_it!();

    async fn status(&self) -> DaemonStatus {
        self.release_status.notified().await;
        test_status()
    }

    async fn stop(&self) {
        self.stop_requested.notify_one();
        self.shutdown_seen.notified().await;
    }

    hosts_no_sessions!();
    serves_no_config!();
    attaches_nothing!();
    logs_in_nothing!();
}

fn hello_line(id: i64, protocol: ProtocolVersion) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": HELLO,
        "params": {"protocol": protocol, "client": {"name": "test", "version": "0"}},
    })
    .to_string()
}

fn request_line(id: i64, method: &str) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method}).to_string()
}

fn request_with_params(id: i64, method: &str, params: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// `Ok(())` for a success, `Err(code)` for an error, per response line.
type Outcome = (Option<RequestId>, Result<(), Option<ErrorCode>>);

async fn send_lines<W: AsyncWrite + Unpin>(write: &mut W, lines: &[String]) {
    for line in lines {
        write
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
    }
}

/// The next response's outcome, or `None` once the daemon closes the
/// connection.
async fn next_outcome<R: AsyncRead + Unpin>(reader: &mut MessageReader<R>) -> Option<Outcome> {
    let line = tokio::time::timeout(RESPONSE_DEADLINE, reader.read_line())
        .await
        .expect("the daemon answers or closes within RESPONSE_DEADLINE")
        .unwrap()?;
    let Message::Response(Response { id, outcome }) = Message::parse(&line).unwrap() else {
        panic!("expected a response");
    };
    Some((id, outcome.map(drop).map_err(|error| error.error_code())))
}

/// Sends `lines` to `service` on one connection, closes the client's write
/// half, and collects one outcome per response, in arrival order. Answers the
/// protocol gives in place arrive in read order, but a service call answers
/// whenever it completes, so a case that expects a fixed order makes at most
/// one service call, as its last request.
async fn exchange<Svc: Service + 'static>(service: Svc, lines: &[String]) -> Vec<Outcome> {
    let (client, server) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let info = test_info();
        serve_connection(server, &info, Arc::new(service), std::future::pending()).await
    });
    let (read, mut write) = tokio::io::split(client);
    send_lines(&mut write, lines).await;
    write.shutdown().await.unwrap();

    let mut reader = MessageReader::new(read);
    let mut outcomes = Vec::new();
    while let Some(outcome) = next_outcome(&mut reader).await {
        outcomes.push(outcome);
    }
    server.await.unwrap().unwrap();
    outcomes
}

// Covers: the hello gate (nothing before hello, no second hello, a rejected
// hello leaves the connection unhandshaken), unknown methods, and recoverable
// parse errors that keep the connection open.
// Owner: protocol server dispatch.
#[tokio::test]
async fn dispatch_enforces_the_handshake_and_rejects_unknown_requests() {
    let id = |id| Some(RequestId::Number(id));
    let incompatible = ProtocolVersion::new(PROTOCOL_VERSION.major + 1, 0);
    let cases: Vec<(&str, Vec<String>, Vec<Outcome>)> = vec![
        (
            "request before hello",
            vec![request_line(1, DAEMON_STATUS)],
            vec![(id(1), Err(Some(ErrorCode::HandshakeRequired)))],
        ),
        (
            "unknown method before hello",
            vec![request_line(1, "rho/bogus")],
            vec![(id(1), Err(Some(ErrorCode::HandshakeRequired)))],
        ),
        (
            "unknown method after hello",
            vec![
                hello_line(1, PROTOCOL_VERSION),
                request_line(2, "rho/bogus"),
                request_line(3, DAEMON_STATUS),
            ],
            vec![
                (id(1), Ok(())),
                (id(2), Err(Some(ErrorCode::MethodNotFound))),
                (id(3), Ok(())),
            ],
        ),
        (
            "second hello",
            vec![
                hello_line(1, PROTOCOL_VERSION),
                hello_line(2, PROTOCOL_VERSION),
            ],
            vec![
                (id(1), Ok(())),
                (id(2), Err(Some(ErrorCode::InvalidRequest))),
            ],
        ),
        (
            "incompatible hello stays unhandshaken",
            vec![hello_line(1, incompatible), request_line(2, DAEMON_STATUS)],
            vec![
                (id(1), Err(Some(ErrorCode::VersionMismatch))),
                (id(2), Err(Some(ErrorCode::HandshakeRequired))),
            ],
        ),
        (
            "invalid params",
            vec![request_line(1, HELLO)],
            vec![(id(1), Err(Some(ErrorCode::InvalidParams)))],
        ),
        (
            "a hello of another version is refused for its version, whatever else it holds",
            vec![request_with_params(
                1,
                HELLO,
                serde_json::json!({"protocol": incompatible, "client": {"later_shape": true}}),
            )],
            vec![(id(1), Err(Some(ErrorCode::VersionMismatch)))],
        ),
        (
            "session params are decoded and service errors pass through",
            vec![
                hello_line(1, PROTOCOL_VERSION),
                request_line(2, SESSION_PROMPT),
                request_with_params(
                    3,
                    SESSION_PROMPT,
                    serde_json::json!({
                        "session": "s",
                        "content": [{"type": "text", "text": "hi"}],
                        "environment": {},
                        "later_field": 1,
                    }),
                ),
            ],
            vec![
                (id(1), Ok(())),
                (id(2), Err(Some(ErrorCode::InvalidParams))),
                (id(3), Err(Some(ErrorCode::SessionNotFound))),
            ],
        ),
        (
            "detach answers although nothing is attached",
            vec![
                hello_line(1, PROTOCOL_VERSION),
                request_with_params(2, SESSION_DETACH, serde_json::json!({"session": "s"})),
            ],
            vec![(id(1), Ok(())), (id(2), Ok(()))],
        ),
        (
            "parse errors keep the connection open",
            vec![
                "not json".into(),
                r#"{"jsonrpc":"1.0","id":4,"method":"rho/hello"}"#.into(),
                hello_line(5, PROTOCOL_VERSION),
            ],
            vec![
                (None, Err(Some(ErrorCode::ParseError))),
                (id(4), Err(Some(ErrorCode::InvalidRequest))),
                (id(5), Ok(())),
            ],
        ),
    ];
    for (name, lines, expected) in cases {
        let service = FakeService { restart: UNUSED };
        assert_eq!(exchange(service, &lines).await, expected, "{name}");
    }
}

/// What a daemon whose restart costs `restart` answers the hello of a client
/// that speaks `protocol`.
async fn hello_answer(
    restart: RestartCost,
    protocol: ProtocolVersion,
) -> Result<HelloResponse, VersionMismatch> {
    let (client, server) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let service = Arc::new(FakeService { restart });
        serve_connection(server, &test_info(), service, std::future::pending()).await
    });
    let (read, mut write) = tokio::io::split(client);
    send_lines(&mut write, &[hello_line(1, protocol)]).await;
    write.shutdown().await.unwrap();
    let line = tokio::time::timeout(RESPONSE_DEADLINE, MessageReader::new(read).read_line())
        .await
        .expect("the daemon answers within RESPONSE_DEADLINE")
        .unwrap()
        .expect("an answer to the hello");
    server.await.unwrap().unwrap();
    let Message::Response(Response { outcome, .. }) = Message::parse(&line).unwrap() else {
        panic!("expected a response");
    };
    match outcome {
        Ok(hello) => Ok(serde_json::from_value(hello).unwrap()),
        Err(refusal) => {
            assert_eq!(refusal.error_code(), Some(ErrorCode::VersionMismatch));
            Err(serde_json::from_value(refusal.data.expect("the mismatch")).unwrap())
        }
    }
}

// Covers: the daemon's version-skew policy at the handshake (#1776). A
// client of its protocol version is accepted. A newer client is refused and
// told that restarting the daemon is the fix and what that would cost; an
// older one is told to change itself, with no cost, since no daemon restart
// helps it.
// Owner: protocol server handshake.
#[tokio::test]
async fn hello_accepts_the_daemons_version_and_tells_any_other_which_side_to_change() {
    let newer = ProtocolVersion::new(PROTOCOL_VERSION.major, PROTOCOL_VERSION.minor + 1);
    let older = ProtocolVersion::new(0, 0);
    let in_use = RestartCost {
        in_progress: 2,
        clients: 1,
    };
    let refusal = |client, fix, restart| VersionMismatch {
        daemon: PROTOCOL_VERSION,
        client,
        fix,
        restart,
    };

    assert_eq!(
        (
            hello_answer(in_use, PROTOCOL_VERSION).await,
            hello_answer(in_use, newer).await,
            hello_answer(UNUSED, newer).await,
            hello_answer(in_use, older).await,
        ),
        (
            Ok(HelloResponse {
                protocol: PROTOCOL_VERSION,
                daemon: test_info().daemon,
                capabilities: Vec::new(),
            }),
            Err(refusal(newer, VersionFix::RestartDaemon, Some(in_use))),
            Err(refusal(newer, VersionFix::RestartDaemon, Some(UNUSED))),
            Err(refusal(older, VersionFix::UpdateClient, None)),
        )
    );
}

// Covers: a handler that panics still answers its request, with an internal
// error, and the connection survives, instead of leaving the client waiting.
// Owner: protocol server dispatch.
#[tokio::test]
async fn a_panicking_handler_answers_with_an_internal_error() {
    let id = |id| Some(RequestId::Number(id));
    let lines = [
        hello_line(1, PROTOCOL_VERSION),
        request_line(2, DAEMON_STATUS),
    ];
    assert_eq!(
        exchange(PanickingService, &lines).await,
        vec![
            (id(1), Ok(())),
            (id(2), Err(Some(ErrorCode::InternalError))),
        ]
    );
}

// Covers: a slow request does not hold up a later one on the same
// connection; at shutdown the connection stops reading but still answers
// every request it already read, including the `rho/daemon/stop` that caused
// the shutdown, and then closes on its own.
// Owner: protocol server dispatch.
#[tokio::test]
async fn requests_run_concurrently_and_shutdown_answers_those_in_flight() {
    let id = |id| Some(RequestId::Number(id));
    let service = Arc::new(GatedService::default());
    let (client, server) = duplex(64 * 1024);
    let server = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            let info = test_info();
            let shutdown = Arc::clone(&service).shutdown();
            serve_connection(server, &info, service, shutdown).await
        }
    });
    // The write half stays open, so only the shutdown can close the
    // connection.
    let (read, mut write) = tokio::io::split(client);
    send_lines(
        &mut write,
        &[
            hello_line(1, PROTOCOL_VERSION),
            request_line(2, DAEMON_STATUS),
            request_line(3, DAEMON_STOP),
        ],
    )
    .await;

    let mut reader = MessageReader::new(read);
    assert_eq!(next_outcome(&mut reader).await, Some((id(1), Ok(()))));
    // The stop is answered while the status before it is still waiting.
    assert_eq!(next_outcome(&mut reader).await, Some((id(3), Ok(()))));
    service.release_status.notify_one();
    assert_eq!(next_outcome(&mut reader).await, Some((id(2), Ok(()))));
    assert_eq!(next_outcome(&mut reader).await, None);
    server.await.unwrap().unwrap();
}

fn notice(seq: u64) -> SessionEvent {
    SessionEvent::Notice {
        text: format!("event {seq}"),
    }
}

/// Hosts every session. An attach publishes events 1 to `early` into a
/// subscription of `backlog` events, hands its publisher to the test, and
/// answers. `status` answers once the test releases it.
struct AttachService {
    backlog: usize,
    early: u64,
    publishers: mpsc::UnboundedSender<Publisher>,
    release_status: Notify,
}

impl AttachService {
    fn new(backlog: usize, early: u64) -> (Arc<Self>, mpsc::UnboundedReceiver<Publisher>) {
        let (publishers, receiver) = mpsc::unbounded_channel();
        let service = Self {
            backlog,
            early,
            publishers,
            release_status: Notify::new(),
        };
        (Arc::new(service), receiver)
    }
}

impl Service for AttachService {
    nobody_uses_it!();

    async fn status(&self) -> DaemonStatus {
        self.release_status.notified().await;
        test_status()
    }

    async fn stop(&self) {}

    hosts_no_sessions!();
    serves_no_config!();
    logs_in_nothing!();

    async fn attach_session(
        &self,
        _: SessionAttachParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        let (publisher, subscription) = subscription(self.backlog, Vec::new());
        for seq in 1..=self.early {
            // A case with `early` over `backlog` lags here on purpose.
            let _ = publisher.try_publish(Seq(seq), notice(seq));
        }
        self.publishers.send(publisher).unwrap();
        let response = SessionAttachResponse {
            stream: StreamId::new("stream"),
            workspace: "/work".into(),
            replay: Replay::Reset,
            replay_events: 0,
            state: JobState::Idle,
            inputs: Vec::new(),
            title: None,
            options: SessionOptions {
                model: ModelOptions {
                    current: ModelRef {
                        provider: "test".into(),
                        model: "model".into(),
                    },
                    available: Vec::new(),
                },
                reasoning: None,
                fast_mode: None,
                permission_mode: PermissionModeOptions {
                    current: PermissionMode::Bypass,
                    available: Vec::new(),
                },
                commands: Vec::new(),
                goal: None,
            },
        };
        Ok((response, subscription))
    }
}

/// One message the daemon sent, reduced to what the attach tests compare.
#[derive(Debug, PartialEq)]
enum Seen {
    Response(Outcome),
    Event(SessionEventParams),
    Detached(SessionDetachedParams),
    LoginPrompt(AuthLoginPromptParams),
    LoginFinished(AuthLoginFinishedParams),
    ConfigChanged(ConfigChangedParams),
}

fn answered(id: i64) -> Seen {
    Seen::Response((Some(RequestId::Number(id)), Ok(())))
}

fn event(seq: u64) -> Seen {
    Seen::Event(SessionEventParams {
        session: SessionId::new("s"),
        seq: Seq(seq),
        event: notice(seq),
    })
}

fn detached(reason: DetachReason) -> Seen {
    Seen::Detached(SessionDetachedParams {
        session: SessionId::new("s"),
        reason,
    })
}

/// A request naming session `s`, with the empty client environment
/// `rho/session/attach` requires; other methods ignore it.
fn session_line(id: i64, method: &str) -> String {
    request_with_params(
        id,
        method,
        serde_json::json!({"session": "s", "environment": {}}),
    )
}

/// The client end of one served connection.
struct Peer {
    reader: MessageReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    server: JoinHandle<Result<(), FrameError>>,
}

impl Peer {
    fn connect<Svc: Service + 'static>(service: Arc<Svc>) -> Self {
        let (client, server) = duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let info = test_info();
            serve_connection(server, &info, service, std::future::pending()).await
        });
        let (read, writer) = tokio::io::split(client);
        Self {
            reader: MessageReader::new(read),
            writer,
            server,
        }
    }

    async fn send(&mut self, line: String) {
        send_lines(&mut self.writer, &[line]).await;
    }

    /// The next message, or `None` once the daemon closes.
    async fn read(&mut self) -> Option<Seen> {
        let line = tokio::time::timeout(RESPONSE_DEADLINE, self.reader.read_line())
            .await
            .expect("the daemon sends or closes within RESPONSE_DEADLINE")
            .unwrap()?;
        Some(match Message::parse(&line).unwrap() {
            Message::Response(Response { id, outcome }) => {
                Seen::Response((id, outcome.map(drop).map_err(|error| error.error_code())))
            }
            Message::Notification(jsonrpc::Notification { method, params }) => {
                let params = params.expect("notification params");
                match method.as_str() {
                    SESSION_EVENT => Seen::Event(serde_json::from_value(params).unwrap()),
                    SESSION_DETACHED => Seen::Detached(serde_json::from_value(params).unwrap()),
                    AUTH_LOGIN_PROMPT => Seen::LoginPrompt(serde_json::from_value(params).unwrap()),
                    AUTH_LOGIN_FINISHED => {
                        Seen::LoginFinished(serde_json::from_value(params).unwrap())
                    }
                    CONFIG_CHANGED => Seen::ConfigChanged(serde_json::from_value(params).unwrap()),
                    other => panic!("unexpected notification {other}"),
                }
            }
            Message::Request(request) => panic!("unexpected request {request:?}"),
        })
    }

    /// The next `count` messages, failing the test if the daemon closes
    /// first.
    async fn take(&mut self, count: usize) -> Vec<Seen> {
        let mut seen = Vec::new();
        for _ in 0..count {
            seen.push(self.read().await.expect("the daemon sends before closing"));
        }
        seen
    }

    /// Stops sending, as a client that leaves does.
    async fn close_input(&mut self) {
        self.writer.shutdown().await.unwrap();
    }

    /// Every message until the daemon closes; call after [`Self::close_input`].
    async fn remaining(mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        while let Some(message) = self.read().await {
            seen.push(message);
        }
        self.server.await.unwrap().unwrap();
        seen
    }
}

/// What the test does with the attach's publisher once it has it.
enum Then {
    Publish(u64),
    Drop,
    Close(DetachReason),
}

// Covers: an attach's response precedes every event, even ones published
// before the service answered; events arrive in seq order; a subscription
// that lags, or a publisher that closes with a reason, ends with
// `rho/session/detached` carrying that reason after the queued events.
// Owner: protocol server forwarding.
#[tokio::test]
async fn attach_answers_then_forwards_events_and_the_detach_reason() {
    let cases = [
        (
            "events follow the response in seq order",
            8,
            2,
            Then::Publish(3),
            vec![answered(1), answered(2), event(1), event(2), event(3)],
        ),
        (
            "a full subscription detaches the client as lagged",
            2,
            3,
            Then::Drop,
            vec![
                answered(1),
                answered(2),
                event(1),
                event(2),
                detached(DetachReason::Lagged),
            ],
        ),
        (
            "a closed publisher detaches the client with its reason",
            8,
            1,
            Then::Close(DetachReason::DaemonStopping),
            vec![
                answered(1),
                answered(2),
                event(1),
                detached(DetachReason::DaemonStopping),
            ],
        ),
    ];
    for (name, backlog, early, then, expected) in cases {
        let (service, mut publishers) = AttachService::new(backlog, early);
        let mut peer = Peer::connect(service);
        peer.send(hello_line(1, PROTOCOL_VERSION)).await;
        peer.send(session_line(2, SESSION_ATTACH)).await;
        let publisher = publishers.recv().await.unwrap();
        match then {
            Then::Publish(seq) => assert_eq!(publisher.try_publish(Seq(seq), notice(seq)), Ok(())),
            Then::Drop => drop(publisher),
            Then::Close(reason) => publisher.close(reason),
        }

        assert_eq!(peer.take(expected.len()).await, expected, "{name}");
        peer.close_input().await;
        assert_eq!(peer.remaining().await, Vec::new(), "{name}");
    }
}

// Covers: attaching a session again stops the earlier forwarder, and
// `rho/session/detach` stops the current one, so no event of a replaced or
// detached subscription reaches the client after the response that ended it.
// Owner: protocol server forwarding.
#[tokio::test]
async fn reattach_replaces_and_detach_stops_the_forwarder() {
    let (service, mut publishers) = AttachService::new(8, 0);
    let mut peer = Peer::connect(service);
    peer.send(hello_line(1, PROTOCOL_VERSION)).await;
    peer.send(session_line(2, SESSION_ATTACH)).await;
    assert_eq!(peer.take(2).await, vec![answered(1), answered(2)]);
    let replaced = publishers.recv().await.unwrap();
    peer.send(session_line(3, SESSION_ATTACH)).await;
    assert_eq!(peer.take(1).await, vec![answered(3)]);
    let current = publishers.recv().await.unwrap();

    // The replaced forwarder may not have dropped its subscription yet, so
    // this can succeed; it must still never be sent.
    let _ = replaced.try_publish(Seq(1), notice(101));
    assert_eq!(current.try_publish(Seq(1), notice(1)), Ok(()));
    assert_eq!(peer.take(1).await, vec![event(1)]);

    peer.send(session_line(4, SESSION_DETACH)).await;
    assert_eq!(peer.take(1).await, vec![answered(4)]);
    let _ = current.try_publish(Seq(2), notice(2));
    peer.close_input().await;
    assert_eq!(peer.remaining().await, Vec::new());
}

// Covers: when the client leaves, the connection drops its subscriptions at
// once (the publisher sees it gone) while a call already read still answers,
// and a live subscription does not keep the connection open.
// Owner: protocol server forwarding.
#[tokio::test]
async fn leaving_stops_forwarders_while_calls_in_flight_still_answer() {
    let (service, mut publishers) = AttachService::new(8, 0);
    let mut peer = Peer::connect(Arc::clone(&service));
    peer.send(hello_line(1, PROTOCOL_VERSION)).await;
    peer.send(session_line(2, SESSION_ATTACH)).await;
    assert_eq!(peer.take(2).await, vec![answered(1), answered(2)]);
    let publisher = publishers.recv().await.unwrap();

    peer.send(request_line(3, DAEMON_STATUS)).await;
    peer.close_input().await;
    tokio::time::timeout(RESPONSE_DEADLINE, publisher.closed())
        .await
        .expect("the connection drops the subscription within RESPONSE_DEADLINE");
    assert_eq!(
        publisher.try_publish(Seq(1), notice(1)),
        Err(Published::Gone)
    );
    service.release_status.notify_one();
    assert_eq!(peer.remaining().await, vec![answered(3)]);
}

// Covers: as the connection ends, a forwarder still sends what its publisher
// already published and the reason of a publisher already closed, so a
// service that closes its publishers before ending connections tells each
// client why it was detached.
// Owner: protocol server forwarding.
#[tokio::test]
async fn ending_sends_what_was_published_and_why_the_publisher_closed() {
    let (service, mut publishers) = AttachService::new(8, 0);
    let mut peer = Peer::connect(service);
    peer.send(hello_line(1, PROTOCOL_VERSION)).await;
    peer.send(session_line(2, SESSION_ATTACH)).await;
    assert_eq!(peer.take(2).await, vec![answered(1), answered(2)]);
    let publisher = publishers.recv().await.unwrap();

    assert_eq!(publisher.try_publish(Seq(1), notice(1)), Ok(()));
    publisher.close(DetachReason::DaemonStopping);
    peer.close_input().await;
    assert_eq!(
        peer.remaining().await,
        vec![event(1), detached(DetachReason::DaemonStopping)]
    );
}

// Covers: a closing connection whose peer stopped reading, with its socket
// and notification queue full, ends with `Stalled` once a write has waited
// CLOSE_GRACE, instead of holding the daemon's stop open forever.
// Owner: protocol server forwarding.
#[tokio::test(start_paused = true)]
async fn a_peer_that_stops_reading_cannot_hold_a_closing_connection_open() {
    let events = 4 * crate::forward::NOTIFICATION_QUEUE as u64;
    let (service, mut publishers) = AttachService::new(events as usize, 0);
    let (client, server) = duplex(1024);
    let (stop, stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let info = test_info();
        let shutdown = async {
            let _ = stopped.await;
        };
        serve_connection(server, &info, service, shutdown).await
    });
    // The read half stays open and unread, as a suspended client's does.
    let (_unread, mut writer) = tokio::io::split(client);
    send_lines(
        &mut writer,
        &[
            hello_line(1, PROTOCOL_VERSION),
            session_line(2, SESSION_ATTACH),
        ],
    )
    .await;
    let publisher = publishers.recv().await.unwrap();
    for seq in 1..=events {
        assert_eq!(publisher.try_publish(Seq(seq), notice(seq)), Ok(()));
    }
    publisher.close(DetachReason::DaemonStopping);

    stop.send(()).unwrap();
    let ended = tokio::time::timeout(RESPONSE_DEADLINE, server)
        .await
        .expect("the closing connection ends within RESPONSE_DEADLINE")
        .unwrap();
    assert!(
        matches!(ended, Err(FrameError::Stalled { grace }) if grace == CLOSE_GRACE),
        "{ended:?}"
    );
}

/// Starts each login with a relay that has already sent its prompt and, with
/// `finish_early`, its end, before the service answers; hands the relay of
/// a login it did not finish to the test.
struct LoginService {
    finish_early: bool,
    relays: mpsc::UnboundedSender<LoginRelay>,
}

fn login_prompt() -> LoginPrompt {
    LoginPrompt {
        url: "https://example.test/device".into(),
        user_code: Some("CODE".into()),
        url_with_code: None,
        instruction: "Enter the code.".into(),
    }
}

impl Service for LoginService {
    nobody_uses_it!();

    async fn status(&self) -> DaemonStatus {
        test_status()
    }

    async fn stop(&self) {}

    hosts_no_sessions!();
    serves_no_config!();
    attaches_nothing!();

    async fn login(
        &self,
        _: AuthLoginParams,
    ) -> Result<(AuthLoginResponse, LoginStream), RpcError> {
        let (relay, stream) = login_relay();
        relay.prompt(login_prompt());
        if self.finish_early {
            relay.finish(LoginOutcome::Succeeded);
        } else {
            self.relays.send(relay).unwrap();
        }
        let response = AuthLoginResponse {
            login: LoginId::new("l1"),
        };
        Ok((response, stream))
    }
}

fn login_line(id: i64) -> String {
    request_with_params(id, AUTH_LOGIN, serde_json::json!({"auth": "xai-oauth"}))
}

// Covers: a login's response precedes its prompt and end even when the
// login sent both before the service answered, and the end is the login's
// last message (#1662).
// Owner: protocol server forwarding.
#[tokio::test]
async fn a_login_answers_then_forwards_its_prompt_and_end() {
    let (relays, _) = mpsc::unbounded_channel();
    let service = Arc::new(LoginService {
        finish_early: true,
        relays,
    });
    let mut peer = Peer::connect(service);
    peer.send(hello_line(1, PROTOCOL_VERSION)).await;
    peer.send(login_line(2)).await;

    assert_eq!(
        peer.take(4).await,
        vec![
            answered(1),
            answered(2),
            Seen::LoginPrompt(AuthLoginPromptParams {
                login: LoginId::new("l1"),
                prompt: login_prompt(),
            }),
            Seen::LoginFinished(AuthLoginFinishedParams {
                login: LoginId::new("l1"),
                outcome: LoginOutcome::Succeeded,
            }),
        ]
    );
    peer.close_input().await;
    assert_eq!(peer.remaining().await, Vec::new());
}

// Covers: when the client that started a login leaves, the connection
// drops the login's stream, so the login sees its client gone and the
// daemon can cancel it (#1662).
// Owner: protocol server forwarding.
#[tokio::test]
async fn leaving_tells_a_login_its_client_is_gone() {
    let (relays, mut started) = mpsc::unbounded_channel();
    let service = Arc::new(LoginService {
        finish_early: false,
        relays,
    });
    let mut peer = Peer::connect(service);
    peer.send(hello_line(1, PROTOCOL_VERSION)).await;
    peer.send(login_line(2)).await;
    let relay = started.recv().await.unwrap();
    assert_eq!(
        peer.take(3).await,
        vec![
            answered(1),
            answered(2),
            Seen::LoginPrompt(AuthLoginPromptParams {
                login: LoginId::new("l1"),
                prompt: login_prompt(),
            }),
        ]
    );

    peer.close_input().await;
    tokio::time::timeout(RESPONSE_DEADLINE, relay.closed())
        .await
        .expect("the connection drops the login's stream within RESPONSE_DEADLINE");
    assert_eq!(peer.remaining().await, Vec::new());
}

/// Serves one settings namespace whose state the test publishes.
struct ConfigService {
    state: tokio::sync::watch::Sender<Arc<ConfigGetResponse>>,
}

impl Service for ConfigService {
    async fn status(&self) -> DaemonStatus {
        test_status()
    }

    async fn stop(&self) {}

    hosts_no_sessions!();
    attaches_nothing!();
    logs_in_nothing!();
    nobody_uses_it!();

    async fn config_schema(&self) -> Result<ConfigSchema, RpcError> {
        Err(no_config())
    }

    async fn register_config(
        &self,
        _: ConfigRegisterParams,
    ) -> Result<ConfigRegisterResponse, RpcError> {
        Err(no_config())
    }

    async fn get_config(&self, _: ConfigGetParams) -> Result<ConfigWatch, RpcError> {
        Ok(ConfigWatch::new(self.state.subscribe()))
    }

    async fn set_config(&self, _: ConfigSetParams) -> Result<ConfigSetResponse, RpcError> {
        Err(no_config())
    }
}

/// A state whose `entries` are bool settings, at `revision`.
fn config_state(revision: &str, entries: &[(&str, bool)]) -> Arc<ConfigGetResponse> {
    Arc::new(ConfigGetResponse {
        revision: ConfigRevision::new(revision),
        values: entries
            .iter()
            .map(|(key, value)| (SettingKey::new(*key), global(*value)))
            .collect(),
        errors: Vec::new(),
    })
}

fn global(value: bool) -> SettingEntry {
    SettingEntry {
        value: Some(SettingValue::Bool(value)),
        source: ValueSource::Global,
    }
}

fn config_get_line(id: i64) -> String {
    request_with_params(
        id,
        CONFIG_GET,
        serde_json::json!({"namespace": {"kind": "agent"}}),
    )
}

// Covers: `rho/config/get` answers, then sends `rho/config/changed` with
// every entry that changed since the last notification, even when two
// states were published before the forwarder ran; reading the namespace
// again replaces the forwarder instead of adding a second one.
// Owner: protocol server config forwarding.
#[tokio::test]
async fn config_get_answers_then_forwards_every_changed_entry() {
    let service = Arc::new(ConfigService {
        state: tokio::sync::watch::Sender::new(config_state("r0", &[("a", false), ("b", false)])),
    });
    let mut peer = Peer::connect(Arc::clone(&service));
    peer.send(hello_line(1, PROTOCOL_VERSION)).await;
    peer.send(config_get_line(2)).await;
    assert_eq!(peer.take(2).await, [answered(1), answered(2)]);

    // Two writes before the forwarder wakes: it sees only the second state,
    // and must still report `a`.
    service
        .state
        .send_replace(config_state("r1", &[("a", true), ("b", false)]));
    service
        .state
        .send_replace(config_state("r2", &[("a", true), ("b", true)]));
    let changed = |revision: &str, entries: &[(&str, bool)]| {
        Seen::ConfigChanged(ConfigChangedParams {
            namespace: ConfigNamespace::Agent,
            revision: ConfigRevision::new(revision),
            values: entries
                .iter()
                .map(|(key, value)| (SettingKey::new(*key), global(*value)))
                .collect(),
            errors: Vec::new(),
        })
    };
    assert_eq!(
        peer.take(1).await,
        [changed("r2", &[("a", true), ("b", true)])]
    );

    peer.send(config_get_line(3)).await;
    assert_eq!(peer.take(1).await, [answered(3)]);
    service
        .state
        .send_replace(config_state("r3", &[("a", false), ("b", true)]));
    assert_eq!(peer.take(1).await, [changed("r3", &[("a", false)])]);
    peer.close_input().await;
    assert_eq!(peer.remaining().await, []);
}
