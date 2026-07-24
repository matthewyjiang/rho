use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::*;
use crate::tools::web::storage::StoredItem;

#[test]
fn single_target_inlines_content_without_retrieve_note() {
    let item = StoredItem {
        url: Some("https://example.com".into()),
        query: None,
        title: Some("Example".into()),
        content: "hello from the page".into(),
        metadata: json!({}),
    };
    let rendered =
        build_fetch_content_output("0123456789abcdef0123456789abcdef", &[item], &[], 12_000);
    let value: Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["content"], "hello from the page");
    assert_eq!(value["contentTruncated"], false);
    assert_eq!(value["itemCount"], 1);
    assert!(value.get("note").is_none() || value["note"].is_null());
    assert!(value.get("items").is_none());
}

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
    assert!(value["note"]
        .as_str()
        .unwrap()
        .contains("get_search_content with only responseId"));
    // Content should not embed the display truncate marker.
    assert!(!value["content"].as_str().unwrap().contains("[truncated]"));
}

#[test]
fn single_target_tiny_limit_still_returns_valid_json() {
    let item = StoredItem {
        url: Some("https://example.com/big".into()),
        query: None,
        title: None,
        content: "x".repeat(5_000),
        metadata: json!({}),
    };
    let rendered =
        build_fetch_content_output("0123456789abcdef0123456789abcdef", &[item], &[], 120);
    let value: Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["contentTruncated"], true);
    assert_eq!(value["itemCount"], 1);
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
    assert!(value["note"].as_str().unwrap().contains("urlIndex"));
}
