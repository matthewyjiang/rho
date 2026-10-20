//! MCP tool-name encoding and parsing.
//!
//! Rho owns a private component codec for native exported tools. Other MCP
//! producers may use the same `mcp__server__tool` shape without that codec, so
//! callers must state which wire dialect they are parsing.

/// The producer of an exported MCP tool name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportedNameDialect {
    /// A name produced by Rho's [`namespaced_tool_name`] encoder.
    Rho,
    /// The conventional shape used by another producer, such as Claude CLI.
    Conventional,
}

/// Parsed `server` + `tool` components from one exported MCP tool name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpToolIdentity {
    pub(crate) server: String,
    pub(crate) tool: String,
}

/// Build Rho's unambiguous exported name for one MCP server tool.
pub(crate) fn namespaced_tool_name(server: &str, tool: &str) -> String {
    format!("{}{}", exported_name_prefix(server), encode_component(tool))
}

/// The `mcp__<server>__` prefix every exported tool of `server` shares, so the
/// prompt can name it exactly instead of leaving the model to guess the codec.
pub(crate) fn exported_name_prefix(server: &str) -> String {
    format!("mcp__{}__", encode_component(server))
}

fn encode_component(value: &str) -> String {
    const ESCAPE_PREFIX: &str = "_rho_";
    // Providers accept `-` in tool names (see `tools::tool_name`), and it
    // cannot merge with the `__` separator, so a hyphenated name such as
    // `t3-code` stays readable.
    let already_safe = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    // `__` joins the components. A component containing it or beginning or ending
    // with `_` could merge with that separator. The leading rule also reserves the
    // escape prefix for this codec.
    if already_safe && !value.contains("__") && !value.starts_with('_') && !value.ends_with('_') {
        return value.to_string();
    }

    let mut encoded = String::with_capacity(ESCAPE_PREFIX.len() + value.len() * 2);
    encoded.push_str(ESCAPE_PREFIX);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in value.bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

/// Parse `mcp__<server>__<tool>` according to its producer's wire dialect.
pub(crate) fn parse_exported_name(
    name: &str,
    dialect: ExportedNameDialect,
) -> Option<McpToolIdentity> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    let component = |value: &str| match dialect {
        ExportedNameDialect::Rho => decode_rho_component(value),
        ExportedNameDialect::Conventional => value.to_string(),
    };
    Some(McpToolIdentity {
        server: component(server),
        tool: component(tool),
    })
}

/// Find the registered MCP tool that another dialect's spelling names.
///
/// Hosts and servers spell MCP tool names their own way: T3 Code writes
/// `mcp__t3_code__task_status` for Rho's `mcp__t3-code__task_status`, and
/// sessions saved before hyphens stayed verbatim hold hex such as
/// `mcp___rho_74332d636f6465__task_status`. Both sides decode Rho escapes,
/// rejoin as `mcp__<server>__<tool>`, and fold every character outside
/// `[A-Za-z0-9]` to `_`. Comparing joined names, not split pairs, keeps a
/// spelling such as `mcp__a__b__c` ambiguous between server `a` and server
/// `a__b`, as it is in the dialects that produce it. Only a unique match
/// resolves. A registered name always matches itself, so it resolves to itself
/// unless a sibling folds to the same key; callers then keep the exact name.
pub(crate) fn resolve_foreign_spelling<'a>(
    requested: &str,
    registered: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let key = spelling_key(requested)?;
    let mut matches = registered
        .into_iter()
        .filter(|name| spelling_key(name).as_ref() == Some(&key));
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

fn spelling_key(name: &str) -> Option<String> {
    let McpToolIdentity { server, tool } = parse_exported_name(name, ExportedNameDialect::Rho)?;
    Some(
        format!("mcp__{server}__{tool}")
            .replace(|character: char| !character.is_ascii_alphanumeric(), "_"),
    )
}

/// Reverse Rho's `_rho_` + hex component escape. Invalid payloads remain
/// verbatim so malformed names can still be presented without guessed text.
fn decode_rho_component(component: &str) -> String {
    let Some(hex) = component.strip_prefix("_rho_") else {
        return component.to_string();
    };
    if hex.is_empty() || hex.len() % 2 != 0 {
        return component.to_string();
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks(2) {
        let hi = (pair[0] as char).to_digit(16);
        let lo = (pair[1] as char).to_digit(16);
        match (hi, lo) {
            (Some(hi), Some(lo)) => bytes.push((hi * 16 + lo) as u8),
            _ => return component.to_string(),
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| component.to_string())
}

#[cfg(test)]
mod tests {
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
}
