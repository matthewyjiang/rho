use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::tools::web::storage::StoredItem;

fn item(url: &str, content: &str) -> StoredItem {
    StoredItem {
        url: Some(url.into()),
        query: None,
        title: None,
        content: content.into(),
        metadata: json!({}),
    }
}

#[test]
fn single_target_marks_truncation_and_points_at_response_id() {
    let rendered = build_fetch_content_output(
        "0123456789abcdef0123456789abcdef",
        &[item("https://example.com/big", &"x".repeat(5_000))],
        800,
    );
    assert!(rendered.len() <= 800);
    assert!(rendered.starts_with("responseId: 0123456789abcdef0123456789abcdef\n"));
    assert!(rendered.contains("\ntruncated\n"));
    assert!(!rendered.contains("[truncated]"));
}

#[test]
fn multi_target_keeps_selectors_and_requires_retrieve() {
    let items = vec![
        item("https://a.example", "a"),
        item("https://b.example", "b"),
    ];
    let rendered = build_fetch_content_output("0123456789abcdef0123456789abcdef", &items, 12_000);
    assert_eq!(
        rendered,
        "responseId: 0123456789abcdef0123456789abcdef\nitems: 2\n0. https://a.example\n1. https://b.example"
    );
}
