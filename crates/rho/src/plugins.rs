//! Agent Plugins 1.0.0 package loading (specification status: Working Draft).
//!
//! Rho loads plugin packages from explicit roots and never recursively
//! searches arbitrary directories for `plugin.json`:
//!
//! - project roots: `<ancestor>/.agents/plugins`, nearest ancestor first
//! - user root: `~/.agents/plugins`
//!
//! Supported component types in this build: skills and MCP servers (stdio
//! and Streamable HTTP transports; legacy HTTP+SSE is skipped per entry).
//! Plugin skills join ordinary skill discovery below every loose skill
//! location; see `crate::skills` for precedence. Plugin MCP servers are
//! translated into the generic native MCP configuration and share its
//! transport, lifecycle, permission, and tool-registration path.
//!
//! Version handling stays isolated behind `$schema` recognition because the
//! 1.0.0 specification is a Working Draft.
//!
//! Specification: <https://agent-plugins.org/specification>

#[path = "plugins/contain.rs"]
mod contain;
#[path = "plugins/manifest.rs"]
mod manifest;
#[path = "plugins/mcp_adapter.rs"]
mod mcp_adapter;

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "plugins/contain_tests.rs"]
mod contain_tests;

use std::path::{Path, PathBuf};

use crate::skills::{Skill, SkillSource};
use crate::tools::mcp::config::{InvalidMcpServer, McpConfig, McpServerConfig};

