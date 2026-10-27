//! mise install evidence: where mise keeps tool versions and which tool owns one.
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

/// Installs directories named by mise's environment overrides, plus their
/// resolved forms, since the running executable's path is already resolved.
/// The default layouts are matched by [`mise_install_path`] without them.
pub(super) fn mise_installs_roots() -> Vec<PathBuf> {
    let installs = std::env::var_os("MISE_INSTALLS_DIR").map(PathBuf::from);
    let data = std::env::var_os("MISE_DATA_DIR").map(|data| PathBuf::from(data).join("installs"));
    installs
        .into_iter()
        .chain(data)
        .filter(|root| !root.as_os_str().is_empty())
        .flat_map(|root| {
            let resolved = std::fs::canonicalize(&root)
                .ok()
                .filter(|real| *real != root);
            std::iter::once(root).chain(resolved)
        })
        .collect()
}

/// The `<installs>/<tool>/<version>` directory holding `path`, under a
/// configured root or the default `…/mise/installs` layout
/// (`~/.local/share/mise`, `%LOCALAPPDATA%\mise`).
pub(super) fn mise_install_path(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let components: Vec<_> = path.components().collect();
    let default_root = components
        .windows(2)
        .rposition(|pair| pair[0].as_os_str() == "mise" && pair[1].as_os_str() == "installs")
        .map(|installs| components[..installs + 2].iter().collect::<PathBuf>());
    roots.iter().cloned().chain(default_root).find_map(|root| {
        let dir = tool_version_dir(path.strip_prefix(&root).ok()?)?;
        Some(root.join(dir))
    })
}

/// `<tool>/<version>` of a path relative to an installs root. The binary must
/// sit inside the version directory.
fn tool_version_dir(rest: &Path) -> Option<PathBuf> {
    let mut parts = rest.components();
    let (Some(Component::Normal(tool)), Some(Component::Normal(version)), Some(_)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    Some(Path::new(tool).join(version))
}

/// The mise tool (`rho`, `github:matthewyjiang/rho`, …) whose installed version
/// lives at `install_path`, as `mise ls --installed --json` names it. Errors
/// say why the lookup failed: mise missing, mise failing, or no match.
pub(crate) fn mise_tool(install_path: &Path) -> Result<String> {
    let output = std::process::Command::new("mise")
        .args(["ls", "--installed", "--json"])
        .output()
        .context("could not run `mise ls --installed --json`")?;
    if !output.status.success() {
        bail!(
            "`mise ls --installed --json` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    mise_tool_from_ls_json(&output.stdout, install_path)
}

fn mise_tool_from_ls_json(json: &[u8], install_path: &Path) -> Result<String> {
    #[derive(Deserialize)]
    struct InstalledVersion {
        install_path: PathBuf,
    }
    let tools: BTreeMap<String, Vec<InstalledVersion>> =
        serde_json::from_slice(json).context("could not parse `mise ls --installed --json`")?;
    // mise may report the path through a symlink the running executable resolved.
    let resolved = std::fs::canonicalize(install_path).ok();
    let matches = |path: &Path| {
        path == install_path
            || resolved.is_some()
                && std::fs::canonicalize(path).ok().as_deref() == resolved.as_deref()
    };
    tools
        .into_iter()
        .find_map(|(tool, versions)| {
            versions
                .iter()
                .any(|version| matches(&version.install_path))
                .then_some(tool)
        })
        .with_context(|| format!("`mise ls --installed` does not list {install_path:?}"))
}

#[cfg(test)]
#[path = "installation_mise_tests.rs"]
mod tests;
