use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{duplex, AsyncWriteExt};

use super::*;

// Covers: every message shape survives write -> read -> parse unchanged, so
// ids, absent params, null ids, and error data reach the peer intact.
// Owner: protocol framing.
#[tokio::test]
async fn messages_round_trip_through_framing() {
    let messages = [
        Message::Request(Request {
            id: RequestId::Number(7),
            method: "rho/daemon/status".into(),
            params: Some(json!({})),
        }),
        Message::Request(Request {
            id: RequestId::String("abc".into()),
            method: "rho/hello".into(),
            params: None,
        }),
        Message::Notification(Notification {
            method: "rho/event".into(),
            params: Some(json!(["line\nbreak", 1])),
        }),
        Message::Response(Response {
            id: Some(RequestId::Number(7)),
            outcome: Ok(json!({"pid": 42})),
        }),
        Message::Response(Response {
            id: Some(RequestId::Number(8)),
            outcome: Err(RpcError::new(ErrorCode::VersionMismatch, "mismatch")
                .with_data(json!({"fix": "update_client"}))),
        }),
        Message::Response(Response {
            id: None,
            outcome: Err(RpcError::new(ErrorCode::ParseError, "bad json")),
        }),
    ];
    let (client, server) = duplex(64 * 1024);
    let mut writer = MessageWriter::new(client);
    let mut reader = MessageReader::new(server);

    for message in &messages {
        writer.write(message).await.unwrap();
    }
    drop(writer);

    let mut received = Vec::new();
    while let Some(line) = reader.read_line().await.unwrap() {
        received.push(Message::parse(&line).unwrap());
    }
    assert_eq!(received, messages);
}

// Covers: a line at the limit is accepted and one byte more is refused with an
// error naming the limit, before the reader buffers an unbounded line.
// Owner: protocol framing.
#[tokio::test]
async fn read_line_enforces_max_line_bytes() {
    for (length, expect_ok) in [(MAX_LINE_BYTES, true), (MAX_LINE_BYTES + 1, false)] {
        let (mut client, server) = duplex(64 * 1024);
        let writer = tokio::spawn(async move {
            let mut line = vec![b'a'; length];
            line.push(b'\n');
            // The reader stops early on an oversized line; a broken pipe is fine.
            let _ = client.write_all(&line).await;
        });
        let mut reader = MessageReader::new(server);

        match reader.read_line().await {
            Ok(Some(line)) => assert!(expect_ok && line.len() == length, "length {length}"),
            Err(FrameError::LineTooLong { limit, observed }) => {
                assert!(!expect_ok, "length {length} was rejected");
                assert_eq!(limit, MAX_LINE_BYTES);
                assert!(observed > MAX_LINE_BYTES, "observed {observed}");
            }
            other => panic!("length {length}: unexpected {other:?}"),
        }
        drop(reader);
        writer.await.unwrap();
    }
}
