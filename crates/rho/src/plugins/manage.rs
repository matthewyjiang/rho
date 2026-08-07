//! Local install, link, enable, disable, and remove for Agent Plugin packages.
//!
//! These operations validate package shape and rewrite Rho-owned state only.
//! They never execute package code, never touch `data/<plugin>`, and never
//! write outside the chosen managed plugins root.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    contain, discovery_roots,
    manifest::{self, PluginManifest},
    state::{project_plugins_root, user_plugins_root, PluginOrigin, PluginScope, PluginStateStore},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallMode {
    Copy,
    Link,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManagedPackage {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) scope: PluginScope,
    pub(crate) origin: PluginOrigin,
    pub(crate) path: PathBuf,
    pub(crate) link_target: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct SourcePackage {
    pub(crate) path: PathBuf,
    pub(crate) manifest: PluginManifest,
}

struct ResolvedPackage {
    package: ManagedPackage,
    root: PathBuf,
}

/// Validate a local package directory without executing any component code.
pub(crate) fn inspect_source(path: &Path) -> anyhow::Result<SourcePackage> {
    let canonical = contain::canonical_root(path)
        .map_err(|error| anyhow::anyhow!("invalid plugin package: {error}"))?;
    let manifest_path = canonical.join("plugin.json");
    if !manifest_path.is_file() {
        anyhow::bail!(
            "invalid plugin package: {} has no plugin.json",
            crate::paths::display(&canonical)
        );
    }
    let contained = contain::contained_path(&canonical, &manifest_path)
        .map_err(|error| anyhow::anyhow!("invalid plugin package: {error}"))?;
    let text = fs::read_to_string(&contained)?;
    let manifest = manifest::parse_manifest(&text)
        .map_err(|error| anyhow::anyhow!("invalid plugin package: {error}"))?;
    Ok(SourcePackage {
        path: canonical,
        manifest,
    })
}

pub(crate) fn install(
    source: &Path,
    scope: PluginScope,
    mode: InstallMode,
    force: bool,
    cwd: &Path,
    home: &Path,
    rho_home: Option<&Path>,
) -> anyhow::Result<ManagedPackage> {
    let source = inspect_source(source)?;
    let root = managed_root(scope, cwd, home);
    let destination = root.join(&source.manifest.name);
    ensure_destination_replaceable(&destination, &root, force)?;
    fs::create_dir_all(&root)?;
    // Re-check after creating the root so canonicalize can run.
    ensure_path_is_managed_child(&destination, &root)?;

    let staging = unique_sibling_path(&root, &source.manifest.name, "staging")?;
    let staged = (|| -> anyhow::Result<()> {
        match mode {
            InstallMode::Copy => copy_package_tree(&source.path, &staging),
            InstallMode::Link => create_package_link(&source.path, &staging),
        }
    })();
    if let Err(error) = staged {
        let _ = remove_path(&staging);
        return Err(error);
    }

    if let Err(error) = swap_staged_into_destination(&staging, &destination) {
        let _ = remove_path(&staging);
        return Err(error);
    }

    let origin = match mode {
        InstallMode::Copy => PluginOrigin::Install,
        InstallMode::Link => PluginOrigin::Link,
    };
    let link_target = match mode {
        InstallMode::Link => Some(crate::paths::display(&source.path)),
        InstallMode::Copy => None,
    };

    let mut state = PluginStateStore::load(cwd, rho_home)?;
    state.record_install(scope, &source.manifest.name, origin, link_target.clone())?;

    Ok(ManagedPackage {
        name: source.manifest.name,
        version: source.manifest.version,
        description: source.manifest.description,
        scope,
        origin,
        path: destination,
        link_target: link_target.map(PathBuf::from),
    })
}

pub(crate) fn set_enabled(
    name: &str,
    enabled: bool,
    cwd: &Path,
    home: Option<&Path>,
    rho_home: Option<&Path>,
) -> anyhow::Result<ManagedPackage> {
    let resolved = resolve_named_package(name, cwd, home, rho_home)?;
    let mut state = PluginStateStore::load(cwd, rho_home)?;
    // State keys follow the manifest name used by discovery.
    state.set_enabled(resolved.package.scope, &resolved.package.name, enabled)?;
    Ok(resolved.package)
}

pub(crate) fn remove(
    name: &str,
    cwd: &Path,
    home: Option<&Path>,
    rho_home: Option<&Path>,
) -> anyhow::Result<ManagedPackage> {
    let resolved = resolve_named_package(name, cwd, home, rho_home)?;
    ensure_path_is_managed_child(&resolved.package.path, &resolved.root)?;

    let metadata = fs::symlink_metadata(&resolved.package.path)?;
    if metadata.file_type().is_symlink() {
        fs::remove_file(&resolved.package.path)?;
    } else if metadata.is_dir() {
        fs::remove_dir_all(&resolved.package.path)?;
    } else {
        anyhow::bail!(
            "refusing to remove {}: not a plugin directory",
            crate::paths::display(&resolved.package.path)
        );
    }

    let mut state = PluginStateStore::load(cwd, rho_home)?;
    state.clear_package_record(resolved.package.scope, &resolved.package.name)?;
    Ok(resolved.package)
}

fn resolve_named_package(
    name: &str,
    cwd: &Path,
    home: Option<&Path>,
    rho_home: Option<&Path>,
) -> anyhow::Result<ResolvedPackage> {
    manifest::validate_plugin_name(name).map_err(|error| anyhow::anyhow!(error))?;
    let mut matches = Vec::new();
    for (scope, root) in discovery_roots(cwd, home) {
        let candidate = root.join(name);
        if package_exists(&candidate) {
            matches.push((scope, candidate, root));
        }
    }
    // Also match by manifest name when the directory name differs.
    if matches.is_empty() {
        for (scope, root) in discovery_roots(cwd, home) {
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_dir() && !is_symlink(&path) {
                    continue;
                }
                if let Ok(source) = inspect_source(&path) {
                    if source.manifest.name == name {
                        matches.push((scope, path, root.clone()));
                    }
                }
            }
        }
    }

    let state = PluginStateStore::load(cwd, rho_home)
        .unwrap_or_else(|_| PluginStateStore::empty(cwd, rho_home));
    let Some((scope, path, root)) = matches.first() else {
        anyhow::bail!("no plugin named `{name}` in the managed plugin roots");
    };
    if matches.len() > 1 {
        let ignored = matches[1..]
            .iter()
            .map(|(_, path, _)| crate::paths::display(path))
            .collect::<Vec<_>>()
            .join(", ");
        tracing::warn!(
            plugin = %name,
            selected = %crate::paths::display(path),
            ignored = %ignored,
            "multiple managed packages matched; using higher-precedence root"
        );
    }
    Ok(ResolvedPackage {
        package: package_at(*scope, path, &state)?,
        root: root.clone(),
    })
}

