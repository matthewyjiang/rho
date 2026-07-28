use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn load_falls_back_to_legacy_temp_cache() {
    let root = tempfile::tempdir().unwrap();
    let store = WebAccessStore::with_root(root.path().to_path_buf());
    let response_id = new_response_id();

    let legacy_dir = std::env::temp_dir().join("rho-web-access").join("content");
    fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_path = legacy_dir.join(format!("{response_id}.json"));
    let payload = StoredContent {
        kind: "fetch_content".into(),
        items: vec![StoredItem {
            url: Some("https://legacy.example".into()),
            query: None,
            title: None,
            content: "legacy body".into(),
            metadata: json!({}),
        }],
    };
    fs::write(&legacy_path, serde_json::to_string(&payload).unwrap()).unwrap();

    let loaded = store.load(&response_id).unwrap();
    assert_eq!(loaded.items[0].content, "legacy body");
    let _ = fs::remove_file(legacy_path);
}

#[test]
fn unreadable_stored_blob_reports_the_read_failure() {
    let root = tempfile::tempdir().unwrap();
    let store = WebAccessStore::with_root(root.path().to_path_buf());
    let response_id = new_response_id();
    // A directory in the blob's place fails to read for a reason other than
    // "missing", so the legacy fallback must not hide it.
    fs::create_dir_all(
        root.path()
            .join("content")
            .join(format!("{response_id}.json")),
    )
    .unwrap();

    let error = store.load(&response_id).unwrap_err();

    let message = error.to_string();
    assert!(
        message.contains("failed to read stored web content"),
        "unexpected error: {message}"
    );
}

#[test]
fn available_selectors_lists_exact_keys() {
    let stored = StoredContent {
        kind: "web_search".into(),
        items: vec![
            StoredItem {
                url: Some("https://a.example".into()),
                query: Some("alpha".into()),
                title: None,
                content: "a".into(),
                metadata: json!({}),
            },
            StoredItem {
                url: Some("https://b.example".into()),
                query: Some("beta".into()),
                title: None,
                content: "b".into(),
                metadata: json!({}),
            },
        ],
    };

    let listing = available_selectors(&stored);
    assert!(listing.contains("urlIndex=0"));
    assert!(listing.contains("url=https://a.example"));
    assert!(listing.contains("queryIndex=0"));
    assert!(listing.contains("urlIndex=1"));
    assert!(listing.contains("queryIndex=1"));
}
