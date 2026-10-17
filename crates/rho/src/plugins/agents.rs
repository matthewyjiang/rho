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
    pub(crate) scope: PluginScope,
    /// Filesystem-resolved agents directory inside the plugin root.
    pub(crate) dir: PathBuf,
    /// Contained `*.md` files in name order.
    pub(crate) files: Vec<PathBuf>,
}

impl PluginAgentFiles {
    /// Agent names the files claim by stem, for plugin inventory.
    pub(super) fn names(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
            .collect()
    }
}

/// Locates the plugin's Rho agent files, recording problems the way other
/// component types do: a wrong-kind or escaping location invalidates only
/// agents, and an escaping file skips only that file (spec §4.1, §6.2).
pub(super) fn discover_plugin_agents(
    scope: PluginScope,
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
    let mut candidates: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    candidates.sort();

    let mut files = Vec::new();
    for candidate in candidates {
        match contain::contained_path(root, &candidate) {
            Ok(path) => files.push(path),
            Err(error) => {
                let name = candidate
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                problems.push(format!("skipping agent file `{name}`: {error}"));
            }
        }
    }
    Some(PluginAgentFiles { scope, dir, files })
}