fn package_at(
    scope: PluginScope,
    path: &Path,
    state: &PluginStateStore,
) -> anyhow::Result<ManagedPackage> {
    let source = inspect_source(path)?;
    let origin = state.origin(scope, &source.manifest.name, path);
    let link_target = state
        .entry(scope, &source.manifest.name)
        .and_then(|entry| entry.link_target.as_ref().map(PathBuf::from))
        .or_else(|| {
            if origin == PluginOrigin::Link {
                fs::read_link(path).ok()
            } else {
                None
            }
        });
    Ok(ManagedPackage {
        name: source.manifest.name,
        version: source.manifest.version,
        description: source.manifest.description,
        scope,
        origin,
        path: path.to_path_buf(),
        link_target,
    })
}

fn managed_root(scope: PluginScope, cwd: &Path, home: &Path) -> PathBuf {
    match scope {
        PluginScope::User => user_plugins_root(home),
        PluginScope::Project => project_plugins_root(cwd),
    }
}

/// Validate that `destination` may be installed, without destroying it yet.
fn ensure_destination_replaceable(
    destination: &Path,
    root: &Path,
    force: bool,
) -> anyhow::Result<()> {
    ensure_path_is_managed_child(destination, root)?;
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(metadata) => {
            if !force {
                anyhow::bail!(
                    "plugin already exists at {} (pass --force to replace)",
                    crate::paths::display(destination)
                );
            }
            if metadata.file_type().is_symlink() || metadata.is_file() {
                Ok(())
            } else if metadata.is_dir() {
                // Only replace a directory that looks like a plugin package.
                if !destination.join("plugin.json").exists() {
                    anyhow::bail!(
                        "refusing to replace {}: directory has no plugin.json",
                        crate::paths::display(destination)
                    );
                }
                Ok(())
            } else {
                anyhow::bail!(
                    "refusing to replace {}: unsupported file type",
                    crate::paths::display(destination)
                );
            }
        }
    }
}

