use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{duplex, AsyncWriteExt, ReadHalf, WriteHalf};

use super::*;
use crate::{
    methods::{
        DetachReason, JobList, Seq, SessionEvent, SessionId, Status, DAEMON_STATUS, JOB_LIST,
        SESSION_DETACHED, SESSION_EVENT,
    },
    ClientInfo, CredentialStoreKind, CredentialStoreStatus, DaemonStatus, JobListParams,
    JobListResponse, ProtocolVersion, RestartCost, StatusParams, VersionFix,
};

fn client_hello() -> ClientHello {
    ClientHello::frontend(ClientInfo {
        name: "test".into(),
        version: "0".into(),
    })
}

fn fake_status() -> DaemonStatus {
    DaemonStatus {
        pid: 1,
        version: "test".into(),
        protocol: PROTOCOL_VERSION,
        started_at: 0,
        uptime_secs: 0,
        jobs: 0,
        socket: "/tmp/rho-test.sock".into(),
        credential_store: CredentialStoreStatus {
            store: CredentialStoreKind::Os,
            selected_by: crate::StoreSelectedBy::Default,
            unreachable: Some("no session D-Bus".into()),
        },
        upgrade: None,
    }
}

type DaemonReader = MessageReader<ReadHalf<tokio::io::DuplexStream>>;
type DaemonWriter = WriteHalf<tokio::io::DuplexStream>;

async fn write_lines(write: &mut DaemonWriter, lines: &[serde_json::Value]) {
    for line in lines {
        write
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
    }
}

async fn read_request(reader: &mut DaemonReader) -> Request {
    let line = reader.read_line().await.unwrap().expect("a request");
    match Message::parse(&line).unwrap() {
        Message::Request(request) => request,
        other => panic!("expected a request, got {other:?}"),
    }
}

/// Reads the hello and accepts it, as a compatible daemon does.
async fn accept_hello(reader: &mut DaemonReader, write: &mut DaemonWriter) {
    let hello = read_request(reader).await;
    write_lines(
        write,
        &[json!({
            "jsonrpc": "2.0",
            "id": hello.id,
            "result": {
                "protocol": PROTOCOL_VERSION,
                "daemon": {"version": "test", "pid": 1},
                "capabilities": [],
            },
        })],
    )
    .await;
}

// Covers: the wire form of a version mismatch (a rejected hello's error data,
// or an accepted hello naming an incompatible version) becomes a typed error
// that says which side to fix and, from an older daemon, what restarting it
// would cost.
// Owner: protocol client handshake.
#[tokio::test]
async fn handshake_maps_incompatible_daemons_to_version_mismatch() {
    let newer = ProtocolVersion::new(PROTOCOL_VERSION.major + 1, 0);
    let older = ProtocolVersion::new(0, 0);
    let mismatch = |daemon, fix, restart| VersionMismatch {
        daemon,
        client: PROTOCOL_VERSION,
        fix,
        restart,
    };
    let in_use = RestartCost {
        in_progress: 2,
        clients: 1,
    };
    let cases = [
        (
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {
                    "code": -32001,
                    "message": "incompatible",
                    "data": {
                        "daemon": newer,
                        "client": PROTOCOL_VERSION,
                        "fix": "update_client",
                    },
                },
            }),
            mismatch(newer, VersionFix::UpdateClient, None),
        ),
        (
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {
                    "code": -32001,
                    "message": "incompatible",
                    "data": {
                        "daemon": older,
                        "client": PROTOCOL_VERSION,
                        "fix": "restart_daemon",
                        "restart": {"in_progress": 2, "clients": 1},
                    },
                },
            }),
            mismatch(older, VersionFix::RestartDaemon, Some(in_use)),
        ),
        (
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocol": newer,
                    "daemon": {"version": "9.0.0", "pid": 1},
                    "capabilities": [],
                },
            }),
            mismatch(newer, VersionFix::UpdateClient, None),
        ),
    ];
    for (reply, expected) in cases {
        let (client, server) = duplex(64 * 1024);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut reader = MessageReader::new(read);
            reader.read_line().await.unwrap().expect("hello");
            write_lines(&mut write, &[reply]).await;
            // Hold the stream open until the client has read the reply.
            reader.read_line().await.unwrap();
        });

        let error = Client::handshake(client, client_hello())
            .await
            .err()
            .expect("mismatch");

        match error {
            ClientError::VersionMismatch(mismatch) => assert_eq!(mismatch, expected),
            other => panic!("expected a version mismatch, got {other:?}"),
        }
        daemon.await.unwrap();
    }
}

