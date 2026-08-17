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

// Covers: the in-memory cache must evict oldest entries and oversized blobs
// Owner: web storage
#[test]
fn memory_cache_evicts_by_count_bytes_and_recency() {
    struct Case {
        entry_limit: usize,
        byte_limit: usize,
        insert: &'static [&'static str],
        touch: Option<&'static str>,
        extra: Option<(&'static str, &'static str)>,
        retain: &'static [&'static str],
        drop: &'static [&'static str],
    }

    let cases = [
        Case {
            entry_limit: 2,
            byte_limit: 64,
            insert: &["a", "b"],
            touch: None,
            extra: Some(("c", "c")),
            retain: &["b", "c"],
            drop: &["a"],
        },
        Case {
            entry_limit: 2,
            byte_limit: 64,
            insert: &["a", "b"],
            touch: Some("a"),
            extra: Some(("c", "c")),
            retain: &["a", "c"],
            drop: &["b"],
        },
        Case {
            entry_limit: 2,
            byte_limit: 8,
            insert: &["a", "b"],
            touch: None,
            extra: Some(("big", "0123456789")),
            retain: &["a", "b"],
            drop: &["big"],
        },
        Case {
            entry_limit: 2,
            byte_limit: 16,
            insert: &["aaaaaaaa"],
            touch: None,
            extra: Some(("overflow", "0123456789")),
            retain: &["overflow"],
            drop: &["aaaaaaaa"],
        },
    ];

    for case in cases {
        let mut cache = MemoryCache::new(
            /*entry_limit*/ case.entry_limit,
            /*byte_limit*/ case.byte_limit,
        );
        for id in case.insert {
            cache.insert((*id).to_owned(), stored_body(id));
        }
        if let Some(id) = case.touch {
            assert!(cache.get(id).is_some());
        }
        if let Some((id, body)) = case.extra {
            cache.insert(id.to_owned(), stored_body(body));
        }
        for id in case.retain {
            assert!(cache.contains(id), "expected {id} retained");
        }
        for id in case.drop {
            assert!(!cache.contains(id), "expected {id} evicted");
        }
    }
}

// Covers: get_search_content must still load a body after RAM eviction
// Owner: web storage
#[test]
fn evicted_memory_entry_still_loads_from_disk() {
    let root = tempfile::tempdir().unwrap();
    let store = WebAccessStore::with_root(root.path().to_path_buf());
    let mut ids = Vec::new();
    for index in 0..=MEMORY_ENTRY_LIMIT {
        let response_id = new_response_id();
        store
            .store(response_id.clone(), stored_body(&format!("body-{index}")))
            .unwrap();
        ids.push(response_id);
    }

    assert!(!store.memory_contains(&ids[0]));
    assert!(store.memory_contains(ids.last().expect("inserted ids")));

    let loaded = store.load(&ids[0]).unwrap();
    assert_eq!(loaded, stored_body("body-0"));
    assert!(store.memory_contains(&ids[0]));
}

fn stored_body(content: &str) -> StoredContent {
    StoredContent {
        kind: String::new(),
        items: vec![StoredItem {
            url: None,
            query: None,
            title: None,
            content: content.into(),
            metadata: json!({}),
        }],
    }
}
