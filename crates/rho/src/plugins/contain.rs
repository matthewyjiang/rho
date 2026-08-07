//! Filesystem containment for plugin package paths.
//!
//! Every package-supplied path a client discovers, reads, or executes must
//! remain within the filesystem-resolved plugin root after resolving symlinks,
//! junctions, reparse points, and equivalent mechanisms (Agent Plugins spec
//! section 4.1).

use std::path::{Path, PathBuf};

use rho_sdk::{Workspace, WorkspacePathErrorKind};

/// Filesystem-resolved plugin root. Callers pass this to the other helpers.
pub(crate) fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    Workspace::new(root)
        .map(|workspace| workspace.root().to_path_buf())
        .map_err(|error| format!("cannot resolve plugin root `{}`: {error}", root.display()))
}

/// Resolve a package-supplied path that must exist within `root`.
pub(crate) fn contained_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let workspace = workspace(root)?;
    workspace
        .resolve_existing(path)
        .map_err(|error| path_error(path, error))
}

/// Resolve a possibly-missing path against canonical `root`, enforcing
/// containment through every existing ancestor.
///
/// The SDK workspace resolver canonicalizes the nearest existing ancestor
/// before appending a missing suffix. This catches escapes through a symlinked
/// parent even when the leaf itself does not exist yet.
pub(crate) fn resolve_in_root(root: &Path, tail: &str) -> Result<PathBuf, String> {
    let workspace = workspace(root)?;
    workspace
        .resolve_for_write(root.join(tail))
        .map(|resolved| resolved.path().to_path_buf())
        .map_err(|error| path_error(&root.join(tail), error))
}

fn workspace(root: &Path) -> Result<Workspace, String> {
    Workspace::new(root)
        .map_err(|error| format!("cannot resolve plugin root `{}`: {error}", root.display()))
}

fn path_error(path: &Path, error: rho_sdk::WorkspacePathError) -> String {
    if matches!(
        error.kind(),
        WorkspacePathErrorKind::OutsideGrantedRoots | WorkspacePathErrorKind::ParentTraversal
    ) {
        format!("`{}` escapes the plugin root", path.display())
    } else {
        format!("cannot resolve `{}`: {error}", path.display())
    }
}
