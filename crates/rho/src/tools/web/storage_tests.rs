use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn store_and_load_round_trip_under_data_root() {
    let root = tempfile::tempdir().unwrap();
    let _guard = CacheRootGuard::set(root.path().to_path_buf());
    let response_id = new_response_id();

    store(
        response_id.clone(),
        StoredContent {
            kind: "fetch_content".into(),
            items: vec![StoredItem {
                url: Some("https://example.com".into()),
                query: None,
                title: Some("Example".into()),
                content: "hello body".into(),
                metadata: json!({"mode": "http_fetch"}),
            }],
        },
    )
    .unwrap();

    let path = root
        .path()
        .join("content")
        .join(format!("{response_id}.json"));
    assert!(path.is_file());

    // Drop memory entry by loading from a fresh process view: clear is not
    // exposed, so read the file path contract directly and via load while the
    // in-memory map still has it.
    let loaded = load(&response_id).unwrap();
    assert_eq!(loaded.kind, "fetch_content");
    assert_eq!(loaded.items[0].content, "hello body");
}

#[test]
fn load_falls_back_to_legacy_temp_cache() {
    let root = tempfile::tempdir().unwrap();
    let _guard = CacheRootGuard::set(root.path().to_path_buf());
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

    let loaded = load(&response_id).unwrap();
    assert_eq!(loaded.items[0].content, "legacy body");
    let _ = fs::remove_file(legacy_path);
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
    assert!(listing.contains("query=\"alpha\""));
    assert!(listing.contains("queryIndex=0"));
    assert!(listing.contains("urlIndex=1"));
    assert!(listing.contains("query=\"beta\""));
}
