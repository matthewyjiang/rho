//! Directories added to a session's workspace scope with `--add-dir` or
//! `/add-dir`, or by the client that opens it: an ACP host's
//! `additionalDirectories` or a daemon client's setup.
//!
//! An added directory becomes an SDK granted root, so checked permission modes
//! treat reads there like workspace reads. Its `AGENTS.md` files join prompt
//! instruction discovery. The set belongs to the session: it is saved in the
//! session snapshot metadata and restored on resume.

use std::path::{Path, PathBuf};

use rho_sdk::{SessionSnapshot, Workspace, WorkspacePathError};

/// Session snapshot metadata key holding the JSON array of added directories.
pub(crate) const METADATA_KEY: &str = "rho.added_dirs";

/// Canonical directories outside the primary workspace, sorted and deduplicated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddedDirs(Vec<PathBuf>);

/// Result of [`AddedDirs::insert`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Insertion {
    Added,
    /// The directory is already in scope through this root: the primary
    /// workspace or an earlier added directory.
    Covered(PathBuf),
}

/// Added directories restored from a saved session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RestoredDirs {
    pub(crate) dirs: AddedDirs,
    /// Saved directories that no longer resolve to a directory. They are left
    /// out of the restored scope.
    pub(crate) missing: Vec<PathBuf>,
}

impl AddedDirs {
    pub(crate) fn as_slice(&self) -> &[PathBuf] {
        &self.0
    }

    /// Adds a canonical directory unless `workspace_root` or an existing entry
    /// already covers it. Entries nested under the new directory are folded
    /// into it.
    pub(crate) fn insert(&mut self, workspace_root: &Path, dir: PathBuf) -> Insertion {
        if dir.starts_with(workspace_root) {
            return Insertion::Covered(workspace_root.to_path_buf());
        }
        if let Some(existing) = self.0.iter().find(|existing| dir.starts_with(existing)) {
            return Insertion::Covered(existing.clone());
        }
        self.0.retain(|existing| !existing.starts_with(&dir));
        self.0.push(dir);
        self.0.sort();
        Insertion::Added
    }

    /// Entries of `self` and `other`, minus any `workspace_root` or another
    /// entry already covers.
    pub(crate) fn union(&self, workspace_root: &Path, other: &Self) -> Self {
        let mut merged = Self::default();
        for dir in self.0.iter().chain(&other.0) {
            merged.insert(workspace_root, dir.clone());
        }
        merged
    }

    /// Grants every added directory on top of the primary `workspace`.
    pub(crate) fn apply_to(&self, workspace: Workspace) -> Result<Workspace, WorkspacePathError> {
        self.0
            .iter()
            .try_fold(workspace, |workspace, dir| workspace.with_granted_root(dir))
    }

    /// Reads the set last saved by [`Self::decorate`] anywhere in `storage`,
    /// not from the active leaf, which may predate `/add-dir`. Unreadable
    /// metadata restores nothing rather than failing the resume.
    pub(crate) fn from_storage(
        storage: &crate::session::Session,
        workspace_root: &Path,
    ) -> RestoredDirs {
        match storage.session_metadata(METADATA_KEY) {
            Ok(encoded) => Self::from_metadata(encoded.as_deref(), workspace_root),
            Err(error) => {
                tracing::warn!(%error, "could not read saved added directories");
                RestoredDirs::default()
            }
        }
    }

    fn from_metadata(encoded: Option<&str>, workspace_root: &Path) -> RestoredDirs {
        let mut restored = RestoredDirs::default();
        for dir in saved_dirs(encoded) {
            match std::fs::canonicalize(&dir) {
                Ok(canonical) if canonical.is_dir() => {
                    restored.dirs.insert(workspace_root, canonical);
                }
                _ => restored.missing.push(dir),
            }
        }
        restored
    }

    /// Records the set in session metadata so resume can restore it.
    pub(crate) fn decorate(&self, snapshot: SessionSnapshot) -> SessionSnapshot {
        if self.0.is_empty() && !snapshot.metadata().contains_key(METADATA_KEY) {
            return snapshot;
        }
        // `resolve` only admits UTF-8 paths, so encoding cannot fail.
        let encoded = serde_json::to_string(&self.0).unwrap_or_else(|_| "[]".into());
        snapshot.with_metadata(METADATA_KEY, encoded)
    }
}