// Covers: overlapping requests each get their own response although the
// daemon answers them in reverse order, notifications sent between the
// responses reach `Notifications` in order, and the stream ends once the
// connection closes.
// Owner: protocol client routing.
#[tokio::test]
async fn overlapping_requests_pair_by_id_and_notifications_interleave() {
    let (client, server) = duplex(64 * 1024);
    let daemon = tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(server);
        let mut reader = MessageReader::new(read);
        accept_hello(&mut reader, &mut write).await;
        // Both requests are pending before either is answered.
        let first = read_request(&mut reader).await;
        let second = read_request(&mut reader).await;
        let answer = |request: &Request| {
            let result = match request.method.as_str() {
                DAEMON_STATUS => serde_json::to_value(fake_status()).unwrap(),
                JOB_LIST => json!({"jobs": []}),
                other => panic!("unexpected request {other}"),
            };
            json!({"jsonrpc": "2.0", "id": request.id, "result": result})
        };
        write_lines(
            &mut write,
            &[
                json!({"jsonrpc": "2.0", "method": SESSION_EVENT, "params": {
                    "session": "s", "seq": 1, "event": {"type": "notice", "text": "hi"},
                }}),
                answer(&second),
                json!({"jsonrpc": "2.0", "method": SESSION_DETACHED, "params": {
                    "session": "s", "reason": "lagged",
                }}),
                json!({"jsonrpc": "2.0", "method": "rho/later/thing", "params": {}}),
                answer(&first),
            ],
        )
        .await;
        // Hold the stream open until the client leaves.
        reader.read_line().await.unwrap();
    });
    let (client, mut notifications, _) = Client::handshake(client, client_hello()).await.unwrap();

    let (status, jobs) = tokio::join!(
        client.request::<Status>(&StatusParams {}),
        client.request::<JobList>(&JobListParams {}),
    );

    assert_eq!(status.unwrap(), fake_status());
    assert_eq!(jobs.unwrap(), JobListResponse { jobs: Vec::new() });
    drop(client);
    let mut received = Vec::new();
    while let Some(notification) = notifications.recv().await {
        received.push(notification);
    }
    assert_eq!(
        received,
        vec![
            DaemonNotification::SessionEvent(SessionEventParams {
                session: SessionId::new("s"),
                seq: Seq(1),
                event: SessionEvent::Notice { text: "hi".into() },
            }),
            DaemonNotification::SessionDetached(SessionDetachedParams {
                session: SessionId::new("s"),
                reason: DetachReason::Lagged,
            }),
            DaemonNotification::Unknown {
                method: "rho/later/thing".into(),
            },
        ]
    );
    daemon.await.unwrap();
}

/// How a request failed, reduced to what the tests compare.
#[derive(Debug, PartialEq)]
enum Failure {
    Closed,
    Rpc(Option<ErrorCode>),
    Invalid,
}

// Covers: a pending request fails, rather than waiting forever, when the
// daemon closes the connection, rejects a line it could not read (a `null`
// id names no request), or sends a line this client cannot read.
// Owner: protocol client routing.
#[tokio::test]
async fn pending_requests_fail_when_the_daemon_cannot_answer_them() {
    let cases = [
        ("the daemon closes the connection", None, Failure::Closed),
        (
            "the daemon rejects an unreadable line",
            Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": "invalid JSON"},
            })),
            Failure::Rpc(Some(ErrorCode::ParseError)),
        ),
        (
            "the daemon sends a notification with the wrong params",
            Some(json!({"jsonrpc": "2.0", "method": SESSION_EVENT, "params": {"session": 1}})),
            Failure::Invalid,
        ),
    ];
    for (name, reply, expected) in cases {
        let (client, server) = duplex(64 * 1024);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut reader = MessageReader::new(read);
            accept_hello(&mut reader, &mut write).await;
            read_request(&mut reader).await;
            let Some(reply) = reply else {
                return;
            };
            write_lines(&mut write, &[reply]).await;
            // Hold the stream open until the client leaves.
            reader.read_line().await.unwrap();
        });
        let (client, _notifications, _) = Client::handshake(client, client_hello()).await.unwrap();

        let failure = match client.request::<Status>(&StatusParams {}).await {
            Err(ClientError::Closed) => Failure::Closed,
            Err(ClientError::Rpc { error, .. }) => Failure::Rpc(error.error_code()),
            Err(ClientError::InvalidMessage(_)) => Failure::Invalid,
            other => panic!("{name}: unexpected outcome {other:?}"),
        };

        assert_eq!(failure, expected, "{name}");
        drop(client);
        daemon.await.unwrap();
    }
}
