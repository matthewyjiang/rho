use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use super::PathScope;

/// Stable classification for workspace path validation failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspacePathErrorKind {
    RootNotAbsolute,
    RootMissing,
    RootNotDirectory,
    ParentTraversal,
    OutsideGrantedRoots,
    Missing,
    InvalidPlatformPath,
    ChangedAfterAuthorization,
    Io,
}

/// Path error with a sanitized description suitable for a tool result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspacePathError {
    kind: WorkspacePathErrorKind,
    message: String,
}

impl WorkspacePathError {
    fn new(kind: WorkspacePathErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> WorkspacePathErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WorkspacePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WorkspacePathError {}

/// Whether the checked target existed when it was resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspacePathState {
    Existing,
    MissingWriteTarget,
}

/// Canonical path and scope produced immediately before authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedWorkspacePath {
    path: PathBuf,
    scope: PathScope,
    state: WorkspacePathState,
}

impl ResolvedWorkspacePath {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn scope(&self) -> &PathScope {
        &self.scope
    }

    pub fn state(&self) -> WorkspacePathState {
        self.state
    }
}

/// Explicit filesystem scope supplied to tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
    granted_roots: Vec<PathBuf>,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, WorkspacePathError> {
        let root = canonical_directory(root.into(), "workspace root")?;
        Ok(Self {
            root,
            granted_roots: Vec::new(),
        })
    }

    /// Deliberately makes another canonical directory resolvable.
    ///
    /// A granted root is still subject to policy and is labeled
    /// [`PathScope::GrantedRoot`] so policy cannot confuse it with the primary
    /// workspace.
    pub fn with_granted_root(
        mut self,
        root: impl Into<PathBuf>,
    ) -> Result<Self, WorkspacePathError> {
        let root = canonical_directory(root.into(), "granted root")?;
        if root != self.root && !self.granted_roots.contains(&root) {
            self.granted_roots.push(root);
            self.granted_roots.sort();
        }
        Ok(self)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn granted_roots(&self) -> &[PathBuf] {
        &self.granted_roots
    }

    /// Resolves lexically without following the target. Prefer
    /// [`Self::resolve_for_read`] or [`Self::resolve_for_write`] before I/O.
    pub fn resolve(&self, path: impl AsRef<Path>) -> Result<PathBuf, WorkspacePathError> {
        self.lexical_path(path.as_ref()).map(|(path, _)| path)
    }

    /// Resolves an existing target through symlinks and rejects targets outside
    /// the primary or deliberately granted roots.
    pub fn resolve_existing(&self, path: impl AsRef<Path>) -> Result<PathBuf, WorkspacePathError> {
        self.resolve_for_read(path).map(|resolved| resolved.path)
    }

    pub fn resolve_for_read(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ResolvedWorkspacePath, WorkspacePathError> {
        let (lexical, _) = self.lexical_path(path.as_ref())?;
        let canonical = std::fs::canonicalize(&lexical).map_err(|error| {
            let kind = if error.kind() == std::io::ErrorKind::NotFound {
                WorkspacePathErrorKind::Missing
            } else {
                WorkspacePathErrorKind::Io
            };
            WorkspacePathError::new(
                kind,
                format!(
                    "workspace read path '{}' cannot be resolved: {error}",
                    lexical.display()
                ),
            )
        })?;
        let scope = self
            .scope_for(&canonical)
            .ok_or_else(|| outside_error(&lexical))?;
        Ok(ResolvedWorkspacePath {
            path: canonical,
            scope,
            state: WorkspacePathState::Existing,
        })
    }

    /// Resolves an existing target or canonicalizes the nearest existing parent
    /// of a missing write target. The returned path avoids executing through a
    /// symlink observed during validation.
    pub fn resolve_for_write(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ResolvedWorkspacePath, WorkspacePathError> {
        let (lexical, _) = self.lexical_path(path.as_ref())?;
        // Use symlink_metadata so a broken final-component symlink is treated as
        // an existing target instead of a creatable missing path. Path::exists()
        // returns false for broken symlinks, which would otherwise authorize the
        // link path and let later write I/O follow it outside granted roots.
        match std::fs::symlink_metadata(&lexical) {
            Ok(_) => {
                let canonical = std::fs::canonicalize(&lexical).map_err(|error| {
                    let kind = if error.kind() == std::io::ErrorKind::NotFound {
                        WorkspacePathErrorKind::Missing
                    } else {
                        WorkspacePathErrorKind::Io
                    };
                    WorkspacePathError::new(
                        kind,
                        format!(
                            "workspace write path '{}' cannot be resolved: {error}",
                            lexical.display()
                        ),
                    )
                })?;
                let scope = self
                    .scope_for(&canonical)
                    .ok_or_else(|| outside_error(&lexical))?;
                return Ok(ResolvedWorkspacePath {
                    path: canonical,
                    scope,
                    state: WorkspacePathState::Existing,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(WorkspacePathError::new(
                    WorkspacePathErrorKind::Io,
                    format!(
                        "workspace write path '{}' cannot be resolved: {error}",
                        lexical.display()
                    ),
                ));
            }
        }

        let mut ancestor = lexical.as_path();
        let mut missing = Vec::new();
        loop {
            match std::fs::symlink_metadata(ancestor) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(WorkspacePathError::new(
                        WorkspacePathErrorKind::Io,
                        format!(
                            "write parent '{}' cannot be resolved: {error}",
                            ancestor.display()
                        ),
                    ));
                }
            }
            let name = ancestor.file_name().ok_or_else(|| {
                WorkspacePathError::new(
                    WorkspacePathErrorKind::Missing,
                    format!("write path '{}' has no existing parent", lexical.display()),
                )
            })?;
            missing.push(name.to_os_string());
            ancestor = ancestor.parent().ok_or_else(|| {
                WorkspacePathError::new(
                    WorkspacePathErrorKind::Missing,
                    format!("write path '{}' has no existing parent", lexical.display()),
                )
            })?;
        }
        let mut canonical = std::fs::canonicalize(ancestor).map_err(|error| {
            WorkspacePathError::new(
                WorkspacePathErrorKind::Io,
                format!(
                    "write parent '{}' cannot be resolved: {error}",
                    ancestor.display()
                ),
            )
        })?;
        let scope = self
            .scope_for(&canonical)
            .ok_or_else(|| outside_error(&lexical))?;
        for component in missing.iter().rev() {
            canonical.push(component);
        }
        Ok(ResolvedWorkspacePath {
            path: canonical,
            scope,
            state: WorkspacePathState::MissingWriteTarget,
        })
    }

    /// Rechecks a resolved target immediately before I/O and rejects a changed
    /// symlink or parent chain. This narrows, but cannot eliminate, filesystem
    /// races on platforms without descriptor-relative safe-open operations.
    pub fn revalidate(&self, resolved: &ResolvedWorkspacePath) -> Result<(), WorkspacePathError> {
        let current = match resolved.state {
            WorkspacePathState::Existing => self.resolve_for_read(&resolved.path),
            WorkspacePathState::MissingWriteTarget => self.resolve_for_write(&resolved.path),
        }?;
        if current == *resolved {
            Ok(())
        } else {
            Err(WorkspacePathError::new(
                WorkspacePathErrorKind::ChangedAfterAuthorization,
                "workspace path changed after authorization",
            ))
        }
    }

    fn lexical_path(&self, requested: &Path) -> Result<(PathBuf, PathScope), WorkspacePathError> {
        validate_native_path(requested)?;
        if requested
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(WorkspacePathError::new(
                WorkspacePathErrorKind::ParentTraversal,
                format!("path '{}' contains parent traversal", requested.display()),
            ));
        }

        if requested.is_absolute() {
            let scope = self
                .scope_for_absolute(requested)
                .ok_or_else(|| outside_error(requested))?;
            return Ok((requested.to_path_buf(), scope));
        }

        if requested
            .components()
            .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
        {
            return Err(WorkspacePathError::new(
                WorkspacePathErrorKind::InvalidPlatformPath,
                format!(
                    "path '{}' is not valid for this platform",
                    requested.display()
                ),
            ));
        }
        Ok((self.root.join(requested), PathScope::PrimaryWorkspace))
    }

    fn scope_for_absolute(&self, path: &Path) -> Option<PathScope> {
        if let Some(scope) = self.scope_for_lexical(path) {
            return Some(scope);
        }
        // Absolute inputs may use a non-canonical form of a granted root before
        // canonicalize runs. Common cases are macOS /var -> /private/var and
        // Windows extended-length \\?\ prefixes from canonicalize().
        self.scope_for_normalized_absolute(path)
    }

    fn scope_for_lexical(&self, path: &Path) -> Option<PathScope> {
        if path.starts_with(&self.root) {
            return Some(PathScope::PrimaryWorkspace);
        }
        self.granted_roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .map(|root| PathScope::GrantedRoot { root: root.clone() })
    }

    fn scope_for_normalized_absolute(&self, path: &Path) -> Option<PathScope> {
        let mut current = path;
        loop {
            if let Ok(canonical) = std::fs::canonicalize(current) {
                return self.scope_for(&canonical);
            }
            current = current.parent()?;
        }
    }

    fn scope_for(&self, canonical: &Path) -> Option<PathScope> {
        self.scope_for_lexical(canonical)
    }
}

fn canonical_directory(path: PathBuf, label: &str) -> Result<PathBuf, WorkspacePathError> {
    validate_native_path(&path)?;
    if !path.is_absolute() {
        return Err(WorkspacePathError::new(
            WorkspacePathErrorKind::RootNotAbsolute,
            format!("{label} must be an absolute path"),
        ));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(WorkspacePathError::new(
            WorkspacePathErrorKind::ParentTraversal,
            format!("{label} must not contain parent traversal"),
        ));
    }
    let canonical = std::fs::canonicalize(&path).map_err(|error| {
        WorkspacePathError::new(
            WorkspacePathErrorKind::RootMissing,
            format!("{label} must be an existing directory: {error}"),
        )
    })?;
    if !canonical.is_dir() {
        return Err(WorkspacePathError::new(
            WorkspacePathErrorKind::RootNotDirectory,
            format!("{label} must be a directory"),
        ));
    }
    Ok(canonical)
}

fn outside_error(path: &Path) -> WorkspacePathError {
    WorkspacePathError::new(
        WorkspacePathErrorKind::OutsideGrantedRoots,
        format!(
            "path '{}' is outside the workspace and granted roots",
            path.display()
        ),
    )
}

fn validate_native_path(path: &Path) -> Result<(), WorkspacePathError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        if path.as_os_str().as_bytes().contains(&0) {
            return Err(WorkspacePathError::new(
                WorkspacePathErrorKind::InvalidPlatformPath,
                "path contains a NUL byte",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if path.as_os_str().encode_wide().any(|unit| unit == 0) {
            return Err(WorkspacePathError::new(
                WorkspacePathErrorKind::InvalidPlatformPath,
                "path contains a NUL code unit",
            ));
        }
    }
    Ok(())
}
