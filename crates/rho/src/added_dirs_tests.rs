use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rho_sdk::{model::ModelIdentity, CompactionState, Revision, SessionId, SessionSnapshot};
use tempfile::TempDir;

use super::*;

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

fn snapshot() -> SessionSnapshot {
    SessionSnapshot::new(
        SessionId::new(),
        Revision::default(),
        Vec::new(),
        ModelIdentity::new("test-provider", "test-api", "test-model"),
        CompactionState::default(),
    )
}

// Covers: an added directory must never duplicate or shadow scope that is
// already granted, and a wider directory absorbs the narrower ones.
// Owner: added-directory set policy
#[test]
fn insert_skips_covered_directories_and_folds_nested_ones() {
    let root = PathBuf::from("/work/project");
    let mut dirs = AddedDirs::default();
    let cases: [(&str, Insertion, &[&str]); 5] = [
        ("/work/other/api", Insertion::Added, &["/work/other/api"]),
        (
            "/work/project/sub",
            Insertion::Covered(root.clone()),
            &["/work/other/api"],
        ),
        (
            "/work/other/api/src",
            Insertion::Covered("/work/other/api".into()),
            &["/work/other/api"],
        ),
        ("/work/other", Insertion::Added, &["/work/other"]),
        ("/data", Insertion::Added, &["/data", "/work/other"]),
    ];
    for (dir, expected, after) in cases {
        assert_eq!(dirs.insert(&root, dir.into()), expected, "{dir}");
        let after: Vec<PathBuf> = after.iter().map(PathBuf::from).collect();
        assert_eq!(dirs.as_slice(), after.as_slice(), "{dir}");
    }
}

// Covers: user paths resolve like a shell would (relative to cwd, `~` to
// home) and anything that is not an existing directory is rejected.
// Owner: added-directory path resolution
#[test]
fn resolve_expands_relative_and_home_paths_and_rejects_non_directories() {
    let cwd = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    std::fs::create_dir(cwd.path().join("sibling")).unwrap();
    std::fs::create_dir(home.path().join("notes")).unwrap();
    std::fs::write(cwd.path().join("file.txt"), "").unwrap();

    let resolve_in = |raw: &str| resolve(Path::new(raw), cwd.path(), Some(home.path()));
    assert_eq!(
        resolve_in("sibling").unwrap(),
        canonical(&cwd.path().join("sibling"))
    );
    assert_eq!(
        resolve_in("~/notes").unwrap(),
        canonical(&home.path().join("notes"))
    );
    assert_eq!(resolve_in("~").unwrap(), canonical(home.path()));
    for rejected in ["file.txt", "missing", "", "~/missing"] {
        assert!(resolve_in(rejected).is_err(), "{rejected:?}");
    }
}

// Covers: a resumed session gets back exactly the directories it saved, and a
// directory deleted since then is reported instead of failing the resume.
// Owner: added-directory session persistence
#[test]
fn snapshot_metadata_round_trips_and_reports_missing_directories() {
    let root = TempDir::new().unwrap();
    let kept = TempDir::new().unwrap();
    let removed = TempDir::new().unwrap();
    let mut dirs = AddedDirs::default();
    dirs.insert(root.path(), canonical(kept.path()));
    dirs.insert(root.path(), canonical(removed.path()));
    let saved = dirs.decorate(snapshot());
    let removed_path = canonical(removed.path());
    drop(removed);

    let restored = AddedDirs::from_metadata(
        saved.metadata().get(METADATA_KEY).map(String::as_str),
        root.path(),
    );

    let mut expected = AddedDirs::default();
    expected.insert(root.path(), canonical(kept.path()));
    assert_eq!(
        restored,
        RestoredDirs {
            dirs: expected,
            missing: vec![removed_path],
        }
    );
    assert_eq!(
        AddedDirs::from_metadata(None, root.path()),
        RestoredDirs::default()
    );
}
