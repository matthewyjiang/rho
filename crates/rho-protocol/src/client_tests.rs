use pretty_assertions::assert_eq;
use tokio::io::{duplex, AsyncWriteExt};

use super::*;
use crate::{ProtocolVersion, VersionFix};

// Covers: the wire form of a version mismatch (a rejected hello's error data,
// or an accepted hello naming another major) becomes a typed error that says
// which side to fix.
// Owner: protocol client handshake.
#[tokio::test]
async fn handshake_maps_incompatible_daemons_to_version_mismatch() {
    let newer = ProtocolVersion::new(PROTOCOL_VERSION.major + 1, 0);
    let cases = [
        (
            serde_json::json!({
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
            VersionFix::UpdateClient,
        ),
        (
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocol": newer,
                    "daemon": {"version": "9.0.0", "pid": 1},
                    "capabilities": [],
                },
            }),
            VersionFix::UpdateClient,
        ),
    ];
    for (reply, fix) in cases {
        let (client, server) = duplex(64 * 1024);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut reader = MessageReader::new(read);
            reader.read_line().await.unwrap().expect("hello");
            write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
            // Hold the stream open until the client has read the reply.
            reader.read_line().await.unwrap();
        });
        let info = ClientInfo {
            name: "test".into(),
            version: "0".into(),
        };

        let error = Client::handshake(client, info)
            .await
            .err()
            .expect("mismatch");

        match error {
            ClientError::VersionMismatch(mismatch) => assert_eq!(
                mismatch,
                VersionMismatch {
                    daemon: newer,
                    client: PROTOCOL_VERSION,
                    fix,
                }
            ),
            other => panic!("expected a version mismatch, got {other:?}"),
        }
        daemon.await.unwrap();
    }
}
