//! Durable plugin activation state.
//!
//! Enable and disable choices live outside package directories so install,
//! link, and package updates never rewrite user policy. User state lives under
//! the Rho data root (`$RHO_HOME` or `~/.rho`). Project state lives at
//! `<repository>/.rho/plugins.toml` (or `<cwd>/.rho/plugins.toml` outside a
//! git worktree). Missing files mean every discovered plugin is enabled.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::config_writer;

const STATE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PluginScope {
    Project,
    User,
}

impl PluginScope {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

/// How a package entered a managed plugins root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PluginOrigin {
    /// Placed by hand or by another tool under a discovery root.
    Local,
    /// Copied into a managed root by `rho plugins install`.
    Install,
    /// Symlinked into a managed root by `rho plugins link`.
    Link,
}

impl PluginOrigin {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Install => "install",
            Self::Link => "link",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PluginStateStore {
    pub(crate) user: PluginStateFile,
    pub(crate) project: PluginStateFile,
    pub(crate) user_path: Option<PathBuf>,
    pub(crate) project_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginStateFile {
    #[serde(default = "default_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) plugins: BTreeMap<String, PluginStateEntry>,
}

impl Default for PluginStateFile {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            plugins: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginStateEntry {
    /// Absent means enabled. Explicit `false` disables the package.
    #[serde(default = "default_enabled", skip_serializing_if = "is_true")]
    pub(crate) enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) origin: Option<PluginOrigin>,
    /// Absolute path recorded for linked packages (display and inspect only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) link_target: Option<String>,
}

impl Default for PluginStateEntry {
    fn default() -> Self {
        Self {
            enabled: true,
            origin: None,
            link_target: None,
        }
    }
}

fn default_version() -> u32 {
    STATE_VERSION
}

fn default_enabled() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl PluginStateStore {
    /// Load user and project state for `cwd`. Missing files are empty defaults.
    pub(crate) fn load(cwd: &Path, rho_home: Option<&Path>) -> anyhow::Result<Self> {
        let user_path = user_state_path(rho_home)?;
        let project_path = project_state_path(cwd);
        Ok(Self {
            user: load_file(&user_path)?,
            project: load_file(&project_path)?,
            user_path: Some(user_path),
            project_path: Some(project_path),
        })
    }

    pub(crate) fn is_enabled(&self, scope: PluginScope, name: &str) -> bool {
        self.file(scope)
            .plugins
            .get(name)
            .map(|entry| entry.enabled)
            .unwrap_or(true)
    }

    pub(crate) fn entry(&self, scope: PluginScope, name: &str) -> Option<&PluginStateEntry> {
        self.file(scope).plugins.get(name)
    }

    pub(crate) fn origin(
        &self,
        scope: PluginScope,
        name: &str,
        package_path: &Path,
    ) -> PluginOrigin {
        if let Some(origin) = self.entry(scope, name).and_then(|entry| entry.origin) {
            return origin;
        }
        match std::fs::symlink_metadata(package_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => PluginOrigin::Link,
            _ => PluginOrigin::Local,
        }
    }

    pub(crate) fn set_enabled(
        &mut self,
        scope: PluginScope,
        name: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let entry = self
            .file_mut(scope)
            .plugins
            .entry(name.to_string())
            .or_default();
        entry.enabled = enabled;
        self.persist(scope)
    }

    pub(crate) fn record_install(
        &mut self,
        scope: PluginScope,
        name: &str,
        origin: PluginOrigin,
        link_target: Option<String>,
    ) -> anyhow::Result<()> {
        let entry = self
            .file_mut(scope)
            .plugins
            .entry(name.to_string())
            .or_default();
        entry.origin = Some(origin);
        entry.link_target = link_target;
        // Fresh installs start enabled even if a prior disable entry remained.
        entry.enabled = true;
        self.persist(scope)
    }

    pub(crate) fn clear_package_record(
        &mut self,
        scope: PluginScope,
        name: &str,
    ) -> anyhow::Result<()> {
        self.file_mut(scope).plugins.remove(name);
        self.persist(scope)
    }

    fn file(&self, scope: PluginScope) -> &PluginStateFile {
        match scope {
            PluginScope::User => &self.user,
            PluginScope::Project => &self.project,
        }
    }

    fn file_mut(&mut self, scope: PluginScope) -> &mut PluginStateFile {
        match scope {
            PluginScope::User => &mut self.user,
            PluginScope::Project => &mut self.project,
        }
    }

    fn persist(&self, scope: PluginScope) -> anyhow::Result<()> {
        let path = match scope {
            PluginScope::User => self
                .user_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("user plugin state path is not configured"))?,
            PluginScope::Project => self
                .project_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("project plugin state path is not configured"))?,
        };
        let file = self.file(scope);
        // Drop empty files so a clean tree does not keep an inert plugins.toml.
        if file.plugins.is_empty() {
            match std::fs::remove_file(path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
        let serialized = toml::to_string_pretty(file)?;
        config_writer::write_atomically(path, &serialized)
    }
}

pub(crate) fn user_state_path(rho_home: Option<&Path>) -> anyhow::Result<PathBuf> {
    let root = match rho_home {
        Some(path) => path.to_path_buf(),
        None => crate::paths::rho_dir()?,
    };
    Ok(root.join("plugins.toml"))
}

pub(crate) fn project_state_path(cwd: &Path) -> PathBuf {
    project_root(cwd).join(".rho").join("plugins.toml")
}

/// Repository root when inside a git worktree, otherwise `cwd`.
pub(crate) fn project_root(cwd: &Path) -> PathBuf {
    crate::workspace::project_ancestor_dirs(cwd)
        .into_iter()
        .next()
        .unwrap_or_else(|| cwd.to_path_buf())
}

pub(crate) fn user_plugins_root(home: &Path) -> PathBuf {
    home.join(".agents").join("plugins")
}

pub(crate) fn project_plugins_root(cwd: &Path) -> PathBuf {
    project_root(cwd).join(".agents").join("plugins")
}

fn load_file(path: &Path) -> anyhow::Result<PluginStateFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_state_file(&text)
            .map_err(|error| anyhow::anyhow!("invalid plugin state {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(PluginStateFile::default())
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn parse_state_file(text: &str) -> Result<PluginStateFile, String> {
    let file: PluginStateFile = toml::from_str(text).map_err(|error| error.to_string())?;
    if file.version != STATE_VERSION {
        return Err(format!(
            "unsupported plugins.toml version {} (expected {STATE_VERSION})",
            file.version
        ));
    }
    for name in file.plugins.keys() {
        super::manifest::validate_plugin_name(name)
            .map_err(|error| format!("invalid plugin key `{name}`: {error}"))?;
    }
    Ok(file)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
