use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::{
    io::{duplex, AsyncWriteExt},
    sync::Notify,
};

use super::*;
use crate::{ProtocolVersion, RequestId};

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

/// Answers every request at once.
struct FakeService;

impl Service for FakeService {
    async fn status(&self) -> DaemonStatus {
        test_status()
    }

    async fn stop(&self) {}
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
