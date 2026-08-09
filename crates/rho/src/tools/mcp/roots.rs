//! Filesystem roots advertised to MCP servers.
//!
//! MCP servers ask for `roots/list` to learn which directories the host wants
//! them to operate on. Rho advertises the session workspace, which is fixed for
//! the life of a session. That is why the capability declares
//! `listChanged: false`: there is no change for Rho to notify about, and
//! claiming otherwise would promise a notification that never arrives.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

// `roots` carries a SEP-2577 deprecation marker in rmcp while every shipping
// server still uses it. Rho implements the current wire protocol.
#[expect(deprecated)]
use rmcp::model::Root;

/// The roots Rho advertises, shared by every server session in one run.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpRoots {
    roots: Arc<RwLock<Vec<McpRoot>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpRoot {
    path: PathBuf,
    name: String,
}

impl McpRoots {
    /// Advertise one workspace directory. A path that cannot become a `file://`
    /// URL is skipped rather than sent in a form servers cannot parse.
    pub(crate) fn for_workspace(workspace: &Path) -> Self {
        let roots = match root_name(workspace) {
            Some(name) => vec![McpRoot {
                path: workspace.to_path_buf(),
                name,
            }],
            None => Vec::new(),
        };
        Self {
            roots: Arc::new(RwLock::new(roots)),
        }
    }

    #[expect(deprecated)]
    pub(crate) fn to_protocol(&self) -> Vec<Root> {
        self.read()
            .iter()
            .filter_map(|root| {
                let uri = file_uri(&root.path)?;
                Some(Root::new(uri).with_name(root.name.clone()))
            })
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// A poisoned roots lock means another thread panicked while holding it.
    /// The contents stay a valid root list either way, so recover instead of
    /// propagating the panic into an MCP request handler.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Vec<McpRoot>> {
        self.roots.read().unwrap_or_else(|error| error.into_inner())
    }
}

fn root_name(path: &Path) -> Option<String> {
    file_uri(path)?;
    Some(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_string(),
    )
}

/// `Url::from_directory_path` rejects relative paths and paths it cannot encode.
fn file_uri(path: &Path) -> Option<String> {
    url::Url::from_directory_path(path)
        .ok()
        .map(|url| url.to_string())
}

#[cfg(test)]
#[path = "roots_tests.rs"]
mod tests;
