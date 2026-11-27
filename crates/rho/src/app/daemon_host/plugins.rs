//! `rho/plugins/*`: the agent plugins a session would find, and installing,
//! updating, enabling, disabling, and removing them on the daemon's
//! machine. Each method runs the `plugins::manage` function its `rho
//! plugins` command runs, so the daemon keeps every guard the CLI has
//! (containment in the managed roots, git without hooks or credentials in
//! URLs, no package code run), on a blocking thread, and git never asks
//! for credentials. A git fetch holds up no other request but the changes
//! queued behind it, which run one at a time.
//!
//! As for `rho/mcp/list`, the params' workspace picks the project and the
//! client's environment its trust; without a workspace the daemon user's
//! home stands in, which leaves only user plugins. A change applies to
//! sessions opened afterwards.

use std::path::{Path, PathBuf};

use rho_protocol::{
    ErrorCode, Plugin, PluginConflict, PluginInstallSource, PluginScope as WireScope, PluginSource,
    PluginStatus as WireStatus, PluginTarget, PluginUpdate, PluginUpdateOutcome,
    PluginsInstallParams, PluginsInstallResponse, PluginsListParams, PluginsListResponse,
    PluginsRemoveParams, PluginsRemoveResponse, PluginsSetEnabledParams, PluginsSetEnabledResponse,
    PluginsUpdateParams, PluginsUpdateResponse, RpcError,
};

use crate::plugins::{
    discover_with_trust,
    git_source::{GitPrompts, GitSource},
    manage::{
        self, InstallMode, ManageError, ManagedPackage, NamedPlugin, UpdateOutcome, UpdateSelection,
    },
    PluginOrigin, PluginReportEntry, PluginScope, PluginStatus, ProjectTrust,
};

/// The daemon's `rho/plugins/*` answers.
pub(super) struct Plugins {
    /// The daemon's Rho data root, whose `plugins.toml` holds user plugin
    /// state, as `rho plugins` run with the daemon's environment reads it.
    rho_home: PathBuf,
    /// Held by each method that changes plugins, so two clients' installs
    /// of one name cannot swap packages under each other. Listing does not
    /// wait for it.
    changing: tokio::sync::Mutex<()>,
}

impl Plugins {
    pub(super) fn new(rho_home: PathBuf) -> Self {
        Self {
            rho_home,
            changing: tokio::sync::Mutex::new(()),
        }
    }

    /// Answers `rho/plugins/list`.
    pub(super) async fn list(
        &self,
        params: PluginsListParams,
    ) -> Result<PluginsListResponse, RpcError> {
        let place = Place::parse(params.workspace.as_deref(), &params.environment)?;
        let rho_home = self.rho_home.clone();
        blocking(move || {
            let plugins = place.discover(&rho_home).into_iter().map(wire_plugin);
            Ok(PluginsListResponse::new(plugins.collect()))
        })
        .await
    }

    /// Handles `rho/plugins/install`.
    pub(super) async fn install(
        &self,
        params: PluginsInstallParams,
    ) -> Result<PluginsInstallResponse, RpcError> {
        let place = Place::parse(params.workspace.as_deref(), &params.environment)?;
        let scope = place.managed_scope(params.scope)?;
        let source = RequestedSource::parse(params.source)?;
        let force = params.force;
        let rho_home = self.rho_home.clone();
        let _changing = self.changing.lock().await;
        blocking(move || {
            let home = place.home()?;
            let (cwd, rho_home_ref) = (place.cwd.as_path(), Some(rho_home.as_path()));
            let package = match &source {
                RequestedSource::Local(path, mode) => {
                    manage::install(path, scope, *mode, force, cwd, home, rho_home_ref)
                }
                RequestedSource::Git(git) => manage::install_git(
                    git,
                    GitPrompts::Never,
                    scope,
                    force,
                    cwd,
                    home,
                    rho_home_ref,
                )
                .map(|installed| installed.package),
            }
            .map_err(rpc_error)?;
            Ok(PluginsInstallResponse::new(
                place.listed(&package, &rho_home)?,
            ))
        })
        .await
    }

    /// Handles `rho/plugins/update`.
    pub(super) async fn update(
        &self,
        params: PluginsUpdateParams,
    ) -> Result<PluginsUpdateResponse, RpcError> {
        let place = Place::parse(params.workspace.as_deref(), &params.environment)?;
        let target = params.plugin.map(Target::parse).transpose()?;
        let rho_home = self.rho_home.clone();
        let _changing = self.changing.lock().await;
        blocking(move || {
            let home = place.home()?;
            let selection = match &target {
                Some(target) => UpdateSelection::Named(target.named()),
                None => UpdateSelection::All,
            };
            let report = manage::update(
                selection,
                place.trust,
                GitPrompts::Never,
                &place.cwd,
                home,
                Some(&rho_home),
            )
            .map_err(rpc_error)?;
            let results = report.results.into_iter().map(|(target, result)| {
                let outcome = match result {
                    Ok(UpdateOutcome::Updated { previous, current }) => {
                        PluginUpdateOutcome::Updated { previous, current }
                    }
                    Ok(UpdateOutcome::UpToDate { commit }) => {
                        PluginUpdateOutcome::UpToDate { commit }
                    }
                    Err(error) => {
                        let error = rpc_error(error);
                        PluginUpdateOutcome::Failed {
                            code: error.code,
                            message: error.message,
                        }
                    }
                };
                PluginUpdate::new(
                    PluginTarget::new(target.name, wire_scope(target.scope)),
                    outcome,
                )
            });
            Ok(PluginsUpdateResponse::new(
                results.collect(),
                report.untrusted,
            ))
        })
        .await
    }

