//! Installation evidence shared by update and uninstall. Policy stays with callers.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScoopInstallScope {
    User,
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ManagedInstallation {
    Cargo { root: Option<PathBuf> },
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

pub(crate) fn detect(path: &Path) -> InstallationEvidence {
    let cargo_metadata = cargo_root_from_bin_path(path).is_some_and(|root| {
        root.join(".crates.toml").exists() || root.join(".crates2.json").exists()
    });
    let mut pacman_owned = false;
    let managed = detect_with(
        path,
        std::env::var("SCOOP_GLOBAL").ok(),
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
    scoop_global: Option<String>,
    cargo_owns: impl FnOnce(&Path) -> bool,
    pacman_owns: impl FnOnce(&Path) -> bool,
) -> Option<ManagedInstallation> {
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
    scoop_install_scope_for_path(path, scoop_global).map(ManagedInstallation::Scoop)
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

fn scoop_install_scope_for_path(
    path: &Path,
    global_roots: impl IntoIterator<Item = impl AsRef<str>>,
) -> Option<ScoopInstallScope> {
    let lower = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if !(lower.contains("/scoop/apps/rho/")
        || lower.ends_with("/scoop/shims/rho")
        || lower.ends_with("/scoop/shims/rho.exe"))
    {
        return None;
    }
    for root in global_roots {
        let root = root
            .as_ref()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase();
        if !root.is_empty() && (lower == root || lower.starts_with(&format!("{root}/"))) {
            return Some(ScoopInstallScope::Global);
        }
    }
    if lower.contains("/programdata/scoop/") {
        return Some(ScoopInstallScope::Global);
    }
    Some(ScoopInstallScope::User)
}

#[cfg(test)]
#[path = "installation_tests.rs"]
mod tests;
