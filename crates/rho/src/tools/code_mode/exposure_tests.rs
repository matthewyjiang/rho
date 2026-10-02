use pretty_assertions::assert_eq;
use serde_json::json;

use super::{search_entries, ToolCatalogEntry};

fn entry(name: &str, description: &str) -> ToolCatalogEntry {
    ToolCatalogEntry {
        name: name.into(),
        description: description.into(),
        parameters: json!({}),
        returns: json!({}),
    }
}

// Covers: model-written multi-term and operator-prefixed queries find MCP tools
// (a `+memorywhale remember` query returned [] and the parent gave up on MCP).
// Owner: codemode discovery search shared by tool_search and search_tools.
#[test]
fn search_matches_any_term_and_ranks_by_matched_terms() {
    let catalog = [
        entry("bash", "Run shell commands."),
        entry("mcp__memorywhale__remember", "Save a lesson to memory."),
        entry(
            "mcp__memorywhale__search_memory",
            "Full-text search over memory.",
        ),
    ];
    let cases = [
        (
            "+memorywhale remember",
            vec![
                "mcp__memorywhale__remember",
                "mcp__memorywhale__search_memory",
            ],
        ),
        (
            "search MEMORY",
            vec![
                "mcp__memorywhale__search_memory",
                "mcp__memorywhale__remember",
            ],
        ),
        (
            "mcp__memorywhale__remember",
            vec!["mcp__memorywhale__remember"],
        ),
        (
            "  ",
            vec![
                "bash",
                "mcp__memorywhale__remember",
                "mcp__memorywhale__search_memory",
            ],
        ),
        ("github", vec![]),
    ];
    for (query, expected) in cases {
        let names: Vec<_> = search_entries(catalog.iter(), query, usize::MAX)
            .into_iter()
            .map(|hit| hit.name)
            .collect::<Vec<_>>();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!((query, names), (query, expected));
    }
}
