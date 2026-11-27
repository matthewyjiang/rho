//! The agent plugins a session would load, and managing them without a
//! session: what `rho plugins` lists, installs, links, updates, enables,
//! disables, and removes.
//!
//! Every method acts on the daemon's machine: an install from a URL runs
//! `git` there, and a path names a directory there. A change applies to
//! sessions opened afterwards; a loaded session keeps the plugins it
//! loaded. A daemon that predates these methods answers
//! [`crate::ErrorCode::MethodNotFound`].

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ClientEnvironment;

/// List the agent plugins a session in the params' workspace would find,
/// as `rho plugins list` lists them: those of the workspace's project
/// roots, nearest first, then the user's. Each is listed whether it loads,
/// is disabled, stays inactive in a project the client does not trust, is
/// shadowed by a same-named plugin of a nearer root, or is rejected for a
/// broken `plugin.json`. Each request reads the plugin roots again.
pub const PLUGINS_LIST: &str = "rho/plugins/list";

/// Install a plugin package into the user's or the workspace project's
/// plugins root, as `rho plugins install` and `rho plugins link` do: copy a
/// local directory, symlink one, or fetch a git repository and copy its
/// package. The package starts enabled. A plugin of the same name already
/// in that root is [`crate::ErrorCode::PluginExists`] unless the params
/// force replacing it. A git install is answered once `git` finishes.
pub const PLUGINS_INSTALL: &str = "rho/plugins/install";

/// Fetch the recorded git source of git-installed plugins again and replace
/// each whose commit moved, as `rho plugins update` does, keeping whether
/// it is enabled. One plugin's failure does not stop the rest. Project
/// plugins are fetched only when the client trusts the project. Answered
/// once every fetch finishes.
pub const PLUGINS_UPDATE: &str = "rho/plugins/update";

/// Enable or disable a plugin, as `rho plugins enable` and `rho plugins
/// disable` do. The choice is saved in its scope's `plugins.toml`
/// ([`Plugin::state_path`]); a disabled plugin's files stay in place.
pub const PLUGINS_SET_ENABLED: &str = "rho/plugins/set_enabled";

/// Remove a plugin's package from its plugins root, and its record from its
/// scope's `plugins.toml`, as `rho plugins remove --yes` does: the request
/// is the confirmation, so a client asks its user first. A symlinked
/// package loses only its link. The plugin's `data/<name>` directory is
/// left in place.
pub const PLUGINS_REMOVE: &str = "rho/plugins/remove";

/// Params of [`PLUGINS_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsListParams {
    /// The directory a session would open in, whose project's plugins are
    /// listed with the user's. Absent, the daemon user's home directory,
    /// which leaves only the user's plugins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// The asking client's environment, whose `RHO_TRUST_PROJECT_PLUGINS`
    /// decides whether the project's plugins load, as for a session that
    /// client opens. Absent, they do not: they are listed
    /// [`PluginStatus::Untrusted`].
    #[serde(default, skip_serializing_if = "ClientEnvironment::is_empty")]
    pub environment: ClientEnvironment,
}

impl PluginsListParams {
    /// Builds it from each of its fields.
    pub fn new(workspace: Option<PathBuf>, environment: ClientEnvironment) -> Self {
        Self {
            workspace,
            environment,
        }
    }
}

/// Result of [`PLUGINS_LIST`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsListResponse {
    /// Every plugin found, in the order a session loads them: the project's
    /// roots, nearest first, then the user's; within a root, by directory
    /// name.
    pub plugins: Vec<Plugin>,
}

impl PluginsListResponse {
    /// Builds it from each of its fields.
    pub fn new(plugins: Vec<Plugin>) -> Self {
        Self { plugins }
    }
}