    /// Handles `rho/plugins/set_enabled`.
    pub(super) async fn set_enabled(
        &self,
        params: PluginsSetEnabledParams,
    ) -> Result<PluginsSetEnabledResponse, RpcError> {
        let place = Place::parse(params.workspace.as_deref(), &params.environment)?;
        let target = Target::parse(params.plugin)?;
        let enabled = params.enabled;
        let rho_home = self.rho_home.clone();
        let _changing = self.changing.lock().await;
        blocking(move || {
            let package = manage::set_enabled(
                target.named(),
                enabled,
                &place.cwd,
                place.home.as_deref(),
                Some(&rho_home),
            )
            .map_err(rpc_error)?;
            Ok(PluginsSetEnabledResponse::new(
                place.listed(&package, &rho_home)?,
            ))
        })
        .await
    }

    /// Handles `rho/plugins/remove`.
    pub(super) async fn remove(
        &self,
        params: PluginsRemoveParams,
    ) -> Result<PluginsRemoveResponse, RpcError> {
        let place = Place::parse(params.workspace.as_deref(), &Default::default())?;
        let target = Target::parse(params.plugin)?;
        let rho_home = self.rho_home.clone();
        let _changing = self.changing.lock().await;
        blocking(move || {
            let package = manage::remove(
                target.named(),
                &place.cwd,
                place.home.as_deref(),
                Some(&rho_home),
            )
            .map_err(rpc_error)?;
            Ok(PluginsRemoveResponse::new(
                crate::paths::without_verbatim_prefix(package.path),
            ))
        })
        .await
    }
}

/// Where a request acts: the directory a session would open in, the
/// daemon user's home, and the plugin trust the client grants.
struct Place {
    cwd: PathBuf,
    /// Whether the params named the workspace `cwd` is, rather than the
    /// home standing in for one.
    workspace: bool,
    home: Option<PathBuf>,
    trust: ProjectTrust,
}

impl Place {
    fn parse(
        workspace: Option<&Path>,
        environment: &rho_protocol::ClientEnvironment,
    ) -> Result<Self, RpcError> {
        let (workspace, trusts) = super::configured::parse(workspace, environment)?;
        let home = crate::paths::home_dir();
        let cwd = workspace
            .clone()
            .or_else(|| home.clone())
            .ok_or_else(no_home)?;
        Ok(Self {
            cwd,
            workspace: workspace.is_some(),
            home,
            trust: trusts.plugins,
        })
    }

    fn home(&self) -> Result<&Path, RpcError> {
        self.home.as_deref().ok_or_else(no_home)
    }

    /// The scope an install goes to: a project scope needs a workspace.
    fn managed_scope(&self, scope: WireScope) -> Result<PluginScope, RpcError> {
        let scope = parse_scope(scope)?;
        if scope == PluginScope::Project && !self.workspace {
            return Err(invalid_params(
                "a project-scope install needs a workspace".to_owned(),
            ));
        }
        Ok(scope)
    }

    /// Every plugin a session here finds, as `rho plugins list` lists them.
    fn discover(&self, rho_home: &Path) -> Vec<PluginReportEntry> {
        discover_with_trust(&self.cwd, self.home.as_deref(), Some(rho_home), self.trust)
            .report
            .plugins
    }

    /// `package` as [`Self::discover`] now lists it.
    fn listed(&self, package: &ManagedPackage, rho_home: &Path) -> Result<Plugin, RpcError> {
        self.discover(rho_home)
            .into_iter()
            .find(|entry| entry.scope == package.scope && entry.path == package.path)
            .map(wire_plugin)
            .ok_or_else(|| {
                RpcError::new(
                    ErrorCode::InternalError,
                    format!(
                        "plugin `{}` at {} is not in its plugins root after the change",
                        package.name,
                        crate::paths::display(&package.path)
                    ),
                )
            })
    }
}

/// An install's source, checked before anything is written.
enum RequestedSource {
    Local(PathBuf, InstallMode),
    Git(GitSource),
}

