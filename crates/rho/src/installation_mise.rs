//! mise install evidence: where mise keeps tool versions and which tool owns one.
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
};

/// Candidate installs directories: mise's environment overrides and its
/// platform default, each with its resolved form, since the running
/// executable's path is already resolved and may no longer contain
/// `mise/installs`. Literal `…/mise/installs` paths match without them.
pub(super) fn mise_installs_roots(var: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let set = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let installs = set("MISE_INSTALLS_DIR");
    let data = set("MISE_DATA_DIR").map(|data| data.join("installs"));
    let default_data = if cfg!(windows) {
        set("LOCALAPPDATA").map(|local| local.join("mise"))
    } else {
        set("XDG_DATA_HOME")
            .map(|data| data.join("mise"))
            .or_else(|| set("HOME").map(|home| home.join(".local/share/mise")))
    };
    installs
        .into_iter()
        .chain(data)
        .chain(default_data.map(|data| data.join("installs")))
        .flat_map(|root| {
            let resolved = std::fs::canonicalize(&root)
                .ok()
                .filter(|real| *real != root);
            std::iter::once(root).chain(resolved)
        })
        .collect()
}

/// The `<installs>/<tool>/<version>` directory holding `path`, under one of
/// `roots` or a literal `…/mise/installs` directory.
pub(super) fn mise_install_path(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let components: Vec<_> = path.components().collect();
    let default_root = components
        .windows(2)
        .rposition(|pair| same_name(pair[0], "mise") && same_name(pair[1], "installs"))
        .map(|installs| components[..installs + 2].iter().collect::<PathBuf>());
    roots.iter().cloned().chain(default_root).find_map(|root| {
        let dir = tool_version_dir(strip_root(path, &root)?)?;
        Some(root.join(dir))
    })
}

/// `path` below `root`, ignoring case on Windows, where `current_exe` and
/// environment values often disagree on it.
fn strip_root<'a>(path: &'a Path, root: &Path) -> Option<&'a Path> {
    let mut rest = path.components();
    for part in root.components() {
        if !same_name(rest.next()?, part.as_os_str()) {
            return None;
        }
    }
    Some(rest.as_path())
}

fn same_name(component: Component<'_>, name: impl AsRef<OsStr>) -> bool {
    let (left, right) = (component.as_os_str(), name.as_ref());
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
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
