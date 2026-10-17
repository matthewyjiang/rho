//! Git-sourced install and update for Agent Plugin packages.
//!
//! Fetches land in a temporary checkout ([`git_source::fetch`]), then reuse the
//! local install path in the parent module. Update walks the git sources
//! recorded in `plugins.toml`, refetches each recorded ref, and replaces a
//! package only when the commit moved.

use std::path::Path;

use super::{
    inspect_source, install_validated, managed_root, package_at, package_exists, place,
    resolve_named_package, InstallMode, ManagedPackage, SourcePackage,
};
use crate::plugins::{
    git_source::{self, Checkout, GitSource},
    state::{GitRecord, InstallRecord, PluginScope, PluginStateStore},
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdatedPackage {
    pub(crate) package: ManagedPackage,
    pub(crate) outcome: UpdateOutcome,
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
    pub(crate) results: Vec<(UpdateTarget, anyhow::Result<UpdatedPackage>)>,
    /// Project packages left alone because the workspace is not trusted.
    pub(crate) untrusted: Vec<String>,
}

/// Fetch a git source and copy its package into a managed root.
pub(crate) fn install_git(
    source: &GitSource,
    scope: PluginScope,
    force: bool,
    cwd: &Path,
    home: &Path,
    rho_home: Option<&Path>,
) -> anyhow::Result<ManagedPackage> {
    let checkout = git_source::fetch(source)?;
    let package = inspect_checkout(&checkout, source)?;
    let record = InstallRecord::Git(GitRecord::new(source, checkout.commit.clone()));
    install_validated(package, &record, scope, force, cwd, home, rho_home)
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
    for scope in [PluginScope::Project, PluginScope::User] {
        for name in state.git_installed(scope) {
            if matches!(selection, UpdateSelection::Named(wanted) if wanted != name) {
                continue;
            }
            if scope == PluginScope::Project && !trust.is_trusted() {
                report.untrusted.push(name);
                continue;
            }
            let result = update_one(scope, &name, &mut state, cwd, home);
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

fn update_one(
    scope: PluginScope,
    name: &str,
    state: &mut PluginStateStore,
    cwd: &Path,
    home: &Path,
) -> anyhow::Result<UpdatedPackage> {
    let Some(record) = state.git_record(scope, name).cloned() else {
        anyhow::bail!("`{name}` has no recorded git source");
    };
    let source = record.source();
    let root = managed_root(scope, cwd, home);
    let path = root.join(name);
    if !package_exists(&path) {
        anyhow::bail!(
            "`{name}` was installed from {} but {} is missing; reinstall it with `rho plugins install`",
            source.describe(),
            crate::paths::display(&path)
        );
    }
    let package = package_at(scope, &path, state)?;

    let checkout = git_source::fetch(&source)?;
    if checkout.commit == record.commit {
        return Ok(UpdatedPackage {
            package,
            outcome: UpdateOutcome::UpToDate {
                commit: record.commit,
            },
        });
    }

    let fetched = inspect_checkout(&checkout, &source)?;
    if fetched.manifest.name != package.name {
        anyhow::bail!(
            "refusing to update `{}`: {} now contains package `{}`",
            package.name,
            source.describe(),
            fetched.manifest.name
        );
    }
    let destination = place(&fetched, &root, InstallMode::Copy, /* force */ true)?;

    let updated = GitRecord::new(&source, checkout.commit.clone());
    state.record_git_update(scope, name, updated.clone())?;
    Ok(UpdatedPackage {
        package: ManagedPackage {
            version: fetched.manifest.version,
            description: fetched.manifest.description,
            path: destination,
            git: Some(updated),
            ..package
        },
        outcome: UpdateOutcome::Updated {
            previous: record.commit,
            current: checkout.commit,
        },
    })
}

/// Validate the package directory inside a fresh checkout.
fn inspect_checkout(checkout: &Checkout, source: &GitSource) -> anyhow::Result<SourcePackage> {
    let dir = checkout.package_dir(source)?;
    if !dir.join("plugin.json").is_file() {
        let commit = git_source::short_commit(&checkout.commit);
        let location = match &source.subdir {
            Some(subdir) => subdir.as_str(),
            None => "the repository root",
        };
        let hint = if source.subdir.is_none() && source.is_github() {
            "; link the plugin directory with a /tree/<ref>/<path> URL"
        } else {
            ""
        };
        anyhow::bail!(
            "no plugin.json at {location} in {} at {commit}{hint}",
            source.url
        );
    }
    inspect_source(&dir)
}

#[cfg(test)]
#[path = "manage_git_tests.rs"]
mod tests;
