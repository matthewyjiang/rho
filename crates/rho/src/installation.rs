//! Installation evidence shared by update and uninstall. Policy stays with callers.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScoopInstallScope {
    User,
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ManagedInstallation {
    Cargo {
        root: Option<PathBuf>,
    },
    /// `install_path` is mise's `<installs>/<tool>/<version>` directory holding the binary.
    Mise {
        install_path: PathBuf,
    },
    Pacman,
    Scoop(ScoopInstallScope),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallHint {
    Cargo,
    Pacman,
    Scoop(ScoopInstallScope),
    Script,
    Unknown,
}

pub(crate) fn install_hint(value: &str) -> InstallHint {
    match value.trim().to_ascii_lowercase().as_str() {
        "cargo" => InstallHint::Cargo,
        "pacman" => InstallHint::Pacman,
        "scoop" => InstallHint::Scoop(ScoopInstallScope::User),
        "scoop-global" | "scoop_global" => InstallHint::Scoop(ScoopInstallScope::Global),
        "script" | "install-script" => InstallHint::Script,
        _ => InstallHint::Unknown,
    }
}

pub(crate) struct InstallationEvidence {
    pub managed: Option<ManagedInstallation>,
    /// A Cargo receipt without confirmed ownership cannot authorize deletion.
    /// Update may still use its script fallback when Cargo cannot identify Rho.
    pub cargo_metadata: bool,
    /// Even an unrecognized pacman package owns its executable, not uninstall.
    pub pacman_owned: bool,
}

#[derive(Default)]
struct ScoopRoots {
    user: Option<String>,
    global: Option<String>,
}

pub(crate) fn detect(path: &Path) -> InstallationEvidence {
    let cargo_metadata = cargo_root_from_bin_path(path).is_some_and(|root| {
        root.join(".crates.toml").exists() || root.join(".crates2.json").exists()
    });
    let mut pacman_owned = false;
    let managed = detect_with(
        path,
        &mise_installs_roots(),
        &ScoopRoots {
            user: std::env::var("SCOOP").ok(),
            global: std::env::var("SCOOP_GLOBAL").ok(),
        },
        cargo_install_root_contains_crate,
        |path| {
            let owner = pacman_owner(path);
            pacman_owned = owner.is_some();
            owner.is_some_and(|owner| owner.contains("rho"))
        },
    );
    InstallationEvidence {
        managed,
        cargo_metadata,
        pacman_owned,
    }
}

fn detect_with(
    path: &Path,
    mise_roots: &[PathBuf],
    scoop_roots: &ScoopRoots,
    cargo_owns: impl FnOnce(&Path) -> bool,
    pacman_owns: impl FnOnce(&Path) -> bool,
) -> Option<ManagedInstallation> {
    // Before Cargo: mise's cargo backend installs into `<version>/bin/rho`
    // with a Cargo receipt, but mise owns that directory.
    if let Some(install_path) = mise_install_path(path, mise_roots) {
        return Some(ManagedInstallation::Mise { install_path });
    }
    if is_cargo_bin_path(path) {
        return Some(ManagedInstallation::Cargo {
            root: cargo_root_from_bin_path(path),
        });
    }
    if let Some(root) = cargo_update_root_for_exe(path, cargo_owns) {
        return Some(ManagedInstallation::Cargo { root: Some(root) });
    }
    if pacman_owns(path) {
        return Some(ManagedInstallation::Pacman);
    }
    scoop_install_scope_for_path(path, scoop_roots).map(ManagedInstallation::Scoop)
}

pub(crate) fn cargo_update_root_for_exe(
    path: &Path,
    cargo_root_contains_crate: impl FnOnce(&Path) -> bool,
) -> Option<PathBuf> {
    if is_cargo_bin_path(path) {
        return None;
    }
    let root = cargo_root_from_bin_path(path)?;
    cargo_root_contains_crate(&root).then_some(root)
}

fn is_cargo_bin_path(path: &Path) -> bool {
    path.to_string_lossy()
        .replace('\\', "/")
        .contains("/.cargo/bin/")
}

pub(crate) fn cargo_root_from_bin_path(path: &Path) -> Option<PathBuf> {
    let bin_dir = path.parent()?;
    (bin_dir.file_name()? == "bin").then(|| bin_dir.parent().map(Path::to_path_buf))?
}

pub(crate) fn cargo_install_root_contains_crate(root: &Path) -> bool {
    std::process::Command::new("cargo")
        .args(["install", "--list", "--root"])
        .arg(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|stdout| cargo_install_list_contains_crate(&stdout))
}

fn cargo_install_list_contains_crate(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.split_whitespace().next() == Some("rho-coding-agent"))
}

