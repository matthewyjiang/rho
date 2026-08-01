use std::fs;

use super::*;
use crate::workflow::test_support::limits;

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(future)
}

// Covers: module traversal or cycles could escape source digest coverage.
// Owner: workflow source collector.
#[test]
fn rejects_traversal_and_import_cycles() {
    assert!(matches!(
        validate_label("//../escape.star"),
        Err(WorkflowError::InvalidModuleLabel { .. })
    ));

    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("a.star"),
        "load(\"//b.star\", \"b\")\na = b",
    )
    .unwrap();
    fs::write(
        root.path().join("b.star"),
        "load(\"//a.star\", \"a\")\nb = a",
    )
    .unwrap();
    let limits = limits();
    let collector = SourceCollector::new(root.path(), &limits).unwrap();
    assert!(matches!(
        block_on(collector.collect(Path::new("a.star"))),
        Err(WorkflowError::ImportCycle { .. })
    ));
}

// Covers: repeated imports could evaluate or digest one source more than once.
// Owner: workflow source collector.
#[test]
fn collects_each_loaded_source_once_in_sorted_manifest() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("common.star"), "value = 1").unwrap();
    fs::write(
        root.path().join("entry.star"),
        "load(\"//common.star\", \"value\")\nresult = value",
    )
    .unwrap();
    let limits = limits();
    let collected = block_on(
        SourceCollector::new(root.path(), &limits)
            .unwrap()
            .collect(Path::new("entry.star")),
    )
    .unwrap();
    assert_eq!(
        collected.sources.keys().cloned().collect::<Vec<_>>(),
        vec!["//common.star".to_owned(), "//entry.star".to_owned()]
    );
}

// Covers: a source path substituted with a symlink must never be opened after authorization.
// Owner: workflow source collector filesystem boundary.
#[cfg(unix)]
#[test]
fn rejects_source_symlink_substitution() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), "WORKFLOW = None").unwrap();
    symlink(outside.path(), root.path().join("entry.star")).unwrap();

    let limits = limits();
    let result = block_on(
        SourceCollector::new(root.path(), &limits)
            .unwrap()
            .collect(Path::new("entry.star")),
    );

    assert!(matches!(result, Err(WorkflowError::SourceSymlink { .. })));
}
