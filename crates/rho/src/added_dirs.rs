//! Directories added to a session's workspace scope with `--add-dir` or `/add-dir`.
//!
//! An added directory becomes an SDK granted root, so checked permission modes
//! treat reads there like workspace reads. Its `AGENTS.md` files join prompt
//! instruction discovery. The set belongs to the session: it is saved in the
//! session snapshot metadata and restored on resume.

use std::path::{Path, PathBuf};

use rho_sdk::{SessionSnapshot, Workspace, WorkspacePathError};

/// Session snapshot metadata key holding the JSON array of added directories.
const METADATA_KEY: &str = "rho.added_dirs";

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
        let Some(encoded) = encoded else {
            return RestoredDirs::default();
        };
        let saved: Vec<PathBuf> = match serde_json::from_str(encoded) {
            Ok(saved) => saved,
            Err(error) => {
                tracing::warn!(%error, "ignoring unreadable saved added directories");
                return RestoredDirs::default();
            }
        };
        let mut restored = RestoredDirs::default();
        for dir in saved {
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