/// One plugin package found in a plugins root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct Plugin {
    /// Its manifest's name; for a [`PluginStatus::Rejected`] package whose
    /// manifest did not load, its directory's name.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub scope: PluginScope,
    /// The package's directory in its plugins root, or the symlink there
    /// for a linked package.
    pub path: PathBuf,
    pub source: PluginSource,
    /// Whether its scope's `plugins.toml` leaves it enabled.
    pub enabled: bool,
    /// The `plugins.toml` that saves whether it is enabled and how it was
    /// installed: the daemon's `~/.rho/plugins.toml` (or `$RHO_HOME`'s) for
    /// a user plugin, the project's `.rho/plugins.toml` for a project one.
    /// It may not exist yet.
    pub state_path: PathBuf,
    pub status: PluginStatus,
    /// Why it was rejected, for a [`PluginStatus::Rejected`] package: its
    /// `plugin.json` is missing, unreadable, invalid, or resolves outside
    /// the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Problems that leave the rest of the plugin usable, such as a skill
    /// or MCP server that did not load or a manifest warning; empty when
    /// clean.
    pub problems: Vec<String>,
    /// The names of the skills it brings. Empty for a shadowed or rejected
    /// package, whose components are not read.
    pub skills: Vec<String>,
    /// The names of the MCP servers its `mcp.json` defines; a session
    /// qualifies each as `<plugin>/<server>`, as
    /// [`super::ConfiguredMcpServer::identity`].
    pub mcp_servers: Vec<String>,
    /// The file names of the Rho agent definitions it ships; the agent
    /// catalog reads each and decides the agent it defines.
    pub agent_files: Vec<String>,
}

impl Plugin {
    /// Builds it from each of its fields.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        version: Option<String>,
        description: Option<String>,
        scope: PluginScope,
        path: PathBuf,
        source: PluginSource,
        enabled: bool,
        state_path: PathBuf,
        status: PluginStatus,
        error: Option<String>,
        problems: Vec<String>,
        skills: Vec<String>,
        mcp_servers: Vec<String>,
        agent_files: Vec<String>,
    ) -> Self {
        Self {
            name,
            version,
            description,
            scope,
            path,
            source,
            enabled,
            state_path,
            status,
            error,
            problems,
            skills,
            mcp_servers,
            agent_files,
        }
    }
}

