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
pub(super) fn namespaced_tool_name(server: &str, tool: &str) -> String {
    format!(
        "mcp__{}__{}",
        encode_component(server),
        encode_component(tool)
    )
}

fn encode_component(value: &str) -> String {
    const ESCAPE_PREFIX: &str = "_rho_";
    let already_safe = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    // `__` joins the two components, so a component holding one would make
    // the exported name ambiguous. The prefix is reserved for this codec.
    if already_safe && !value.starts_with(ESCAPE_PREFIX) && !value.contains("__") {
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
}
