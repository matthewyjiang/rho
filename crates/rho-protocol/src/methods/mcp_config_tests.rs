use std::fmt;

use pretty_assertions::assert_eq;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;

use super::*;
use crate::{SecretItem, SyncItem};

/// Asserts `value` encodes as `wire` and `wire` decodes back to `value`.
fn round_trips<T>(value: T, wire: serde_json::Value)
where
    T: Serialize + DeserializeOwned + PartialEq + fmt::Debug,
{
    assert_eq!(serde_json::to_value(&value).unwrap(), wire);
    assert_eq!(serde_json::from_value::<T>(wire).unwrap(), value);
}

// Covers: a server entry, a set's changes and secret sources, and the MCP
// secret and journal items travel in their wire form, tagged by `op` or
// `type`, with an absent `error`, `sealed`, `sync`, and empty `secrets` left
// out. Reading unknown values is `schema_tests`'s.
// Owner: protocol wire contract.
#[test]
fn mcp_config_messages_travel_in_their_wire_form() {
    let header = McpSecretSlot::new(McpSecretKind::Header, "Authorization");
    round_trips(
        McpServerEntry::new(
            "docs".into(),
            json!({"transport": "streamable_http", "url": "https://docs.example"}),
            vec![header.clone()],
            None,
        ),
        json!({
            "identity": "docs",
            "value": {"transport": "streamable_http", "url": "https://docs.example"},
            "secrets": [{"kind": "header", "name": "Authorization"}],
        }),
    );
    round_trips(
        McpConfigSetParams::new(
            ConfigRevision::new("r1"),
            vec![
                McpServerChange::Put {
                    identity: "docs".into(),
                    value: json!({"transport": "stdio", "command": "docs"}),
                    secrets: vec![
                        McpSecret::new(header, SecretSource::Sealed),
                        McpSecret::new(
                            McpSecretSlot::new(McpSecretKind::Env, "TOKEN"),
                            SecretSource::Keep,
                        ),
                    ],
                },
                McpServerChange::Put {
                    identity: "bare".into(),
                    value: json!({"transport": "stdio", "command": "bare"}),
                    secrets: Vec::new(),
                },
                McpServerChange::Remove {
                    identity: "old".into(),
                },
            ],
        )
        .with_sync(SyncId::new("s")),
        json!({
            "revision": "r1",
            "changes": [
                {"op": "put", "identity": "docs", "value": {"transport": "stdio", "command": "docs"}, "secrets": [
                    {"slot": {"kind": "header", "name": "Authorization"}, "source": "sealed"},
                    {"slot": {"kind": "env", "name": "TOKEN"}, "source": "keep"},
                ]},
                {"op": "put", "identity": "bare", "value": {"transport": "stdio", "command": "bare"}},
                {"op": "remove", "identity": "old"},
            ],
            "sync": "s",
        }),
    );
    round_trips(
        vec![
            SecretItem::mcp_header("docs", "Authorization"),
            SecretItem::mcp_env("docs", "TOKEN"),
        ],
        json!([
            {"type": "mcp_header", "server": "docs", "name": "Authorization"},
            {"type": "mcp_env", "server": "docs", "name": "TOKEN"},
        ]),
    );
    round_trips(
        SyncItem::mcp_server("docs"),
        json!({"type": "mcp_server", "identity": "docs"}),
    );
}
