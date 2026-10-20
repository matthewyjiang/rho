use pretty_assertions::assert_eq;
use serde_json::json;

use super::{search_entries, ToolCatalogEntry};

fn entry(name: &str, description: &str) -> ToolCatalogEntry {
    ToolCatalogEntry {
        name: name.into(),
        description: description.into(),
        parameters: json!({}),
        returns: None,
    }
}

fn with_parameters(mut entry: ToolCatalogEntry, names: &[&str]) -> ToolCatalogEntry {
    let properties: serde_json::Map<_, _> = names
        .iter()
        .map(|name| ((*name).to_owned(), json!({"type": "string"})))
        .collect();
    entry.parameters = json!({"type": "object", "properties": properties});
    entry
}

/// Asserts each query's full ranking at several limits, so truncation keeps
/// the ranked prefix.
fn assert_rankings(catalog: &[ToolCatalogEntry], cases: &[(&str, Vec<&str>)]) {
    for (query, expected) in cases {
        for limit in [0, 1, catalog.len(), usize::MAX] {
            let names: Vec<&str> = search_entries(catalog, query, limit)
                .into_iter()
                .map(|hit| hit.name.as_str())
                .collect();
            assert_eq!(
                (*query, limit, names.as_slice()),
                (*query, limit, &expected[..expected.len().min(limit)])
            );
        }
    }
}

// Covers: model-written multi-term and operator-prefixed queries find MCP tools
// (a `+memorywhale remember` query returned [] and the parent gave up on MCP).
// Owner: codemode discovery search shared by tool_search and search_tools.
#[test]
fn search_matches_any_term_and_ranks_name_hits_first() {
    let catalog = [
        entry("bash", "Run shell commands."),
        entry("mcp__memorywhale__remember", "Save a lesson to memory."),
        entry(
            "mcp__memorywhale__search_memory",
            "Full-text search over memory.",
        ),
    ];
    assert_rankings(
        &catalog,
        &[
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
                vec![
                    "mcp__memorywhale__remember",
                    "mcp__memorywhale__search_memory",
                ],
            ),
            (
                "  ",
                vec![
                    "bash",
                    "mcp__memorywhale__remember",
                    "mcp__memorywhale__search_memory",
                ],
            ),
            // A name hit outranks a description hit.
            ("shell remember", vec!["mcp__memorywhale__remember", "bash"]),
            // Filler words must not match every description containing `a`/`to`.
            ("post a message to slack", vec![]),
            ("github", vec![]),
        ],
    );
}

// Covers: query and tool spellings differ in case style and inflection, as
// with `createIssue` against `create_issue`, or `listing issues` against
// `list_issues`. Owner: search normalization.
#[test]
fn search_normalizes_case_styles_separators_and_inflections() {
    let catalog = [
        entry("mcp__github__create_issue", "Open a new issue."),
        entry("mcp__github__list_issues", "List issues in a repository."),
        entry("mcp__playwright__browser_navigate", "Navigate to a URL."),
        entry("str_replace", "Replace exact text in a file."),
        entry(
            "openapi__github__repos_getContent",
            "Get repository content\n\nGET /repos/{owner}/{repo}/contents/{path} on OpenAPI source `github` (tags: repos).",
        ),
    ];
    assert_rankings(
        &catalog,
        &[
            (
                "createIssue",
                vec!["mcp__github__create_issue", "mcp__github__list_issues"],
            ),
            (
                "listing issues",
                vec!["mcp__github__list_issues", "mcp__github__create_issue"],
            ),
            (
                "browser-navigate",
                vec!["mcp__playwright__browser_navigate"],
            ),
            // `repos` also reaches `repository` by prefix, at a discount.
            (
                "repos.get_content",
                vec![
                    "openapi__github__repos_getContent",
                    "mcp__github__list_issues",
                ],
            ),
            // The catalog kind is a low-weight word, so it still lists that
            // catalog without diluting tool names.
            (
                "mcp",
                vec![
                    "mcp__github__create_issue",
                    "mcp__github__list_issues",
                    "mcp__playwright__browser_navigate",
                ],
            ),
            // `getting` stems to `get`, but `string` keeps its ending rather
            // than becoming `str`.
            ("getting content", vec!["openapi__github__repos_getContent"]),
            ("string", vec![]),
            // A query made only of filler matches nothing.
            ("a to", vec![]),
        ],
    );
}

// Covers: the substring scorer gave every `get` + `repository` operation the
// same score and fell back to name order, burying `repos/get` under longer
// operations. BM25 prefers the shorter, more specific match and lets a rare
// term outweigh a common one. Owner: search ranking.
#[test]
fn search_prefers_specific_matches_and_rare_terms() {
    let catalog = [
        entry(
            "openapi__github__actions_get_github_actions_permissions_repository",
            "Get GitHub Actions permissions for a repository\n\nGET /repos/{owner}/{repo}/actions/permissions on OpenAPI source `github` (tags: actions).",
        ),
        entry(
            "openapi__github__repos_get",
            "Get a repository\n\nGET /repos/{owner}/{repo} on OpenAPI source `github` (tags: repos).",
        ),
        entry(
            "openapi__github__repos_get_webhook",
            "Get a repository webhook\n\nGET /repos/{owner}/{repo}/hooks/{hook_id} on OpenAPI source `github` (tags: repos).",
        ),
        with_parameters(
            entry(
                "openapi__github__repos_list_for_org",
                "List organization repositories\n\nGET /orgs/{org}/repos on OpenAPI source `github` (tags: repos).",
            ),
            &["org", "type", "sort"],
        ),
    ];
    let expected_first = [
        ("get repository", "openapi__github__repos_get"),
        ("repository webhook", "openapi__github__repos_get_webhook"),
        // `sort` appears only as a parameter name.
        ("sort repositories", "openapi__github__repos_list_for_org"),
    ];
    for (query, expected) in expected_first {
        let first = search_entries(&catalog, query, 1)
            .first()
            .map(|hit| hit.name.as_str());
        assert_eq!((query, first), (query, Some(expected)));
    }
}
