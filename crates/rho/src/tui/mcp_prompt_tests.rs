use pretty_assertions::assert_eq;

use super::parse_command;

// Covers: only a well-formed `mcp:<server>:<prompt>` command may be treated as
// an MCP prompt, so a skill or prompt template with a similar name still
// reaches its own handler.
// Owner: pure unit
#[test]
fn only_well_formed_mcp_commands_parse() {
    let cases = [
        ("mcp:docs:search", Some(("docs", "search"))),
        ("MCP:docs:search", Some(("docs", "search"))),
        // Prompt names may contain colons; only the first one separates.
        ("mcp:docs:search:deep", Some(("docs", "search:deep"))),
        ("mcp:docs", None),
        ("mcp::search", None),
        ("mcp:docs:", None),
        ("skill:review", None),
        ("prompt:review", None),
        ("mcp", None),
    ];
    for (command, expected) in cases {
        assert_eq!(
            parse_command(command),
            expected.map(|(server, prompt)| (server.to_string(), prompt.to_string())),
            "{command}"
        );
    }
}
