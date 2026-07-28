use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::*;
use crate::tools::web::storage::StoredItem;

#[test]
fn single_target_marks_truncation_and_points_at_response_id() {
    let item = StoredItem {
        url: Some("https://example.com/big".into()),
        query: None,
        title: None,
        content: "x".repeat(5_000),
        metadata: json!({}),
    };
    let rendered =
        build_fetch_content_output("0123456789abcdef0123456789abcdef", &[item], &[], 800);
    assert!(rendered.len() <= 800);
    let value: Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["contentTruncated"], true);
    assert_eq!(value["itemCount"], 1);
    // Content should not embed the display truncate marker.
    assert!(!value["content"].as_str().unwrap().contains("[truncated]"));
}

#[test]
fn multi_target_keeps_previews_and_requires_retrieve() {
    let items = vec![
        StoredItem {
            url: Some("https://a.example".into()),
            query: None,
            title: None,
            content: "a".into(),
            metadata: json!({}),
        },
        StoredItem {
            url: Some("https://b.example".into()),
            query: None,
            title: None,
            content: "b".into(),
            metadata: json!({}),
        },
    ];
    let previews = vec![
        json!({"url": "https://a.example"}),
        json!({"url": "https://b.example"}),
    ];
    let rendered = build_fetch_content_output(
        "0123456789abcdef0123456789abcdef",
        &items,
        &previews,
        12_000,
    );
    let value: Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["itemCount"], 2);
    assert_eq!(value["contentTruncated"], true);
    assert!(value["items"].as_array().unwrap().len() == 2);
}