/// Non-mutating containment check. Does not create directories.
fn ensure_path_is_managed_child(path: &Path, root: &Path) -> anyhow::Result<()> {
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("plugin destination is missing a final path component"))?;
    if file_name == "data" {
        anyhow::bail!("refusing to use the reserved plugin data directory");
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("plugin destination has no parent"))?;

    if !root.exists() {
        // Install into a not-yet-created root: accept exact parent equality.
        if parent != root {
            anyhow::bail!(
                "plugin destination parent {} is not the managed root",
                crate::paths::display(parent)
            );
        }
        return Ok(());
    }

    let canonical_root = fs::canonicalize(root).map_err(|error| {
        anyhow::anyhow!(
            "cannot resolve managed plugins root {}: {error}",
            crate::paths::display(root)
        )
    })?;
    // Compare the parent when the leaf does not exist yet.
    let canonical_parent = if parent.exists() {
        fs::canonicalize(parent)?
    } else if parent == root {
        canonical_root.clone()
    } else {
        anyhow::bail!(
            "plugin destination parent {} is not the managed root",
            crate::paths::display(parent)
        );
    };
    if canonical_parent != canonical_root {
        anyhow::bail!(
            "refusing path outside managed plugins root {}: {}",
            crate::paths::display(&canonical_root),
            crate::paths::display(path)
        );
    }
    Ok(())
}

fn swap_staged_into_destination(staging: &Path, destination: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::rename(staging, destination)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
        Ok(_) => {
            let backup = unique_sibling_path(
                destination.parent().unwrap_or_else(|| Path::new(".")),
                destination
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("plugin"),
                "backup",
            )?;
            fs::rename(destination, &backup).map_err(|error| {
                anyhow::anyhow!(
                    "failed to move existing plugin aside at {}: {error}",
                    crate::paths::display(destination)
                )
            })?;
            if let Err(error) = fs::rename(staging, destination) {
                // Best-effort rollback so a failed swap keeps the prior package.
                let _ = fs::rename(&backup, destination);
                return Err(anyhow::anyhow!(
                    "failed to move staged plugin into {}: {error}",
                    crate::paths::display(destination)
                ));
            }
            let _ = remove_path(&backup);
            Ok(())
        }
    }
}

fn unique_sibling_path(root: &Path, name: &str, kind: &str) -> anyhow::Result<PathBuf> {
    let token = uuid::Uuid::new_v4().simple();
    let path = root.join(format!(".{name}.{kind}.{token}"));
    if path.exists() {
        anyhow::bail!(
            "temporary path already exists: {}",
            crate::paths::display(&path)
        );
    }
    Ok(path)
}

fn remove_path(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path)?;
            Ok(())
        }
        Ok(_) => anyhow::bail!(
            "refusing to remove unsupported path {}",
            crate::paths::display(path)
        ),
    }
}

fn package_exists(path: &Path) -> bool {
    path.join("plugin.json").exists()
        || is_symlink(path) && {
            fs::read_link(path)
                .ok()
                .map(|target| {
                    let resolved = if target.is_absolute() {
                        target
                    } else {
                        path.parent().unwrap_or(path).join(target)
                    };
                    resolved.join("plugin.json").exists()
                })
                .unwrap_or(false)
        }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn create_package_link(source: &Path, destination: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(source, destination).map_err(|error| {
            anyhow::anyhow!(
                "failed to create directory symlink (Windows may require developer mode or elevation): {error}"
            )
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (source, destination);
        anyhow::bail!("plugin linking is not supported on this platform")
    }
}

/// Copy a package tree without following directory symlinks.
fn copy_package_tree(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(destination)?;
    copy_dir_contents(source, destination)
}

fn copy_dir_contents(source: &Path, destination: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir_all(&to)?;
            copy_dir_contents(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&from)?;
            recreate_symlink(&target, &to)?;
        } else {
            anyhow::bail!(
                "refusing to install special file {}",
                crate::paths::display(&from)
            );
        }
    }
    Ok(())
}

fn recreate_symlink(target: &Path, destination: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, destination)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        let metadata_path = if target.is_absolute() {
            target.to_path_buf()
        } else {
            destination
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(target)
        };
        if metadata_path
            .metadata()
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
        {
            std::os::windows::fs::symlink_dir(target, destination)?;
        } else {
            std::os::windows::fs::symlink_file(target, destination)?;
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, destination);
        anyhow::bail!("symlink copy is not supported on this platform")
    }
}

#[cfg(test)]
#[path = "manage_tests.rs"]
mod tests;