impl RequestedSource {
    fn parse(source: PluginInstallSource) -> Result<Self, RpcError> {
        let local = |path: PathBuf, mode| {
            // Never resolved against the daemon's own directory.
            if !path.is_absolute() {
                return Err(invalid_params(format!(
                    "plugin path {} is not absolute",
                    path.display()
                )));
            }
            Ok(Self::Local(path, mode))
        };
        match source {
            PluginInstallSource::Path { path } => local(path, InstallMode::Copy),
            PluginInstallSource::Link { path } => local(path, InstallMode::Link),
            PluginInstallSource::Git { url } => GitSource::parse(&url)
                .map(Self::Git)
                .map_err(|error| invalid_params(format!("{error:#}"))),
            _ => Err(invalid_params("unknown plugin source".to_owned())),
        }
    }
}

/// A request's [`PluginTarget`], its scope known to this daemon.
struct Target {
    name: String,
    scope: PluginScope,
}

impl Target {
    fn parse(target: PluginTarget) -> Result<Self, RpcError> {
        Ok(Self {
            scope: parse_scope(target.scope)?,
            name: target.name,
        })
    }

    fn named(&self) -> NamedPlugin<'_> {
        NamedPlugin::in_scope(&self.name, self.scope)
    }
}

fn parse_scope(scope: WireScope) -> Result<PluginScope, RpcError> {
    match scope {
        WireScope::User => Ok(PluginScope::User),
        WireScope::Project => Ok(PluginScope::Project),
        _ => Err(invalid_params(format!("unknown plugin scope {scope:?}"))),
    }
}

fn wire_scope(scope: PluginScope) -> WireScope {
    match scope {
        PluginScope::User => WireScope::User,
        PluginScope::Project => WireScope::Project,
    }
}

/// `entry` as the protocol lists it.
fn wire_plugin(entry: PluginReportEntry) -> Plugin {
    let source = match (entry.origin, entry.git) {
        (PluginOrigin::Git, Some(git)) => PluginSource::Git {
            url: git.url,
            git_ref: git.git_ref,
            path: git.subdir,
            commit: git.commit,
        },
        (PluginOrigin::Link, _) => PluginSource::Link {
            target: entry.link_target.unwrap_or_else(|| entry.path.clone()),
        },
        // Git origin comes only from a git record, so it always has one.
        (PluginOrigin::Install | PluginOrigin::Git, _) => PluginSource::Install,
        (PluginOrigin::Local, _) => PluginSource::Local,
    };
    let status = match entry.status {
        PluginStatus::Loaded => WireStatus::Loaded,
        PluginStatus::Disabled => WireStatus::Disabled,
        PluginStatus::Untrusted => WireStatus::Untrusted,
        PluginStatus::Rejected => WireStatus::Rejected,
        PluginStatus::Shadowed => WireStatus::Shadowed,
    };
    // A rejected package's only problem is its rejection, which the wire
    // carries in `error`.
    let problems = match entry.rejection {
        Some(_) => Vec::new(),
        None => entry.problems,
    };
    Plugin::new(
        entry.name,
        entry.version,
        entry.description,
        wire_scope(entry.scope),
        crate::paths::without_verbatim_prefix(entry.path),
        source,
        entry.enabled,
        crate::paths::without_verbatim_prefix(entry.state_path),
        status,
        entry.rejection,
        problems,
        entry.skill_names,
        entry.mcp_server_names,
        entry.agent_files,
    )
}

/// `error` as the `rho/plugins/*` error a client tells apart by its code.
fn rpc_error(error: ManageError) -> RpcError {
    let code = match &error {
        ManageError::NotFound(_) => ErrorCode::PluginNotFound,
        ManageError::Exists { name, path } => {
            let data = PluginConflict::new(
                name.clone(),
                crate::paths::without_verbatim_prefix(path.clone()),
            );
            return RpcError::new(
                ErrorCode::PluginExists,
                format!(
                    "plugin `{name}` already exists at {}; install again with `force` to replace it",
                    crate::paths::display(path)
                ),
            )
            .with_data(serde_json::to_value(data).unwrap_or_default());
        }
        ManageError::Untrusted { .. } => ErrorCode::PluginUntrusted,
        ManageError::InvalidPackage(_) => ErrorCode::PluginInvalid,
        ManageError::Fetch(_) => ErrorCode::PluginFetchFailed,
        ManageError::Refused(_) => ErrorCode::InvalidParams,
        ManageError::Io(_) | ManageError::Other(_) => ErrorCode::InternalError,
    };
    RpcError::new(code, format!("{error:#}"))
}

fn invalid_params(message: String) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

fn no_home() -> RpcError {
    RpcError::new(
        ErrorCode::InternalError,
        "could not determine the daemon user's home directory",
    )
}

/// Runs `work` on a blocking thread: plugin roots are read and written, and
/// `git` run, synchronously.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(work).await.map_err(|error| {
        RpcError::new(
            ErrorCode::InternalError,
            format!("plugin task failed: {error}"),
        )
    })?
}