pub(crate) const SUPPORTED_COMPONENTS: &str =
    "skills, mcp (stdio, streamable-http; sse unsupported)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginScope {
    Project,
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PluginStatus {
    Loaded,
    Rejected,
    Shadowed,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct PluginReportEntry {
    pub(crate) name: String,
    pub(crate) root: String,
    pub(crate) status: PluginStatus,
    /// Manifest warnings plus component-level problems; empty when clean.
    pub(crate) problems: Vec<String>,
    pub(crate) skill_count: usize,
    pub(crate) mcp_server_count: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct PluginLoadReport {
    pub(crate) plugins: Vec<PluginReportEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PluginLoadSummary {
    pub(crate) discovered: bool,
    pub(crate) loaded: usize,
    pub(crate) rejected: usize,
    pub(crate) problems: usize,
    pub(crate) skills: usize,
    pub(crate) mcp_servers: usize,
}

pub(crate) struct PluginDiscovery {
    pub(crate) skills: Vec<Skill>,
    pub(crate) mcp: McpConfig,
    pub(crate) report: PluginLoadReport,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ComponentSelection {
    All,
    SkillsOnly,
    McpOnly,
}

impl ComponentSelection {
    const fn loads_skills(self) -> bool {
        matches!(self, Self::All | Self::SkillsOnly)
    }

    const fn loads_mcp(self) -> bool {
        matches!(self, Self::All | Self::McpOnly)
    }
}

/// Plugin-owned skills in precedence order for ordinary skill discovery.
pub(crate) fn skills_by_precedence(cwd: &Path, home: Option<&Path>) -> Vec<Skill> {
    discover_components(cwd, home, ComponentSelection::SkillsOnly).skills
}

/// Discover and load plugin packages from the explicit roots.
pub(crate) fn discover(cwd: &Path, home: Option<&Path>) -> PluginDiscovery {
    discover_components(cwd, home, ComponentSelection::All)
}

/// Discover only plugin MCP configuration for the `rho mcp` inventory path.
pub(crate) fn discover_mcp(cwd: &Path, home: Option<&Path>) -> PluginDiscovery {
    discover_components(cwd, home, ComponentSelection::McpOnly)
}

fn discover_components(
    cwd: &Path,
    home: Option<&Path>,
    components: ComponentSelection,
) -> PluginDiscovery {
    let mut discovery = PluginDiscovery {
        skills: Vec::new(),
        mcp: McpConfig::default(),
        report: PluginLoadReport::default(),
    };

    for root in plugin_roots(cwd, home) {
        let Some(candidates) = plugin_candidates(&root.path) else {
            continue;
        };
        for candidate in candidates {
            load_candidate(
                &mut discovery,
                &root.path,
                &candidate,
                root.scope,
                components,
            );
        }
    }

    discovery
}

struct RootSpec {
    path: PathBuf,
    scope: PluginScope,
}

fn plugin_roots(cwd: &Path, home: Option<&Path>) -> Vec<RootSpec> {
    let mut roots: Vec<RootSpec> = crate::workspace::project_ancestor_dirs(cwd)
        .into_iter()
        .rev()
        .map(|dir| RootSpec {
            path: dir.join(".agents").join("plugins"),
            scope: PluginScope::Project,
        })
        .collect();
    if let Some(home) = home {
        roots.push(RootSpec {
            path: home.join(".agents").join("plugins"),
            scope: PluginScope::User,
        });
    }
    roots
}

/// Immediate child directories of `plugins_root` that contain a `plugin.json`
/// entry, sorted for deterministic order. Missing roots are not an error.
fn plugin_candidates(plugins_root: &Path) -> Option<Vec<PathBuf>> {
    let entries = std::fs::read_dir(plugins_root).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("plugin.json").exists())
        .collect();
    candidates.sort();
    Some(candidates)
}

fn load_candidate(
    discovery: &mut PluginDiscovery,
    plugins_root: &Path,
    candidate: &Path,
    scope: PluginScope,
    components: ComponentSelection,
) {
    let directory_name = candidate
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| candidate.display().to_string());

    let reject = |discovery: &mut PluginDiscovery, reason: String| {
        discovery.report.plugins.push(PluginReportEntry {
            name: directory_name.clone(),
            root: crate::paths::display(candidate),
            status: PluginStatus::Rejected,
            problems: vec![reason],
            skill_count: 0,
            mcp_server_count: 0,
        });
    };

    // The manifest loads before any component. If plugin.json escapes the
    // resolved root, the whole plugin is rejected (spec §4.1).
    let root = match contain::canonical_root(candidate) {
        Ok(root) => root,
        Err(error) => return reject(discovery, error),
    };
    let manifest_path = root.join("plugin.json");
    if !manifest_path.is_file() {
        return reject(discovery, "plugin.json is not a regular file".to_string());
    }
    if let Err(error) = contain::contained_path(&root, &manifest_path) {
        return reject(discovery, error);
    }
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => return reject(discovery, format!("cannot read plugin.json: {error}")),
    };
    let manifest = match manifest::parse_manifest(&text) {
        Ok(manifest) => manifest,
        Err(error) => return reject(discovery, format!("invalid manifest: {error}")),
    };

    if discovery
        .report
        .plugins
        .iter()
        .any(|entry| entry.status == PluginStatus::Loaded && entry.name == manifest.name)
    {
        discovery.report.plugins.push(PluginReportEntry {
            name: manifest.name.clone(),
            root: crate::paths::display(candidate),
            status: PluginStatus::Shadowed,
            problems: vec![format!(
                "shadowed by a higher-precedence plugin root ({scope:?} root skipped)"
            )],
            skill_count: 0,
            mcp_server_count: 0,
        });
        return;
    }

    let mut problems: Vec<String> = manifest.warnings.clone();

    let skills = if components.loads_skills() {
        discover_plugin_skills(&manifest.name, &root, &mut problems)
    } else {
        Vec::new()
    };

    let (mcp_servers, invalid_mcp_servers) = if components.loads_mcp() {
        load_mcp_component(&manifest, &root, plugins_root, &mut problems)
    } else {
        (Vec::new(), Vec::new())
    };

    if components == ComponentSelection::All
        && skills.is_empty()
        && mcp_servers.is_empty()
        && invalid_mcp_servers.is_empty()
    {
        problems.push("plugin has no usable components".to_string());
    }

    let skill_count = skills.len();
    let mcp_server_count = mcp_servers.len();
    discovery.skills.extend(skills);
    discovery.mcp.servers.extend(
        mcp_servers
            .into_iter()
            .map(|(name, server)| (format!("{}/{name}", manifest.name), server)),
    );
    discovery.mcp.invalid_servers.extend(invalid_mcp_servers);
    discovery.report.plugins.push(PluginReportEntry {
        name: manifest.name,
        root: crate::paths::display(candidate),
        status: PluginStatus::Loaded,
        problems,
        skill_count,
        mcp_server_count,
    });
}

fn discover_plugin_skills(
    plugin_name: &str,
    root: &Path,
    problems: &mut Vec<String>,
) -> Vec<Skill> {
    let mut skills = Vec::new();
    let skills_location = root.join("skills");
    if !skills_location.exists() {
        return skills;
    }
    // A present location with the wrong filesystem kind invalidates only the
    // skills component type (spec §6.2).
    let skills_dir = if skills_location.is_dir() {
        match contain::contained_path(root, &skills_location) {
            Ok(dir) => dir,
            Err(error) => {
                problems.push(format!("skills component invalid: {error}"));
                return skills;
            }
        }
    } else {
        problems.push("skills location exists but is not a directory".to_string());
        return skills;
    };

    // Immediate child directories only; never recurse for nested skills.
    let Ok(entries) = std::fs::read_dir(&skills_dir) else {
        return skills;
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    children.sort();

    for child in children {
        let skill_name = child
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| child.display().to_string());
        let skill_md = child.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let skill_md = match contain::contained_path(root, &skill_md) {
            Ok(path) => path,
            Err(error) => {
                problems.push(format!("skipping skill `{skill_name}`: {error}"));
                continue;
            }
        };
        let contents = match std::fs::read_to_string(&skill_md) {
            Ok(contents) => contents,
            Err(error) => {
                problems.push(format!(
                    "skipping skill `{skill_name}`: cannot read SKILL.md: {error}"
                ));
                continue;
            }
        };
        let source = SkillSource::plugin(skill_md.clone(), plugin_name.to_string());
        match crate::skills::parse_skill(&contents, source, Some(&skill_md)) {
            Ok(skill) => skills.push(skill),
            Err(error) => problems.push(format!("skipping skill `{skill_name}`: {error}")),
        }
    }

    skills
}

fn load_mcp_component(
    manifest: &manifest::PluginManifest,
    root: &Path,
    plugins_root: &Path,
    problems: &mut Vec<String>,
) -> (Vec<(String, McpServerConfig)>, Vec<InvalidMcpServer>) {
    let mcp_path = root.join("mcp.json");
    if !mcp_path.exists() {
        return (Vec::new(), Vec::new());
    }
    if !mcp_path.is_file() {
        problems.push("mcp.json exists but is not a regular file".to_string());
        return (Vec::new(), Vec::new());
    }
    let mcp_path = match contain::contained_path(root, &mcp_path) {
        Ok(path) => path,
        Err(error) => {
            problems.push(format!("MCP component invalid: {error}"));
            return (Vec::new(), Vec::new());
        }
    };
    let text = match std::fs::read_to_string(&mcp_path) {
        Ok(text) => text,
        Err(error) => {
            problems.push(format!("cannot read mcp.json: {error}"));
            return (Vec::new(), Vec::new());
        }
    };

    let storage_root = match contain::canonical_root(plugins_root) {
        Ok(root) => root,
        Err(error) => {
            problems.push(format!("MCP component invalid: {error}"));
            return (Vec::new(), Vec::new());
        }
    };
    let data_tail = format!("data/{}", manifest.name);
    let data_dir = match contain::resolve_in_root(&storage_root, &data_tail) {
        Ok(path) => path,
        Err(error) => {
            problems.push(format!("MCP component invalid: {error}"));
            return (Vec::new(), Vec::new());
        }
    };
    let outcome =
        mcp_adapter::load_plugin_mcp(&text, &manifest.name, root, &storage_root, &data_dir);
    if let Some(reason) = outcome.disabled_reason {
        problems.push(format!("MCP disabled for plugin: {reason}"));
    }
    for invalid in &outcome.invalid {
        problems.push(format!(
            "invalid MCP server `{}`: {}",
            invalid.identity, invalid.error
        ));
    }
    for skipped in &outcome.skipped_unsupported {
        problems.push(format!(
            "skipping MCP server `{skipped}`: transport not supported by this Rho build"
        ));
    }
    (outcome.servers, outcome.invalid)
}

/// Emit diagnostics for one plugin load pass.
pub(crate) fn log(report: &PluginLoadReport) {
    for entry in &report.plugins {
        match entry.status {
            PluginStatus::Rejected => {
                for problem in &entry.problems {
                    tracing::warn!(
                        plugin = %entry.name,
                        root = %entry.root,
                        problem = %problem,
                        "rejecting Agent Plugin; supported components: {SUPPORTED_COMPONENTS}"
                    );
                }
            }
            PluginStatus::Shadowed => {
                tracing::warn!(
                    plugin = %entry.name,
                    root = %entry.root,
                    "Agent Plugin shadowed by a higher-precedence plugin root"
                );
            }
            PluginStatus::Loaded => {
                for problem in &entry.problems {
                    tracing::warn!(
                        plugin = %entry.name,
                        problem = %problem,
                        "Agent Plugin component problem"
                    );
                }
                if entry.problems.is_empty() {
                    tracing::debug!(
                        plugin = %entry.name,
                        skills = entry.skill_count,
                        mcp_servers = entry.mcp_server_count,
                        "loaded Agent Plugin"
                    );
                }
            }
        }
    }
}

impl PluginLoadReport {
    pub(crate) fn summary(&self) -> PluginLoadSummary {
        let mut summary = PluginLoadSummary {
            discovered: !self.plugins.is_empty(),
            ..PluginLoadSummary::default()
        };
        for entry in &self.plugins {
            match entry.status {
                PluginStatus::Loaded => {
                    summary.loaded += 1;
                    summary.problems += entry.problems.len();
                }
                PluginStatus::Rejected => summary.rejected += 1,
                PluginStatus::Shadowed => {}
            }
            summary.skills += entry.skill_count;
            summary.mcp_servers += entry.mcp_server_count;
        }
        summary
    }
}
