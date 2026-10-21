use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::tools::web::storage::StoredItem;

fn item(url: &str, title: Option<&str>, content: &str) -> StoredItem {
    StoredItem {
        url: Some(url.into()),
        query: None,
        title: title.map(str::to_string),
        content: content.into(),
        metadata: json!({}),
    }
}

// Covers: search results stay as numbered lines under a responseId header
// Owner: web output
#[test]
fn web_search_is_response_id_plus_summaries() {
    assert_eq!(
        format_web_search(
            "0123456789abcdef0123456789abcdef",
            &["1. [Example] https://example.com - hello".into()],
        ),
        "responseId: 0123456789abcdef0123456789abcdef\n1. [Example] https://example.com - hello"
    );
}

// Covers: a single fetch that fits keeps the body and does not mark truncated
// Owner: web output
#[test]
fn single_fetch_inlines_body_when_it_fits() {
    let rendered = format_single_fetch(
        "0123456789abcdef0123456789abcdef",
        &item("https://example.com/doc", Some("Doc"), "hello"),
        12_000,
    );
    assert_eq!(
        rendered,
        "responseId: 0123456789abcdef0123456789abcdef\nurl: https://example.com/doc\ntitle: Doc\n\nhello"
    );
}