/// Whose plugins root a plugin is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginScope {
    /// The daemon user's `~/.agents/plugins`, which always loads.
    User,
    /// A `.agents/plugins` of the workspace's project, which loads only
    /// when the client trusts the project.
    Project,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How a plugin's package came to be in its plugins root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginSource {
    /// Placed there by hand or by another tool; Rho has no record of it.
    Local,
    /// Copied from a local directory by an install.
    Install,
    /// Symlinked to `target`, a local directory, by an install with
    /// [`PluginInstallSource::Link`].
    Link { target: PathBuf },
    /// Copied from the git repository at `url` by an install;
    /// [`PLUGINS_UPDATE`] fetches it again.
    Git {
        url: String,
        /// The branch, tag, or commit installed; absent, the repository's
        /// default branch.
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        /// The package's directory in the repository; absent, its root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// The commit the package was copied from.
        commit: String,
    },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Whether a session loads a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginStatus {
    /// A session loads its skills, MCP servers, and agents.
    Loaded,
    /// Disabled in its scope's `plugins.toml`, so a session loads none of
    /// it; it still keeps a same-named plugin of a later root from loading.
    Disabled,
    /// A project plugin the client does not trust the project for
    /// (`RHO_TRUST_PROJECT_PLUGINS`), so a session loads none of it. Its
    /// components are still listed.
    Untrusted,
    /// Its `plugin.json` did not load; [`Plugin::error`] says why.
    Rejected,
    /// A plugin of the same name in a nearer root loads or is disabled, so
    /// this one is skipped.
    Shadowed,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The plugin a request acts on: a row of [`PLUGINS_LIST`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginTarget {
    /// Its [`Plugin::name`].
    pub name: String,
    /// Its [`Plugin::scope`]. In project scope the nearest project root
    /// that holds a plugin of that name is the one acted on.
    pub scope: PluginScope,
}

impl PluginTarget {
    /// Builds it from each of its fields.
    pub fn new(name: String, scope: PluginScope) -> Self {
        Self { name, scope }
    }
}

/// Params of [`PLUGINS_INSTALL`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsInstallParams {
    pub source: PluginInstallSource,
    /// The plugins root to install into: the daemon user's, or the
    /// workspace project's, which needs a workspace.
    pub scope: PluginScope,
    /// Replace a plugin of the same name already in that root. Even then, a
    /// directory there without a `plugin.json` is never replaced.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
    /// The directory a session would open in, whose project a project-scope
    /// install goes to. Absent, the daemon user's home directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// The asking client's environment, whose project trust the returned
    /// [`Plugin::status`] reflects. Installing into a project the client
    /// does not trust is allowed; the plugin stays inactive there.
    #[serde(default, skip_serializing_if = "ClientEnvironment::is_empty")]
    pub environment: ClientEnvironment,
}

impl PluginsInstallParams {
    /// Builds it from each of its fields.
    pub fn new(
        source: PluginInstallSource,
        scope: PluginScope,
        force: bool,
        workspace: Option<PathBuf>,
        environment: ClientEnvironment,
    ) -> Self {
        Self {
            source,
            scope,
            force,
            workspace,
            environment,
        }
    }
}

/// Where [`PLUGINS_INSTALL`] reads a package from. The daemon reads it, so
/// it has no `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginInstallSource {
    /// Copy the package directory at `path`, an absolute path on the
    /// daemon's machine.
    Path { path: PathBuf },
    /// Symlink the package directory at `path`, an absolute path on the
    /// daemon's machine, so edits there apply to new sessions without
    /// installing again.
    Link { path: PathBuf },
    /// Fetch the repository at `url` with the daemon machine's `git`, then
    /// copy its package. `url` is what `rho plugins install` takes: an
    /// `https://`, `http://`, `ssh://`, `git://`, `file://`, or `git@`
    /// remote, whose default branch's root holds the package, or a GitHub
    /// link, whose `/tree/<ref>/<path>` names the ref and the package's
    /// directory. A URL with credentials in it is refused.
    Git { url: String },
}

/// Result of [`PLUGINS_INSTALL`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsInstallResponse {
    /// The installed plugin as [`PLUGINS_LIST`] now lists it; a nearer
    /// root's plugin of the same name leaves it
    /// [`PluginStatus::Shadowed`].
    pub plugin: Plugin,
}

impl PluginsInstallResponse {
    /// Builds it from each of its fields.
    pub fn new(plugin: Plugin) -> Self {
        Self { plugin }
    }
}

/// Params of [`PLUGINS_UPDATE`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsUpdateParams {
    /// The plugin to update, which must have been installed from git.
    /// Absent, every git-installed plugin of the user and of the
    /// workspace's project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginTarget>,
    /// The directory a session would open in, whose project's plugins are
    /// updated. Absent, the daemon user's home directory, which leaves only
    /// the user's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// The asking client's environment, whose `RHO_TRUST_PROJECT_PLUGINS`
    /// decides whether the project's plugins are fetched: a project's
    /// plugin records arrive with the repository, so they are fetched only
    /// for a client that trusts it.
    #[serde(default, skip_serializing_if = "ClientEnvironment::is_empty")]
    pub environment: ClientEnvironment,
}

impl PluginsUpdateParams {
    /// Builds it from each of its fields.
    pub fn new(
        plugin: Option<PluginTarget>,
        workspace: Option<PathBuf>,
        environment: ClientEnvironment,
    ) -> Self {
        Self {
            plugin,
            workspace,
            environment,
        }
    }
}

