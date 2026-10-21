use pretty_assertions::assert_eq;

use super::*;

#[test]
fn rho_codec_round_trips_unsafe_and_reserved_components() {
    for (server, tool) in [
        ("git-hub", "issues/list"),
        ("devtools__validator", "lint"),
        ("_rho_6162", "tool"),
        ("a_", "tool"),
        ("a", "_tool"),
        ("_a", "lint"),
        ("srv", "tool_"),
        ("-t3-code-", "get-issue"),
    ] {
        let name = namespaced_tool_name(server, tool);
        assert_eq!(
            parse_exported_name(&name, ExportedNameDialect::Rho),
            Some(McpToolIdentity {
                server: server.into(),
                tool: tool.into(),
            })
        );
    }
}

// Covers: names over the provider limit are capped, and a capped name never
// decodes into a truncated identity; only the session's registered names
// recover it, while uncapped names still decode without registration.
// Owner: MCP exported-name codec.
#[test]
fn capped_names_resolve_only_through_registered_names() {
    let identity = |server: &str, tool: &str| McpToolIdentity {
        server: server.into(),
        tool: tool.into(),
    };
    let capped = [
        // The cut lands in the tool, after an intact server prefix.
        identity(
            "chrome-devtools",
            "performance_analyze_insight_for_extended_trace_sessions",
        ),
        // A plugin server's escaped identity, cut inside the server.
        identity("devtools-suite/validator-service", "echo/value"),
    ];
    let names = capped
        .iter()
        .map(|identity| namespaced_tool_name(&identity.server, &identity.tool))
        .collect::<Vec<_>>();
    let registered = names
        .iter()
        .cloned()
        .zip(capped.iter().cloned())
        .collect::<McpToolNames>();
    let uncapped = namespaced_tool_name("t3-code", "task_status");

    assert_eq!(
        names
            .iter()
            .map(|name| (
                name.len() <= 64,
                parse_exported_name(name, ExportedNameDialect::Rho),
                McpToolNames::default().identify(name),
                registered.identify(name),
            ))
            .collect::<Vec<_>>(),
        capped
            .iter()
            .map(|identity| (true, None, None, Some(identity.clone())))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        McpToolNames::default().identify(&uncapped),
        Some(identity("t3-code", "task_status"))
    );
}

#[test]
fn conventional_names_never_decode_rho_escape_shaped_components() {
    assert_eq!(
        parse_exported_name(
            "mcp___rho_6162___rho_746f6f6c",
            ExportedNameDialect::Conventional,
        ),
        Some(McpToolIdentity {
            server: "_rho_6162".into(),
            tool: "_rho_746f6f6c".into(),
        })
    );
}

// Covers: other harnesses' spellings and legacy hex names find the one
// registered tool they mean; spellings two tools share, including across
// the `__` separator, and non-MCP spellings resolve to nothing.
#[test]
fn foreign_spellings_resolve_only_to_a_unique_tool() {
    let registered = [
        namespaced_tool_name("t3-code", "task_status"),
        namespaced_tool_name("acme/docs", "search"),
        namespaced_tool_name("srv", "get-issue"),
        namespaced_tool_name("srv", "get.issue"),
        namespaced_tool_name("a", "b__c"),
        namespaced_tool_name("a__b", "c"),
        namespaced_tool_name("foo-", "tool"),
        "read_file".to_string(),
    ];
    let resolve = |requested: &str| {
        resolve_foreign_spelling(requested, registered.iter().map(String::as_str))
    };
    assert_eq!(
        [
            "mcp__t3_code__task_status",
            "mcp___rho_74332d636f6465__task_status",
            "mcp__acme_docs__search",
            "mcp__foo___tool",
            "mcp__srv__get_issue",
            "mcp__srv__get-issue",
            "mcp__a__b__c",
            "mcp__t3_code__missing",
            "read_file",
        ]
        .map(resolve),
        [
            Some("mcp__t3-code__task_status"),
            Some("mcp__t3-code__task_status"),
            Some(registered[1].as_str()),
            Some("mcp__foo-__tool"),
            None,
            None,
            None,
            None,
            None,
        ]
    );
}
