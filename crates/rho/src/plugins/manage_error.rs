//! Why a plugin management operation failed, by what its caller can do
//! about it.
//!
//! The CLI prints these as text, and the daemon answers each kind with its
//! own `rho/plugins/*` error code. Each message is the CLI's text, so the
//! CLI's output does not depend on which kind an error is.

use std::path::PathBuf;

use crate::plugins::TRUST_PROJECT_PLUGINS_ENV;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ManageError {
    /// No package of that name is in the plugins roots searched, or a
    /// recorded package is gone from its root.
    #[error("{0}")]
    NotFound(String),
    /// The destination holds a plugin already, and the caller did not
    /// force replacing it.
    #[error(
        "plugin already exists at {} (pass --force to replace)",
        crate::paths::display(.path)
    )]
    Exists { name: String, path: PathBuf },
    /// A named update of a project plugin in a workspace whose project
    /// plugins are not trusted.
    #[error(
        "`{name}` is a project plugin and the workspace is not trusted; set {TRUST_PROJECT_PLUGINS_ENV}=1 to update it"
    )]
    Untrusted { name: String },
    /// The source, or what a fetch brought, is not a usable plugin package.
    #[error("{0}")]
    InvalidPackage(String),
    /// `git` is missing, or could not fetch or check out the source.
    #[error("{0}")]
    Fetch(String),
    /// The request cannot be carried out as asked: a scope with no plugins
    /// root here, a destination outside the managed root, or an update of
    /// a package not installed from git.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ManageError {
    /// `error`, with its causes, as a package the caller cannot use.
    pub(crate) fn invalid_package(error: &anyhow::Error) -> Self {
        Self::InvalidPackage(format!("{error:#}"))
    }

    /// `error`, with its causes, as a failed `git` run.
    pub(crate) fn fetch(error: &anyhow::Error) -> Self {
        Self::Fetch(format!("{error:#}"))
    }
}
