//! Git-sourced install and update for Agent Plugin packages.
//!
//! Fetches land in a temporary checkout ([`git_source::fetch`]), then reuse the
//! local copy path in the parent module. Update refetches the recorded ref and
//! replaces the package only when the commit moved.

use std::path::Path;

use super::{
    inspect_source, managed_root, package_at, place, resolve_named_package, InstallMode,
    ManagedPackage, ResolvedPackage, SourcePackage,
};
use crate::plugins::{
    git_source::{self, Checkout, GitSource},
    state::{GitRecord, InstallRecord, PluginOrigin, PluginScope, PluginStateStore},
};

/// Result of `rho plugins update` for one package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateOutcome {
    /// The fetched commit replaced `previous`; `package.git` holds the new commit.
    Updated { previous: String },
    /// The recorded commit is still current.
    UpToDate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdatedPackage {
    pub(crate) package: ManagedPackage,
    pub(crate) outcome: UpdateOutcome,
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
    let root = managed_root(scope, cwd, home);
    let destination = place(&package, &root, InstallMode::Copy, force)?;

    let record = GitRecord::new(source, checkout.commit.clone());
    let mut state = PluginStateStore::load(cwd, rho_home)?;
    state.record_install(
        scope,
        &package.manifest.name,
        InstallRecord::Git(record.clone()),
    )?;
    Ok(ManagedPackage {
        name: package.manifest.name,
        version: package.manifest.version,
        description: package.manifest.description,
        scope,
        origin: PluginOrigin::Git,
        path: destination,
        link_target: None,
        git: Some(record),
    })
}

/// Refetch a git-installed package's recorded ref and replace it when the
/// commit moved. Enablement is preserved.
pub(crate) fn update(
    name: &str,
    cwd: &Path,
    home: &Path,
    rho_home: Option<&Path>,
) -> anyhow::Result<UpdatedPackage> {
    let resolved = resolve_named_package(name, cwd, Some(home), rho_home)?;
    update_resolved(resolved, cwd, rho_home)
}

/// Update every git-installed package recorded in project and user state.
///
/// Each package reports its own result so one failed fetch does not stop the
/// rest.
pub(crate) fn update_all(
    cwd: &Path,
    home: &Path,
    rho_home: Option<&Path>,
) -> anyhow::Result<Vec<(String, anyhow::Result<UpdatedPackage>)>> {
    let state = PluginStateStore::load(cwd, rho_home)?;
    let mut results = Vec::new();
    for scope in [PluginScope::Project, PluginScope::User] {
        let root = managed_root(scope, cwd, home);
        for name in state.git_installed(scope) {
            let result = package_at(scope, &root.join(&name), &state).and_then(|package| {
                let resolved = ResolvedPackage {
                    package,
                    root: root.clone(),
                };
                update_resolved(resolved, cwd, rho_home)
            });
            results.push((name, result));
        }
    }
    Ok(results)
}

fn update_resolved(
    resolved: ResolvedPackage,
    cwd: &Path,
    rho_home: Option<&Path>,
) -> anyhow::Result<UpdatedPackage> {
    let ResolvedPackage { package, root } = resolved;
    let Some(record) = package.git.clone() else {
        anyhow::bail!(
            "`{}` has {} origin, not git; reinstall it with `rho plugins install <source> --force`",
            package.name,
            package.origin.as_str()
        );
    };
    let source = record.source();
    let checkout = git_source::fetch(&source)?;
    if checkout.commit == record.commit {
        return Ok(UpdatedPackage {
            package,
            outcome: UpdateOutcome::UpToDate,
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
    let mut state = PluginStateStore::load(cwd, rho_home)?;
    state.record_git_update(package.scope, &package.name, updated.clone())?;
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
        },
    })
}

/// Validate the package directory inside a fresh checkout.
fn inspect_checkout(checkout: &Checkout, source: &GitSource) -> anyhow::Result<SourcePackage> {
    let dir = checkout.package_dir(source)?;
    if !dir.join("plugin.json").is_file() {
        let commit = git_source::short_commit(&checkout.commit);
        match &source.subdir {
            Some(subdir) => anyhow::bail!(
                "no plugin.json at {subdir} in {} at {commit}",
                source.url
            ),
            None => anyhow::bail!(
                "no plugin.json at the root of {} at {commit}; point at the plugin directory with a /tree/<ref>/<path> URL",
                source.url
            ),
        }
    }
    inspect_source(&dir)
}

#[cfg(test)]
#[path = "manage_git_tests.rs"]
mod tests;
