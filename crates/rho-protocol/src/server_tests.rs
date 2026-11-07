use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::{
    io::{duplex, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf},
    sync::{oneshot, Notify},
    task::JoinHandle,
};

use super::*;
use crate::{
    methods::{
        DetachReason, JobState, Replay, Seq, SessionDetachedParams, SessionEvent,
        SessionEventParams, SessionId, StreamId, SESSION_DETACHED, SESSION_EVENT,
    },
    subscription, ProtocolVersion, Published, Publisher, RequestId,
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
    RpcError::new(ErrorCode::SessionNotLoaded, "no such session")
}

/// The session and job methods of a test service that hosts no sessions.
macro_rules! hosts_no_sessions {
    () => {
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

        async fn answer_session(
            &self,
            _: SessionAnswerParams,
        ) -> Result<SessionAnswerResponse, RpcError> {
            Err(no_such_session())
        }

        async fn list_jobs(&self) -> JobListResponse {
            JobListResponse { jobs: Vec::new() }
        }
    };
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

/// Answers every request at once.
struct FakeService;

impl Service for FakeService {
    async fn status(&self) -> DaemonStatus {
        test_status()
    }

    async fn stop(&self) {}

    hosts_no_sessions!();
    attaches_nothing!();
}

/// Panics in `status`, as a handler bug would.
struct PanickingService;

fn buggy_status() -> DaemonStatus {
    panic!("a status handler bug")
}

impl Service for PanickingService {
    async fn status(&self) -> DaemonStatus {
        buggy_status()
    }

    async fn stop(&self) {}

    hosts_no_sessions!();
    attaches_nothing!();
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
    async fn status(&self) -> DaemonStatus {
        self.release_status.notified().await;
        test_status()
    }

    async fn stop(&self) {
        self.stop_requested.notify_one();
        self.shutdown_seen.notified().await;
    }

    hosts_no_sessions!();
    attaches_nothing!();
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
            "session params are decoded and service errors pass through",
            vec![
                hello_line(1, PROTOCOL_VERSION),
                request_line(2, SESSION_PROMPT),
                request_with_params(
                    3,
                    SESSION_PROMPT,
                    serde_json::json!({"session": "s", "text": "hi", "later_field": 1}),
                ),
            ],
            vec![
                (id(1), Ok(())),
                (id(2), Err(Some(ErrorCode::InvalidParams))),
                (id(3), Err(Some(ErrorCode::SessionNotLoaded))),
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
        assert_eq!(exchange(FakeService, &lines).await, expected, "{name}");
    }
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
    async fn status(&self) -> DaemonStatus {
        self.release_status.notified().await;
        test_status()
    }

    async fn stop(&self) {}

    hosts_no_sessions!();

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
            replay: Replay::Reset,
            state: JobState::Idle,
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

fn session_line(id: i64, method: &str) -> String {
    request_with_params(id, method, serde_json::json!({"session": "s"}))
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
