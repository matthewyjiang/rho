//! Filesystem containment for plugin package paths.
//!
//! Every package-supplied path a client discovers, reads, or executes must
//! remain within the filesystem-resolved plugin root after resolving
//! symlinks, junctions, reparse points, and equivalent mechanisms
//! (Agent Plugins spec §4.1). `std::fs::canonicalize` performs that
//! resolution on the platforms Rho supports.

use std::path::{Component, Path, PathBuf};

/// Filesystem-resolved plugin root. Callers pass this to the other helpers.
pub(crate) fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve plugin root `{}`: {error}", root.display()))
}

/// Resolve a package-supplied path that must stay within `root`.
///
/// The path must exist; symlinked targets outside the root are rejected.
pub(crate) fn contained_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let resolved = std::fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve `{}`: {error}", path.display()))?;
    if !resolved.starts_with(root) {
        return Err(format!("`{}` escapes the plugin root", path.display()));
    }
    Ok(resolved)
}

/// Resolve a possibly-missing path against `root`, enforcing containment.
///
/// Existing paths are canonicalized so symlink escapes are caught; missing
/// paths are normalized lexically (a missing path cannot hold a symlink).
pub(crate) fn resolve_in_root(root: &Path, tail: &str) -> Result<PathBuf, String> {
    let candidate = root.join(tail);
    if candidate.exists() {
        return contained_path(root, &candidate);
    }
    normalized_within(root, tail)
}

/// Join `tail` onto `base` after lexical normalization, rejecting any `..`
/// component that would escape `base`. `tail` must be relative.
pub(crate) fn normalized_within(base: &Path, tail: &str) -> Result<PathBuf, String> {
    let mut path = base.to_path_buf();
    let mut depth = 0usize;
    for component in Path::new(tail).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return Err(format!("`{tail}` escapes its permitted root"));
                }
                depth -= 1;
                path.pop();
            }
            Component::Normal(part) => {
                depth += 1;
                path.push(part);
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("`{tail}` must be a relative path"));
            }
        }
    }
    Ok(path)
}