#[cfg(target_os = "linux")]
fn pacman_owner(path: &Path) -> Option<String> {
    std::process::Command::new("pacman")
        .arg("-Qqo")
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|owner| owner.trim().to_owned())
        .filter(|owner| !owner.is_empty())
}

#[cfg(not(target_os = "linux"))]
fn pacman_owner(_path: &Path) -> Option<String> {
    None
}

/// Installs directories named by mise's environment overrides. The default
/// layouts are matched by [`mise_install_path`] without them.
fn mise_installs_roots() -> Vec<PathBuf> {
    let installs = std::env::var_os("MISE_INSTALLS_DIR").map(PathBuf::from);
    let data = std::env::var_os("MISE_DATA_DIR").map(|data| PathBuf::from(data).join("installs"));
    installs
        .into_iter()
        .chain(data)
        .filter(|root| !root.as_os_str().is_empty())
        .collect()
}

/// The `<installs>/<tool>/<version>` directory holding `path`, under a
/// configured root or the default `…/mise/installs` layout
/// (`~/.local/share/mise`, `%LOCALAPPDATA%\mise`).
fn mise_install_path(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let configured = roots.iter().find_map(|root| {
        let rest = path.strip_prefix(root).ok()?;
        Some(root.join(tool_version_dir(rest)?))
    });
    if configured.is_some() {
        return configured;
    }
    let components: Vec<_> = path.components().collect();
    let installs = components
        .windows(2)
        .rposition(|pair| pair[0].as_os_str() == "mise" && pair[1].as_os_str() == "installs")?;
    let root: PathBuf = components[..installs + 2].iter().collect();
    let rest = path.strip_prefix(&root).ok()?;
    Some(root.join(tool_version_dir(rest)?))
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
/// lives at `install_path`, as `mise ls --installed --json` names it.
pub(crate) fn mise_tool(install_path: &Path) -> Option<String> {
    let output = std::process::Command::new("mise")
        .args(["ls", "--installed", "--json"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    mise_tool_from_ls_json(&output.stdout, install_path)
}

fn mise_tool_from_ls_json(json: &[u8], install_path: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct InstalledVersion {
        install_path: PathBuf,
    }
    let tools: BTreeMap<String, Vec<InstalledVersion>> = serde_json::from_slice(json).ok()?;
    tools.into_iter().find_map(|(tool, versions)| {
        versions
            .iter()
            .any(|version| same_path(&version.install_path, install_path))
            .then_some(tool)
    })
}

// mise may report the path through a symlink the running executable resolved.
fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

fn scoop_install_scope_for_path(path: &Path, roots: &ScoopRoots) -> Option<ScoopInstallScope> {
    let lower = normalized_scoop_path(&path.to_string_lossy());
    // Configured roots need not contain a directory literally named `scoop`.
    // Prefer global when both variables name the same root.
    for (root, scope) in [
        (roots.global.as_deref(), ScoopInstallScope::Global),
        (roots.user.as_deref(), ScoopInstallScope::User),
    ] {
        let Some(root) = root else { continue };
        let root = normalized_scoop_path(root);
        let root = root.trim_end_matches('/');
        if !root.is_empty()
            && lower
                .strip_prefix(&format!("{root}/"))
                .is_some_and(is_scoop_rho_entry)
        {
            return Some(scope);
        }
    }
    // Retain default-layout detection when the launching shell lacks Scoop's
    // environment variables, but require the actual package or shim layout.
    let (root, entry) = lower.rsplit_once("/scoop/")?;
    if !is_scoop_rho_entry(entry) {
        return None;
    }
    Some(if root.ends_with("/programdata") {
        ScoopInstallScope::Global
    } else {
        ScoopInstallScope::User
    })
}

fn normalized_scoop_path(value: &str) -> String {
    let lower = value.replace('\\', "/").to_ascii_lowercase();
    if let Some(unc) = lower.strip_prefix("//?/unc/") {
        format!("//{unc}")
    } else {
        lower.strip_prefix("//?/").unwrap_or(&lower).to_owned()
    }
}

fn is_scoop_rho_entry(entry: &str) -> bool {
    if matches!(entry, "shims/rho" | "shims/rho.exe") {
        return true;
    }
    let Some((version, binary)) = entry
        .strip_prefix("apps/rho/")
        .and_then(|entry| entry.split_once('/'))
    else {
        return false;
    };
    !matches!(version, "" | "." | "..") && matches!(binary, "rho" | "rho.exe")
}

#[cfg(test)]
#[path = "installation_tests.rs"]
mod tests;
