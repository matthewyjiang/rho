use pretty_assertions::assert_eq;

use super::{parse_command, prompt_turn_text};

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

// Covers: a server-controlled prompt description must share the same output
// budget as the expanded body, so prompts/get cannot build an arbitrarily large
// model turn by stuffing the description while the body alone is capped.
// Owner: pure unit (MCP prompt expansion budgeting).
#[test]
fn prompt_description_shares_output_budget() {
    let body = "body-text";
    let description = "D".repeat(20);
    let combined = prompt_turn_text(Some(&description), body, 12);
    assert!(combined.starts_with('D'));
    assert!(combined.len() <= 12 + "\n[truncated]".len());
    assert!(combined.contains("[truncated]") || combined.len() <= 12);

    assert_eq!(
        prompt_turn_text(None, body, 12),
        body,
        "no description leaves the already-capped body alone"
    );
    assert_eq!(
        prompt_turn_text(Some("   "), body, 12),
        body,
        "whitespace-only description is ignored"
    );
}
