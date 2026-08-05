use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use pretty_assertions::assert_eq;

use super::*;

// Covers: identical content reuses one version and unions seen lines
// Owner: snapshot store
#[test]
fn record_fuses_identical_content_and_merges_seen_lines() {
    let store = SnapshotStore::new();
    let path = PathBuf::from("/tmp/a.rs");
    let tag1 = store.record(&path, "one\ntwo\n", Some([1usize]));
    let tag2 = store.record(&path, "one\ntwo\n", Some([2usize]));
    assert_eq!(tag1, tag2);
    let snap = store.by_hash(&path, &tag1).unwrap();
    assert_eq!(snap.seen_lines.unwrap(), HashSet::from([1, 2]));
}

// Covers: short-tag collisions must not fuse distinct texts
// Owner: snapshot store
#[test]
fn record_keeps_distinct_texts_that_share_a_tag_apart() {
    let store = SnapshotStore::with_limits(10, 4, 1024 * 1024);
    let path = PathBuf::from("/tmp/a.rs");
    // Force two versions even if hashes differ; identity is full text.
    store.record(&path, "alpha\n", Some([1usize]));
    store.record(&path, "beta\n", Some([1usize]));
    assert_eq!(store.head(&path).unwrap().text, "beta\n");
    assert!(store.by_content(&path, "alpha\n").is_some());
}

// Covers: path LRU drops cold histories under the path budget
// Owner: snapshot store
#[test]
fn evicts_least_recent_paths_when_over_budget() {
    let store = SnapshotStore::with_limits(2, 2, 1024 * 1024);
    store.record(PathBuf::from("/a"), "a\n", None::<[usize; 0]>);
    store.record(PathBuf::from("/b"), "b\n", None::<[usize; 0]>);
    store.record(PathBuf::from("/c"), "c\n", None::<[usize; 0]>);
    assert!(store.head(Path::new("/a")).is_none());
    assert!(store.head(Path::new("/b")).is_some());
    assert!(store.head(Path::new("/c")).is_some());
}
