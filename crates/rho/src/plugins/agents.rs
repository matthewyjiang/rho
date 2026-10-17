//! Rho agent definitions shipped inside an Agent Plugin.
//!
//! Agent Plugins 1.0.0 standardizes only skills and MCP servers; agents are
//! outside the portable format (spec §7 and Design Decisions). Rho reads them
//! from its own client extension directory (spec §8.2), which other clients
//! must ignore, so a plugin that ships Rho agents stays portable:
//!
//! `<plugin root>/io.github.matthewyjiang.rho/agents/*.md`
//!
//! This module only locates contained files. The agent catalog parses them
//! and owns id precedence, so plugin agents follow the same definition schema
//! as every other agent file.

use std::path::{Path, PathBuf};

use super::{contain, PluginScope};

/// Reverse-domain extension namespace Rho owns (spec §8). Renaming it breaks
/// every plugin that ships Rho agents.
pub(crate) const RHO_EXTENSION_NAMESPACE: &str = "io.github.matthewyjiang.rho";

/// Agent definition files from one active plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginAgentFiles {
    /// Filesystem-resolved agents directory inside the plugin root.
    pub(crate) dir: PathBuf,
    /// Regular `*.md` files directly in `dir`, in name order. Symlinked files
    /// are left out and reported as problems, so every reader (including the
    /// workflow tool, which never follows links) sees the same set.
    pub(crate) files: Vec<PathBuf>,
}

impl PluginAgentFiles {
    /// File names for plugin inventory. The catalog, not the plugin loader,
    /// parses each file and decides the agent id it defines.
    pub(super) fn file_names(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect()
    }

    /// Reads each kept file for the agent catalog. Every reader (the
    /// interactive catalog and the workflow tool) goes through here.
    ///
    /// Each read walks the path discovery resolved without following links,
    /// so a directory or file swapped for a symlink after discovery fails
    /// instead of reaching outside the plugin root. That anchor is why the
    /// workflow tool can skip the permission gate here: discovery already
    /// applied enable state, trust, and containment, and Rho (not the
    /// workflow graph) chose the files. A file that cannot be read is
    /// third-party content, so it skips only itself.
    pub(crate) fn read_sources(&self) -> Vec<crate::agent::AgentSource> {
        self.files
            .iter()
            .map(|path| crate::agent::AgentSource {
                path: path.clone(),
                contents: crate::workflow::read_resolved_utf8(path)
                    .map_err(|error| error.to_string()),
            })
            .collect()
    }
}

/// Active plugins' agent files, grouped as the catalog's two plugin tiers.
/// Within a tier, nearer project roots come first; order never decides a
/// winner because a duplicate id within one tier is an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PluginAgentTiers {
    pub(crate) user: Vec<PluginAgentFiles>,
    pub(crate) project: Vec<PluginAgentFiles>,
}

impl PluginAgentTiers {
    pub(super) fn push(&mut self, scope: PluginScope, files: PluginAgentFiles) {
        match scope {
            PluginScope::User => self.user.push(files),
            PluginScope::Project => self.project.push(files),
        }
    }
}

/// Locates the plugin's Rho agent files, recording problems the way other
/// component types do: a wrong-kind or escaping location invalidates only
/// agents, and a symlinked file skips only that file (spec §4.1, §6.2).
pub(super) fn discover_plugin_agents(
    root: &Path,
    problems: &mut Vec<String>,
) -> Option<PluginAgentFiles> {
    let location = root.join(RHO_EXTENSION_NAMESPACE).join("agents");
    if !location.exists() {
        return None;
    }
    if !location.is_dir() {
        problems.push(format!(
            "agents location {RHO_EXTENSION_NAMESPACE}/agents exists but is not a directory"
        ));
        return None;
    }
    let dir = match contain::contained_path(root, &location) {
        Ok(dir) => dir,
        Err(error) => {
            problems.push(format!("agents component invalid: {error}"));
            return None;
        }
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => {
            problems.push(format!("agents directory unreadable: {error}"));
            return None;
        }
    };
    let mut entries: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut files = Vec::new();
    for entry in entries {
        // `file_type` does not follow links. `dir` is resolved inside the
        // root, so a regular file in it is too.
        match entry.file_type() {
            Ok(kind) if kind.is_file() => files.push(entry.path()),
            Ok(kind) if kind.is_symlink() => problems.push(format!(
                "skipping agent file `{}`: symlinked agent files are not supported",
                entry.file_name().to_string_lossy()
            )),
            Ok(_) => {}
            Err(error) => problems.push(format!(
                "skipping agent file `{}`: {error}",
                entry.file_name().to_string_lossy()
            )),
        }
    }
    Some(PluginAgentFiles { dir, files })
}
