use pretty_assertions::assert_eq;
use serde_json::json;

use rho_tools::tool_card::{ToolFact, ToolHeader};

use super::{argument_summary, mcp_header_and_facts, primary_argument};
use crate::tools::mcp::exported_name::{parse_exported_name, ExportedNameDialect, McpToolIdentity};

// Covers: plain exported names decode to server + tool; non-MCP names and
// malformed shapes are rejected rather than half-parsed.
// Owner: mcp display helper
#[test]
fn parse_exported_name_splits_server_and_tool() {
    let cases: &[(&str, Option<(&str, &str)>)] = &[
        ("mcp__filesystem__read", Some(("filesystem", "read"))),
        // First `__` after the prefix splits; the rest stays in the tool name.
        ("mcp__srv__a__b", Some(("srv", "a__b"))),
        ("bash", None),
        ("mcp__only", None),
        ("mcp____tool", None),
        ("mcp__srv__", None),
    ];
    for (name, expected) in cases {
        let expected = expected.map(|(server, tool)| McpToolIdentity {
            server: server.into(),
            tool: tool.into(),
        });
        assert_eq!(
            parse_exported_name(name, ExportedNameDialect::Rho),
            expected,
            "name: {name}"
        );
    }
}

// Covers: `_rho_` hex escapes from namespaced_tool_name round-trip back to the
// original component; invalid hex is shown verbatim instead of guessed.
// Owner: mcp display helper
#[test]
fn parse_exported_name_decodes_rho_escapes() {
    // From mcp_tests: server "git-hub", tool "issues/list".
    let parsed = parse_exported_name(
        "mcp___rho_6769742d687562___rho_6973737565732f6c697374",
        ExportedNameDialect::Rho,
    )
    .unwrap();
    assert_eq!(
        parsed,
        McpToolIdentity {
            server: "git-hub".into(),
            tool: "issues/list".into(),
        }
    );

    // Odd-length / non-hex payloads stay as-is.
    let parsed = parse_exported_name("mcp___rho_zz__tool", ExportedNameDialect::Rho).unwrap();
    assert_eq!(parsed.server, "_rho_zz");
    assert_eq!(parsed.tool, "tool");
}

// Covers: priority keys win over other strings, single-string fallback,
// multiline values keep only their first line.
// Owner: mcp display helper
#[test]
fn primary_argument_prefers_known_keys_then_single_string() {
    let args = json!({"note": "misc", "path": "crates/rho", "count": 3});
    assert_eq!(
        primary_argument(&args),
        Some(("path".into(), "crates/rho".into()))
    );

    let args = json!({"target": "src/main.rs", "count": 3});
    assert_eq!(
        primary_argument(&args),
        Some(("target".into(), "src/main.rs".into()))
    );

    // Two unknown strings: ambiguous, no primary.
    let args = json!({"a": "x", "b": "y"});
    assert_eq!(primary_argument(&args), None);

    let args = json!({"prompt": "first line\nsecond line"});
    assert_eq!(
        primary_argument(&args),
        Some(("prompt".into(), "first line…".into()))
    );

    assert_eq!(primary_argument(&json!({})), None);
    assert_eq!(primary_argument(&json!("not an object")), None);
}

// Covers: scalars render as key value pairs in call order, promoted key is
// skipped, multiline strings are omitted, containers collapse.
// Owner: mcp display helper
#[test]
fn argument_summary_joins_scalars_and_skips_primary() {
    let args = json!({
        "path": "crates",
        "output_mode": "files_with_matches",
        "max_results": 50,
        "literal": true,
        "body": "line one\nline two",
        "extra": {"nested": 1},
        "items": [1, 2],
    });
    // serde_json preserve_order keeps the model's argument order.
    assert_eq!(
        argument_summary(&args, Some("path")).unwrap(),
        "output_mode files_with_matches · max_results 50 · literal true · extra {…} · items […]"
    );
    assert_eq!(argument_summary(&json!({}), None), None);
    assert_eq!(
        argument_summary(&json!({"body": "a\nb"}), None),
        None,
        "only a multiline string leaves nothing to summarize"
    );
}

