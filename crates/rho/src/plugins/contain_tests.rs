//! Filesystem containment regressions for package-provided paths.

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::contain;

// Covers: a missing leaf beneath a symlink must not bypass package containment,
// including the shared plugin data directory.
// Owner: plugin path containment.
#[cfg(unix)]
#[test]
fn missing_leaf_beneath_escaping_symlink_is_rejected() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().join("plugin");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("data")).unwrap();
    let root = contain::canonical_root(&root).unwrap();

    for tail in ["link/missing", "data/plugin"] {
        let error = contain::resolve_in_root(&root, tail).unwrap_err();
        assert!(error.contains("escapes the plugin root"), "{tail}: {error}");
    }
}

// Covers: a missing path beneath an internal symlink resolves inside the root.
// Owner: plugin path containment.
#[cfg(unix)]
#[test]
fn missing_leaf_beneath_internal_symlink_uses_resolved_parent() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().join("plugin");
    std::fs::create_dir_all(root.join("real")).unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
    let root = contain::canonical_root(&root).unwrap();

    let resolved = contain::resolve_in_root(&root, "link/missing").unwrap();

    assert_eq!(resolved, root.join("real/missing"));
}
