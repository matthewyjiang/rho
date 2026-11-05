use pretty_assertions::assert_eq;
use tokio::io::{duplex, AsyncWriteExt};

use super::*;
use crate::{ProtocolVersion, RequestId};

struct FakeService;

impl Service for FakeService {
    fn status(&self) -> DaemonStatus {
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

    fn stop(&self) {}
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

/// Sends `lines` on one connection and collects one outcome per response.
async fn exchange(lines: &[String]) -> Vec<Outcome> {
    let (client, server) = duplex(64 * 1024);
    let info = ServerInfo {
        daemon: DaemonInfo {
            version: "test".into(),
            pid: 1,
        },
        capabilities: Vec::new(),
    };
    let service = FakeService;
    let server = tokio::spawn(async move {
        serve_connection(server, &info, &service, std::future::pending()).await
    });
    let (read, mut write) = tokio::io::split(client);
    for line in lines {
        write
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
    }
    write.shutdown().await.unwrap();

    let mut reader = MessageReader::new(read);
    let mut outcomes = Vec::new();
    while let Some(line) = reader.read_line().await.unwrap() {
        let Message::Response(Response { id, outcome }) = Message::parse(&line).unwrap() else {
            panic!("expected a response");
        };
        outcomes.push((id, outcome.map(drop).map_err(|error| error.error_code())));
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
        assert_eq!(exchange(&lines).await, expected, "{name}");
    }
}