// Covers: the shared grammar constructor both producers consume — decoded
// verb + promoted primary in the header, provenance fact first, summary fact
// second, and rejection of non-MCP names.
// Owner: mcp display helper
#[test]
fn mcp_header_and_facts_assembles_shared_grammar() {
    let args = json!({"path": "crates", "max_results": 50});
    let (header, facts) =
        mcp_header_and_facts("mcp__olive__grep", Some(&args), ExportedNameDialect::Rho).unwrap();
    assert_eq!(header, ToolHeader::call("grep", Some("crates".into())));
    assert_eq!(
        facts,
        vec![
            ToolFact::Meta {
                text: "mcp · olive".into()
            },
            ToolFact::Text {
                text: "max_results 50".into()
            },
        ]
    );

    // No arguments: header stays bare, provenance still present.
    let (header, facts) =
        mcp_header_and_facts("mcp__olive__grep", None, ExportedNameDialect::Rho).unwrap();
    assert_eq!(header, ToolHeader::call("grep", None));
    assert_eq!(
        facts,
        vec![ToolFact::Meta {
            text: "mcp · olive".into()
        }]
    );

    assert_eq!(
        mcp_header_and_facts("bash", None, ExportedNameDialect::Rho),
        None
    );
}

// Covers: multiline primaries stay within the 80-char budget including the
// continuation ellipsis.
// Owner: mcp display helper
#[test]
fn primary_argument_multiline_stays_within_budget() {
    // Exactly at the budget: the continuation ellipsis must not push past it.
    for len in [79, 80, 81] {
        let first_line = "x".repeat(len);
        let args = json!({ "prompt": format!("{first_line}\nmore") });
        let (_, display) = primary_argument(&args).unwrap();
        assert!(
            display.chars().count() <= 80,
            "len {len}: {} chars",
            display.chars().count()
        );
        assert!(display.ends_with('…'), "len {len}");
    }
}

// Covers: untrusted MCP keys, values, and name components flatten control
// characters so headers and facts stay one terminal row.
// Owner: mcp display helper
#[test]
fn display_text_flattens_control_characters() {
    let args = json!({
        "x\ninjected": "ok",
        "note": "a\rb\tc",
        "path": "crates/rho\rpayload",
    });
    assert_eq!(
        primary_argument(&args),
        Some(("path".into(), "crates/rho payload".into()))
    );
    assert_eq!(
        argument_summary(&args, Some("path")).as_deref(),
        Some("x injected ok · note a b c")
    );

    let (header, facts) = mcp_header_and_facts(
        "mcp__olive\rsrv__grep\ttool",
        Some(&args),
        ExportedNameDialect::Rho,
    )
    .unwrap();
    assert_eq!(
        header,
        ToolHeader::call("grep tool", Some("crates/rho payload".into()))
    );
    assert_eq!(
        facts,
        vec![
            ToolFact::Meta {
                text: "mcp · olive srv".into()
            },
            ToolFact::Text {
                text: "x injected ok · note a b c".into()
            },
        ]
    );
}

// Covers: truncation budgets are measured after control flattening, so a
// string of tabs cannot overflow the header or summary fact.
// Owner: mcp display helper
#[test]
fn display_text_truncates_after_control_normalization() {
    let first = format!("{}x", "\t".repeat(80));
    let args = json!({ "prompt": format!("{first}\nmore") });
    let (_, display) = primary_argument(&args).unwrap();
    assert!(display.chars().count() <= 80);
    assert!(!display.contains(char::is_control));
    assert!(display.ends_with('…'));

    let args = json!({ "note": format!("{}y", "\r".repeat(200)) });
    let summary = argument_summary(&args, None).unwrap();
    assert!(summary.chars().count() <= 160);
    assert!(!summary.contains(char::is_control));
    assert!(summary.starts_with("note "));
}