/// The directories a [`METADATA_KEY`] value names, as saved: possibly gone
/// since. Unreadable metadata names none.
pub(crate) fn saved_dirs(encoded: Option<&str>) -> Vec<PathBuf> {
    let Some(encoded) = encoded else {
        return Vec::new();
    };
    serde_json::from_str(encoded).unwrap_or_else(|error| {
        tracing::warn!(%error, "ignoring unreadable saved added directories");
        Vec::new()
    })
}

/// Resolves `--add-dir` values against the launch directory `cwd`, as
/// [`resolve`] does with the user's home. A bad path fails startup instead
/// of silently narrowing the scope the user asked for.
pub(crate) fn launch_added_dirs(raw_dirs: &[PathBuf], cwd: &Path) -> anyhow::Result<AddedDirs> {
    let workspace_root = std::fs::canonicalize(cwd)?;
    let home = crate::paths::home_dir();
    let mut added = AddedDirs::default();
    for raw in raw_dirs {
        let dir = resolve(raw, cwd, home.as_deref())
            .map_err(|error| anyhow::anyhow!("invalid --add-dir: {error}"))?;
        added.insert(&workspace_root, dir);
    }
    Ok(added)
}

/// A requested added directory the session cannot grant. A client that asks
/// for roots expects all of them, so one bad entry fails the request that
/// opens the session rather than open it with fewer.
#[derive(Debug)]
pub(crate) struct RejectedDirectory {
    entry: PathBuf,
    reason: String,
}

impl std::fmt::Display for RejectedDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "additional directory '{}': {}",
            self.entry.display(),
            self.reason
        )
    }
}

impl std::error::Error for RejectedDirectory {}

/// The added directories of a session a client opens with `requested`
/// roots, such as an ACP host's `additionalDirectories` or a daemon client's
/// setup: the `launch` (`--add-dir`) set plus the requested ones. The
/// request alone names the client's roots, so a reopened session does not
/// get back roots it saved earlier. Entries must be absolute, then resolve
/// like `--add-dir`: an existing UTF-8 directory, canonicalized. `root` is
/// the canonical session workspace; entries it or another entry already
/// covers are folded in.
pub(crate) fn session_added_dirs(
    launch: &AddedDirs,
    requested: &[PathBuf],
    root: &Path,
) -> Result<AddedDirs, RejectedDirectory> {
    let mut client = AddedDirs::default();
    for entry in requested {
        let reject = |reason: String| RejectedDirectory {
            entry: entry.clone(),
            reason,
        };
        if entry.as_os_str().is_empty() {
            return Err(reject("path is empty".into()));
        }
        if !entry.is_absolute() {
            return Err(reject("path must be absolute".into()));
        }
        let dir = resolve(entry, root, /*home*/ None).map_err(|error| reject(error.to_string()))?;
        client.insert(root, dir);
    }
    Ok(launch.union(root, &client))
}

/// Resolves a user-supplied directory: `~` expands to `home`, relative paths
/// join `cwd`, and the result is canonical.
pub(crate) fn resolve(raw: &Path, cwd: &Path, home: Option<&Path>) -> anyhow::Result<PathBuf> {
    let text = raw
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{} is not valid UTF-8", raw.display()))?;
    if text.trim().is_empty() {
        anyhow::bail!("directory path is empty");
    }
    let expanded = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = home.ok_or_else(|| anyhow::anyhow!("home directory unavailable"))?;
            home.join(rest.trim_start_matches('/'))
        }
        _ => cwd.join(raw),
    };
    let canonical = std::fs::canonicalize(&expanded)
        .map_err(|error| anyhow::anyhow!("{}: {error}", expanded.display()))?;
    if !canonical.is_dir() {
        anyhow::bail!("{} is not a directory", canonical.display());
    }
    if canonical.to_str().is_none() {
        anyhow::bail!("{} is not valid UTF-8", canonical.display());
    }
    Ok(canonical)
}

#[cfg(test)]
#[path = "added_dirs_tests.rs"]
mod tests;