/// Result of [`PLUGINS_UPDATE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsUpdateResponse {
    /// Each plugin fetched, project plugins first; empty when none was
    /// installed from git.
    pub results: Vec<PluginUpdate>,
    /// The names of the git-installed project plugins left alone because
    /// the client does not trust the project.
    pub untrusted: Vec<String>,
}

impl PluginsUpdateResponse {
    /// Builds it from each of its fields.
    pub fn new(results: Vec<PluginUpdate>, untrusted: Vec<String>) -> Self {
        Self { results, untrusted }
    }
}

/// What [`PLUGINS_UPDATE`] did to one plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginUpdate {
    pub plugin: PluginTarget,
    pub outcome: PluginUpdateOutcome,
}

impl PluginUpdate {
    /// Builds it from each of its fields.
    pub fn new(plugin: PluginTarget, outcome: PluginUpdateOutcome) -> Self {
        Self { plugin, outcome }
    }
}

/// How updating one plugin ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginUpdateOutcome {
    /// Its ref moved from commit `previous` to `current`, whose package
    /// replaced it.
    Updated { previous: String, current: String },
    /// Its ref still points at `commit`, the one installed.
    UpToDate { commit: String },
    /// It could not be updated and was left as it was. `code` is the
    /// [`crate::ErrorCode`] that stopped it, such as
    /// [`crate::ErrorCode::PluginFetchFailed`], and `message` says why. An
    /// update of one named plugin reports its fetch this way too; the
    /// request fails only when nothing can be fetched.
    Failed { code: i64, message: String },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Params of [`PLUGINS_SET_ENABLED`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsSetEnabledParams {
    pub plugin: PluginTarget,
    pub enabled: bool,
    /// The directory a session would open in, whose project's plugins a
    /// project-scope target is among. Absent, the daemon user's home
    /// directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// The asking client's environment, whose project trust the returned
    /// [`Plugin::status`] reflects. Enabling a project plugin the client
    /// does not trust is allowed; it stays inactive there.
    #[serde(default, skip_serializing_if = "ClientEnvironment::is_empty")]
    pub environment: ClientEnvironment,
}

impl PluginsSetEnabledParams {
    /// Builds it from each of its fields.
    pub fn new(
        plugin: PluginTarget,
        enabled: bool,
        workspace: Option<PathBuf>,
        environment: ClientEnvironment,
    ) -> Self {
        Self {
            plugin,
            enabled,
            workspace,
            environment,
        }
    }
}

/// Result of [`PLUGINS_SET_ENABLED`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsSetEnabledResponse {
    /// The plugin as [`PLUGINS_LIST`] now lists it.
    pub plugin: Plugin,
}

impl PluginsSetEnabledResponse {
    /// Builds it from each of its fields.
    pub fn new(plugin: Plugin) -> Self {
        Self { plugin }
    }
}

/// Params of [`PLUGINS_REMOVE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsRemoveParams {
    pub plugin: PluginTarget,
    /// The directory a session would open in, whose project's plugins a
    /// project-scope target is among. Absent, the daemon user's home
    /// directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
}

impl PluginsRemoveParams {
    /// Builds it from each of its fields.
    pub fn new(plugin: PluginTarget, workspace: Option<PathBuf>) -> Self {
        Self { plugin, workspace }
    }
}

/// Result of [`PLUGINS_REMOVE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginsRemoveResponse {
    /// Where the removed package was.
    pub path: PathBuf,
}

impl PluginsRemoveResponse {
    /// Builds it from each of its fields.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

/// The `data` of a [`crate::ErrorCode::PluginExists`] error: the plugin
/// already in the plugins root an install would replace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PluginConflict {
    /// The name of the package being installed, which the plugin there
    /// shares.
    pub name: String,
    /// The plugin there.
    pub path: PathBuf,
}

impl PluginConflict {
    /// Builds it from each of its fields.
    pub fn new(name: String, path: PathBuf) -> Self {
        Self { name, path }
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
