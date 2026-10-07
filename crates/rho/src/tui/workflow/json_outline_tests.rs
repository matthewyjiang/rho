use pretty_assertions::assert_eq;
use serde_json::json;

use super::outline_lines;

fn plain(value: &serde_json::Value, width: usize) -> Vec<String> {
    outline_lines(value, width)
        .iter()
        .map(|line| line.to_string().trim_end().to_owned())
        .collect()
}

// Covers: a schema agent result reads summary-first with aligned facts,
// wrapped paragraphs, hanging bullet indents, and an explicit empty list.
// Owner: workflow details JSON outline.
#[test]
fn object_leads_with_facts_then_paragraphs_then_lists() {
    let value = json!({
        "applied": ["collapse the shepherd nodes into one node", "gate publishing"],
        "skipped": [],
        "status": "fixed",
        "summary": "Applied all major findings and the low-risk minor fixes.",
    });

    assert_eq!(
        plain(&value, 30),
        vec![
            "skipped  none",
            "status   fixed",
            "summary",
            "  Applied all major findings",
            "  and the low-risk minor",
            "  fixes.",
            "applied · 2",
            "  • collapse the shepherd",
            "    nodes into one node",
            "  • gate publishing",
        ]
    );
}

// Covers: object items are numbered with their fields hung under the number,
// and a nested list keeps its own marker after the outer one.
// Owner: workflow details JSON outline.
#[test]
fn arrays_of_collections_are_numbered_and_keep_inner_markers() {
    let value = json!([
        {"severity": "major", "title": "two shepherds"},
        ["a", "b"],
    ]);

    assert_eq!(
        plain(&value, 40),
        vec![
            "1. severity  major",
            "   title     two shepherds",
            "",
            "2. • a",
            "   • b",
        ]
    );
}
