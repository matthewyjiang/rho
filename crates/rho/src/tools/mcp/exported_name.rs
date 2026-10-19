//! MCP tool-name encoding and parsing.
//!
//! Rho owns a private component codec for native exported tools. Other MCP
//! producers may use the same `mcp__server__tool` shape without that codec, so
//! callers must state which wire dialect they are parsing.

use crate::tools::tool_name::{cap_tool_name, capped_body};

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

/// Build Rho's exported name for one MCP server tool. Names within the
/// provider limit round-trip through [`parse_exported_name`]. Longer names are
/// capped with a hash suffix and parse only to a truncated identity, so the
/// definition names the remote tool in its description; registration rejects
/// any resulting collision.
pub(crate) fn namespaced_tool_name(server: &str, tool: &str) -> String {
    cap_tool_name(format!(
        "mcp__{}__{}",
        encode_component(server),
        encode_component(tool)
    ))
}

fn encode_component(value: &str) -> String {
    const ESCAPE_PREFIX: &str = "_rho_";
    let already_safe = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
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
/// For a Rho name capped by [`namespaced_tool_name`], the tool is the decoded
/// readable prefix followed by `…`; a cut inside the server yields `None`.
pub(crate) fn parse_exported_name(
    name: &str,
    dialect: ExportedNameDialect,
) -> Option<McpToolIdentity> {
    let (body, capped) = match dialect {
        // Rho components never contain `-`; Conventional names may.
        ExportedNameDialect::Rho => capped_body(name).map_or((name, false), |body| (body, true)),
        ExportedNameDialect::Conventional => (name, false),
    };
    let rest = body.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    let component = |value: &str| match dialect {
        ExportedNameDialect::Rho => decode_rho_component(value),
        ExportedNameDialect::Conventional => value.to_string(),
    };
    let tool = if capped {
        format!("{}…", component(whole_escape_bytes(tool)))
    } else {
        component(tool)
    };
    Some(McpToolIdentity {
        server: component(server),
        tool,
    })
}

/// Drop a half byte a cap left at the end of a `_rho_` escape payload.
fn whole_escape_bytes(component: &str) -> &str {
    match component.strip_prefix("_rho_") {
        Some(hex) if hex.len() % 2 == 1 => &component[..component.len() - 1],
        _ => component,
    }
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

    // Covers: a server tool whose encoded name exceeds the provider limit
    // exports a provider-valid name that parses to a readable truncated tool,
    // and a cut inside the server never decodes the hash as an escape.
    #[test]
    fn rho_codec_caps_over_long_names() {
        let name = namespaced_tool_name("github", &"get-content".repeat(8));
        assert_eq!(
            (
                name.len(),
                parse_exported_name(&name, ExportedNameDialect::Rho)
            ),
            (
                64,
                Some(McpToolIdentity {
                    server: "github".into(),
                    tool: "get-contentget-con…".into(),
                })
            )
        );
        let server_cut = namespaced_tool_name(&"my-company-docs".repeat(3), "tool");
        assert_eq!(
            parse_exported_name(&server_cut, ExportedNameDialect::Rho),
            None
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
}
