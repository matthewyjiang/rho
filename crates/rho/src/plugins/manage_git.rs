//! Git-sourced install and update for Agent Plugin packages.
//!
//! Fetches land in a temporary checkout ([`git_source::fetch`]), then reuse the
//! local install path in the parent module. Update walks the git sources
//! recorded in `plugins.toml`, refetches each recorded ref, and replaces a
//! package only when the commit moved.

use std::path::{Path, PathBuf};

use super::{
    inspect_source, install_validated, managed_root, package_exists, place, resolve_named_package,
    InstallMode, ManagedPackage, SourcePackage,
};
use crate::plugins::{
    git_source::{self, Checkout, GitSource},
    state::{
        project_plugins_root, same_location, user_plugins_root, InstallRecord, PluginScope,
        PluginStateStore,
    },
    ProjectTrust, TRUST_PROJECT_PLUGINS_ENV,
};

/// Which git-installed packages `rho plugins update` refreshes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateSelection<'a> {
    All,
    Named(&'a str),
}

/// Result of `rho plugins update` for one package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateOutcome {
    /// The package was replaced with the newer `current` commit.
    Updated { previous: String, current: String },
    /// The recorded commit is still what the ref points at.
    UpToDate { commit: String },
}

/// One package `update` attempted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdateTarget {
    pub(crate) name: String,
    pub(crate) scope: PluginScope,
}

#[derive(Debug, Default)]
pub(crate) struct UpdateReport {
    /// Per-package results; one failed fetch does not stop the rest.
    pub(crate) results: Vec<(UpdateTarget, anyhow::Result<UpdateOutcome>)>,
    /// Project packages left alone because the workspace is not trusted.
    pub(crate) untrusted: Vec<String>,
}

/// A package installed from git, with the commit that was copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitInstall {
    pub(crate) package: ManagedPackage,
    pub(crate) commit: String,
}

/// Fetch a git source and copy its package into a managed root.
pub(crate) fn install_git(
    source: &GitSource,
    scope: PluginScope,
    force: bool,
    cwd: &Path,
    home: &Path,
    rho_home: Option<&Path>,
) -> anyhow::Result<GitInstall> {
    let root = managed_root(scope, cwd, home)?;
    let checkout = git_source::fetch(source)?;
    let package = inspect_checkout(&checkout, source)?;
    let record = InstallRecord::Git(source.record(checkout.commit.clone()));
    let package = install_validated(package, &record, scope, &root, force, cwd, rho_home)?;
    Ok(GitInstall {
        package,
        commit: checkout.commit,
    })
}

/// Refetch recorded git sources and replace packages whose ref moved.
///
/// Enablement is preserved. Project sources arrive with cloned repositories,
/// so they are fetched only in trusted workspaces.
pub(crate) fn update(
    selection: UpdateSelection<'_>,
    trust: ProjectTrust,
    cwd: &Path,
    home: &Path,
    rho_home: Option<&Path>,
) -> anyhow::Result<UpdateReport> {
    let mut state = PluginStateStore::load(cwd, rho_home)?;
    let mut report = UpdateReport::default();
    for (scope, root) in update_scopes(&state, cwd, home) {
        for name in state.git_installed(scope) {
            if matches!(selection, UpdateSelection::Named(wanted) if wanted != name) {
                continue;
            }
            if scope == PluginScope::Project && !trust.is_trusted() {
                report.untrusted.push(name);
                continue;
            }
            let result = update_one(scope, &root, &name, &mut state);
            report.results.push((UpdateTarget { name, scope }, result));
        }
    }

    if let UpdateSelection::Named(name) = selection {
        if !report.results.is_empty() {
            return Ok(report);
        }
        if !report.untrusted.is_empty() {
            anyhow::bail!(
                "`{name}` is a project plugin and the workspace is not trusted; set {TRUST_PROJECT_PLUGINS_ENV}=1 to update it"
            );
        }
        let resolved = resolve_named_package(name, cwd, Some(home), rho_home)?;
        anyhow::bail!(
            "`{name}` has {} origin, not git; reinstall it from a repository with `rho plugins install <url> --force`",
            resolved.package.origin.as_str()
        );
    }
    Ok(report)
}

/// Scopes to walk with their plugins roots, dropping project scope when it
/// would revisit user records or packages.
///
/// Running from `$HOME` outside a repository makes the project root `$HOME`:
/// its plugins root is the user root, and with the default `RHO_HOME` both
/// scopes also load `~/.rho/plugins.toml`. Walking both would update each
/// package twice and misreport it as untrusted. A `RHO_HOME` inside the
/// project root shares the state file without sharing the plugins root.
fn update_scopes(state: &PluginStateStore, cwd: &Path, home: &Path) -> Vec<(PluginScope, PathBuf)> {
    let project = project_plugins_root(cwd, home)
        .filter(|_| !same_location(&state.project_path, &state.user_path))
        .map(|root| (PluginScope::Project, root));
    project
        .into_iter()
        .chain([(PluginScope::User, user_plugins_root(home))])
        .collect()
}

/// Refetch one recorded source into `root`, the plugins root for `scope`.
/// Works from the state record and the package directory alone, so a
/// corrupted installed manifest is still repairable.
fn update_one(
    scope: PluginScope,
    root: &Path,
    name: &str,
    state: &mut PluginStateStore,
) -> anyhow::Result<UpdateOutcome> {
    let Some(record) = state.git_record(scope, name).cloned() else {
        anyhow::bail!("`{name}` has no recorded git source");
    };
    let source = GitSource::try_from(&record)?;
    let path = root.join(name);
    if !package_exists(&path) {
        anyhow::bail!(
            "`{name}` was installed from {} but {} is missing; reinstall it with `rho plugins install`",
            source.describe(),
            crate::paths::display(&path)
        );
    }

    let checkout = git_source::fetch(&source)?;
    if checkout.commit == record.commit {
        return Ok(UpdateOutcome::UpToDate {
            commit: record.commit,
        });
    }

    let fetched = inspect_checkout(&checkout, &source)?;
    if fetched.manifest.name != name {
        anyhow::bail!(
            "refusing to update `{name}`: {} now contains package `{}`",
            source.describe(),
            fetched.manifest.name
        );
    }
    place(&fetched, root, InstallMode::Copy, /* force */ true)?;
    state.record_git_update(scope, name, source.record(checkout.commit.clone()))?;
    Ok(UpdateOutcome::Updated {
        previous: record.commit,
        current: checkout.commit,
    })
}

/// Validate the package directory inside a fresh checkout.
fn inspect_checkout(checkout: &Checkout, source: &GitSource) -> anyhow::Result<SourcePackage> {
    let dir = checkout.package_dir(source)?;
    if !dir.join("plugin.json").is_file() {
        let commit = git_source::short_commit(&checkout.commit);
        let location = source.subdir().unwrap_or("the repository root");
        let hint = if source.subdir().is_none() && source.is_github() {
            "; link the plugin directory with a /tree/<ref>/<path> URL"
        } else {
            ""
        };
        anyhow::bail!(
            "no plugin.json at {location} in {} at {commit}{hint}",
            source.url()
        );
    }
    inspect_source(&dir)
}

#[cfg(test)]
#[path = "manage_git_tests.rs"]
mod tests;
