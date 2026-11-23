use pretty_assertions::assert_eq;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::frame_channel;

// Covers: one incoming text frame (including pretty-printed JSON with embedded
// newlines) becomes one NDJSON line, and writes split on `\n` into exact
// text frames without trailing newlines.
// Owner: rho-protocol frame_stream adapter.
#[tokio::test]
async fn frames_round_trip_as_single_lines_without_smuggling() {
    let (stream, sender, mut receiver) = frame_channel();
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    // Compact frame and pretty-printed frame both read as single lines.
    assert_eq!(
        sender.send(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
        Ok(())
    );
    assert_eq!(
        sender.send("{\n  \"jsonrpc\": \"2.0\",\n  \"id\": 2,\n  \"result\": {}\n}\n"),
        Ok(())
    );
    // Invalid JSON containing a newline must stay one line (no smuggling).
    assert_eq!(
        sender.send("not json\n{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{}}"),
        Ok(())
    );

    assert_eq!(
        lines.next_line().await.unwrap().as_deref(),
        Some(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)
    );
    let compacted = lines.next_line().await.unwrap().unwrap();
    assert!(!compacted.contains('\n'));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&compacted).unwrap(),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {}})
    );
    assert_eq!(
        lines.next_line().await.unwrap().as_deref(),
        Some(r#"not json {"jsonrpc":"2.0","id":3,"result":{}}"#)
    );

    // Partial writes buffer until `\n`, then emit one frame per line.
    writer.write_all(b"{\"a\":1}").await.unwrap();
    writer.write_all(b"\n{\"b\":2}\r\n").await.unwrap();
    assert_eq!(receiver.recv().await.as_deref(), Some(r#"{"a":1}"#));
    assert_eq!(receiver.recv().await.as_deref(), Some(r#"{"b":2}"#));

    // Shutting down the write side closes both the outgoing receiver and reads.
    writer.shutdown().await.unwrap();
    assert_eq!(receiver.recv().await, None);
    assert_eq!(lines.next_line().await.unwrap(), None);
}
