use std::collections::HashMap;

use pretty_assertions::assert_eq;
use rho_protocol::{
    McpSecret, McpSecretKind, McpSecretSlot, McpServerChange, McpServerEntry, SecretSource,
};
use serde_json::{json, Value};

use super::*;
use crate::config::ConfigDocument;

const FILE: &str = r#"# Mine.
[mcp.servers.docs]
transport = "streamable_http"
url = "https://docs.example/mcp"
headers = { Authorization = "Bearer old", "X-Team" = "core" }
headers_from_env = { "X-Trace" = "TRACE_ID" }

[mcp.servers.tool]
transport = "stdio"
command = "tool"
env = { TOKEN = "t0k" }
"#;

fn header(name: &str) -> McpSecretSlot {
    McpSecretSlot::new(McpSecretKind::Header, name)
}

fn put(identity: &str, value: Value, secrets: Vec<McpSecret>) -> McpServerChange {
    McpServerChange::Put {
        identity: identity.into(),
        value,
        secrets,
    }
}

fn docs_value() -> Value {
    json!({"transport": "streamable_http", "url": "https://docs.example/mcp"})
}

// Covers: a read leaves out every header and env value, naming each slot,
// keeps fields that only name variables, and reports a server the loader
// refuses with its value still shown.
// Owner: config MCP servers.
#[test]
fn a_read_names_secrets_without_their_values() {
    let doc = ConfigDocument::parse(&format!(
        "{FILE}\n[mcp.servers.broken]\ntransport = \"stdio\"\n"
    ))
    .unwrap();
    let entries = mcp_server_entries(&doc);
    assert_eq!(
        entries[1..],
        [
            McpServerEntry::new(
                "docs".into(),
                json!({
                    "transport": "streamable_http",
                    "url": "https://docs.example/mcp",
                    "headers_from_env": {"X-Trace": "TRACE_ID"},
                }),
                vec![header("Authorization"), header("X-Team")],
                None,
            ),
            McpServerEntry::new(
                "tool".into(),
                json!({"transport": "stdio", "command": "tool"}),
                vec![McpSecretSlot::new(McpSecretKind::Env, "TOKEN")],
                None,
            ),
        ]
    );
    assert_eq!(entries[0].identity, "broken");
    assert_eq!(entries[0].value, json!({"transport": "stdio"}));
    assert!(entries[0].error.is_some());
}

// Covers: a put rebuilds the secret maps from the values the file held
// (`keep`) and from the sealed bundle (`sealed`), drops a slot it leaves
// out, and keeps the rest of the file; restoring the entry the file held
// before puts it back as it was.
// Owner: config MCP servers.
#[test]
fn a_put_fills_secrets_from_the_file_and_the_bundle() {
    let mut doc = ConfigDocument::parse(FILE).unwrap();
    let before = mcp_server(&doc, "docs");
    let sealed = HashMap::from([(("docs".to_owned(), header("Authorization")), "Bearer new")]);
    apply_mcp_changes(
        &mut doc,
        &[put(
            "docs",
            docs_value(),
            vec![
                McpSecret::new(header("Authorization"), SecretSource::Sealed),
                McpSecret::new(header("X-Team"), SecretSource::Keep),
            ],
        )],
        &sealed,
    )
    .unwrap();
    assert_eq!(
        mcp_secret(&doc, "docs", &header("Authorization")).as_deref(),
        Some("Bearer new")
    );
    assert_eq!(
        mcp_secret(&doc, "docs", &header("X-Team")).as_deref(),
        Some("core")
    );
    assert_eq!(
        mcp_server_entries(&doc)[0].value,
        docs_value(),
        "a put drops `headers_from_env` it leaves out"
    );
    assert!(doc.text().starts_with("# Mine.\n"));

    restore_mcp_servers(&mut doc, &[("docs".to_owned(), before.clone())]).unwrap();
    assert_eq!(mcp_server(&doc, "docs"), before);
}

// Covers: a put that carries secret maps in its value, keeps a slot the
// file holds no value for, or names a sealed slot the bundle lacks, and a
// remove of a server the file lacks, are each refused by name, and no
// change of the batch lands.
// Owner: config MCP servers.
#[test]
fn a_change_it_cannot_fill_refuses_the_whole_batch() {
    let refused = [
        put(
            "docs",
            json!({"transport": "stdio", "command": "x", "env": {"A": "b"}}),
            vec![],
        ),
        put(
            "docs",
            docs_value(),
            vec![McpSecret::new(header("Missing"), SecretSource::Keep)],
        ),
        put(
            "docs",
            docs_value(),
            vec![McpSecret::new(
                header("Authorization"),
                SecretSource::Sealed,
            )],
        ),
        McpServerChange::Remove {
            identity: "gone".into(),
        },
    ];
    for change in refused {
        let mut doc = ConfigDocument::parse(FILE).unwrap();
        let batch = [
            McpServerChange::Remove {
                identity: "tool".into(),
            },
            change.clone(),
        ];
        let result = apply_mcp_changes(&mut doc, &batch, &HashMap::new());
        assert!(
            matches!(&result, Err(ChangeError::Refused(errors)) if errors.len() == 1),
            "{change:?}: {result:?}"
        );
        assert_eq!(doc.text(), FILE, "{change:?}");
    }

    // A server whose put failed is not tried again by a later change.
    let mut doc = ConfigDocument::parse(FILE).unwrap();
    let batch = [
        put("fresh", json!({"transport": "stdio"}), vec![]),
        McpServerChange::Remove {
            identity: "fresh".into(),
        },
    ];
    let result = apply_mcp_changes(&mut doc, &batch, &HashMap::new());
    assert!(
        matches!(&result, Err(ChangeError::Refused(errors)) if errors.len() == 1),
        "{result:?}"
    );
}

// Covers: the revision covers secret values, so changing only a header
// moves it.
// Owner: config MCP servers.
#[test]
fn the_revision_moves_with_a_secret_value() {
    let key = [7_u8; 32];
    let doc = ConfigDocument::parse(FILE).unwrap();
    let changed = ConfigDocument::parse(&FILE.replace("Bearer old", "Bearer older")).unwrap();
    assert_ne!(
        mcp_servers_revision(&doc, &key),
        mcp_servers_revision(&changed, &key)
    );
}
