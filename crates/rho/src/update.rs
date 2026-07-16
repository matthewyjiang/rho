#[cfg(not(windows))]
use std::process::Stdio;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(not(windows))]
use anyhow::Context;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};
use serde::Deserialize;
#[cfg(not(windows))]
use tokio::process::Command;

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/matthewyjiang/rho/releases/latest";
const CRATE_NAME: &str = "rho-coding-agent";
const PACMAN_PACKAGE_TARGET: &str = "mjiang-extras/rho-coding-agent";
const SCOOP_PACKAGE: &str = "rho";
#[cfg(not(windows))]
const SCRIPT_INSTALL_SH_COMMAND: &str = "tmp=$(mktemp) || exit; curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.sh -o \"$tmp\"; status=$?; if [ $status -eq 0 ]; then sh \"$tmp\"; status=$?; fi; rm -f \"$tmp\"; exit $status";
#[cfg(windows)]
const SCRIPT_INSTALL_PS1_COMMAND: &str =
    "irm https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1 | iex";
#[cfg(windows)]
const SCRIPT_INSTALL_PS1_DISPLAY_COMMAND: &str = "powershell -NoProfile -ExecutionPolicy Bypass -Command \"irm https://raw.githubusercontent.com/matthewyjiang/rho/main/scripts/install.ps1 | iex\"";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub latest_tag: String,
    pub latest_version: String,
    pub current_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallMethod {
    Cargo,
    Pacman,
    Scoop,
    ScoopGlobal,
    Script,
}

impl InstallMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cargo => "Cargo",
            Self::Pacman => "pacman",
            Self::Scoop => "Scoop",
            Self::ScoopGlobal => "Scoop (global)",
            Self::Script => "install script",
        }
    }

    pub fn update_command(self) -> String {
        match self {
            Self::Cargo => cargo_update_command_display(),
            Self::Pacman => pacman_update_command_display(),
            Self::Scoop => scoop_update_command_display(ScoopInstallScope::User),
            Self::ScoopGlobal => scoop_update_command_display(ScoopInstallScope::Global),
            Self::Script => script_update_command_display(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScoopInstallScope {
    User,
    Global,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

pub async fn available_update(current_version: &str) -> anyhow::Result<Option<UpdateInfo>> {
    let latest_tag = latest_release_tag().await?;
    let Some(latest_version) = release_tag_to_version(&latest_tag) else {
        anyhow::bail!("latest release tag '{latest_tag}' does not contain a version");
    };
    if version_is_newer(&latest_version, current_version) {
        Ok(Some(UpdateInfo {
            latest_tag,
            latest_version,
            current_version: current_version.to_string(),
        }))
    } else {
        Ok(None)
    }
}

pub async fn update_notice(current_version: &str) -> Option<String> {
    match tokio::time::timeout(
        Duration::from_millis(900),
        available_update(current_version),
    )
    .await
    {
        Ok(Ok(Some(update))) => Some(format!(
            "update available: v{} (current v{}). run `rho update` to {} via {}.",
            update.latest_version,
            update.current_version,
            update_action_label(),
            detect_install_method().label()
        )),
        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => None,
    }
}

pub async fn run_update(current_version: &str) -> anyhow::Result<()> {
    let method = detect_install_method();
    println!("detected install method: {}", method.label());

    match available_update(current_version).await {
        Ok(Some(update)) => {
            println!(
                "rho v{} is available (current v{}).",
                update.latest_version, update.current_version
            );
        }
        Ok(None) => {
            println!("rho is up to date (v{current_version}).");
            return Ok(());
        }
        Err(err) => {
            eprintln!("warning: could not check latest release: {err}");
            println!("continuing with {} update command.", method.label());
        }
    }

    println!("update command: {}", method.update_command());
    if method == InstallMethod::Pacman {
        println!("pacman may prompt for your sudo password.");
    }

    run_update_command(method).await
}

async fn run_update_command(method: InstallMethod) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        println!(
            "automatic updates are disabled on Windows to avoid launching background shells that can trigger security software."
        );
        println!("copy and run this command yourself to update:");
        println!("{}", method.update_command());
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let status = update_command(method)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .with_context(|| format!("failed to run {} update command", method.label()))?;

        if !status.success() {
            anyhow::bail!("{} update command exited with {status}", method.label());
        }
        Ok(())
    }
}

#[cfg(windows)]
fn update_action_label() -> &'static str {
    "show the update command"
}

#[cfg(not(windows))]
fn update_action_label() -> &'static str {
    "update"
}

#[cfg(not(windows))]
fn update_command(method: InstallMethod) -> Command {
    match method {
        InstallMethod::Cargo => {
            let mut command = Command::new("cargo");
            command.args(["install", CRATE_NAME, "--locked"]);
            if let Some(root) = current_cargo_update_root() {
                command.arg("--root").arg(root);
            }
            command
        }
        InstallMethod::Pacman => {
            let mut command = Command::new("sudo");
            command.args(["pacman", "-Sy", PACMAN_PACKAGE_TARGET]);
            command
        }
        InstallMethod::Scoop | InstallMethod::ScoopGlobal => {
            let mut command = Command::new("sh");
            command.args(["-c", &method.update_command()]);
            command
        }
        InstallMethod::Script => script_update_command(),
    }
}

fn cargo_update_command_display() -> String {
    let mut command = format!("cargo install {CRATE_NAME} --locked");
    if let Some(root) = current_cargo_update_root() {
        command.push_str(" --root ");
        command.push_str(&shell_quote_path(&root));
    }
    command
}

fn pacman_update_command_display() -> String {
    format!("sudo pacman -Sy {PACMAN_PACKAGE_TARGET}")
}

fn scoop_update_command_display(scope: ScoopInstallScope) -> String {
    // Refresh Scoop/buckets first so a just-published release is visible even when
    // Scoop's own outdated check would still skip a bucket sync.
    match scope {
        ScoopInstallScope::User => format!("scoop update; scoop update {SCOOP_PACKAGE}"),
        ScoopInstallScope::Global => {
            format!("scoop update; scoop update -g {SCOOP_PACKAGE}")
        }
    }
}

#[cfg(windows)]
fn script_update_command_display() -> String {
    let Some(install_dir) = current_exe_parent() else {
        return SCRIPT_INSTALL_PS1_DISPLAY_COMMAND.to_string();
    };
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -Command {command}",
        command = powershell_quote(&format!(
            "$env:RHO_INSTALL_DIR={}; {SCRIPT_INSTALL_PS1_COMMAND}",
            powershell_quote_path(&install_dir)
        ))
    )
}

#[cfg(not(windows))]
fn script_update_command_display() -> String {
    let command = format!("sh -c {}", shell_quote(SCRIPT_INSTALL_SH_COMMAND));
    let Some(install_dir) = current_exe_parent() else {
        return command;
    };
    format!(
        "RHO_INSTALL_DIR={} {command}",
        shell_quote_path(&install_dir)
    )
}

#[cfg(not(windows))]
fn script_update_command() -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", SCRIPT_INSTALL_SH_COMMAND]);
    if let Some(install_dir) = current_exe_parent() {
        command.env("RHO_INSTALL_DIR", install_dir);
    }
    command
}

pub fn detect_install_method() -> InstallMethod {
    if let Ok(method) = std::env::var("RHO_INSTALL_METHOD") {
        match method.trim().to_ascii_lowercase().as_str() {
            "cargo" => return InstallMethod::Cargo,
            "pacman" => return InstallMethod::Pacman,
            "scoop" => return InstallMethod::Scoop,
            "scoop-global" | "scoop_global" => return InstallMethod::ScoopGlobal,
            "script" | "install-script" => return InstallMethod::Script,
            _ => {}
        }
    }

    let current_exe = std::env::current_exe().ok();
    if current_exe
        .as_deref()
        .is_some_and(|path| is_cargo_bin_path(path) || is_cargo_installed_at_root(path))
    {
        return InstallMethod::Cargo;
    }
    if current_exe.as_deref().is_some_and(is_pacman_owned) {
        return InstallMethod::Pacman;
    }
    if let Some(scope) = current_exe.as_deref().and_then(scoop_install_scope) {
        return match scope {
            ScoopInstallScope::User => InstallMethod::Scoop,
            ScoopInstallScope::Global => InstallMethod::ScoopGlobal,
        };
    }
    InstallMethod::Script
}

fn current_exe_parent() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

fn current_cargo_update_root() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    cargo_update_root_for_exe(&current_exe, cargo_install_root_contains_crate)
}

fn cargo_update_root_for_exe(
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
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.contains("/.cargo/bin/")
}

fn cargo_root_from_bin_path(path: &Path) -> Option<PathBuf> {
    let bin_dir = path.parent()?;
    (bin_dir.file_name()? == "bin").then(|| bin_dir.parent().map(Path::to_path_buf))?
}

fn is_cargo_installed_at_root(path: &Path) -> bool {
    cargo_update_root_for_exe(path, cargo_install_root_contains_crate).is_some()
}

fn cargo_install_root_contains_crate(root: &Path) -> bool {
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
        .any(|line| line.split_whitespace().next() == Some(CRATE_NAME))
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote(&path.to_string_lossy())
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(windows)]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn powershell_quote_path(path: &Path) -> String {
    powershell_quote(&path.to_string_lossy())
}

#[cfg(target_os = "linux")]
fn is_pacman_owned(path: &Path) -> bool {
    std::process::Command::new("pacman")
        .arg("-Qqo")
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|owner| owner.trim().contains("rho"))
}

#[cfg(not(target_os = "linux"))]
fn is_pacman_owned(_path: &Path) -> bool {
    false
}

fn scoop_install_scope(path: &Path) -> Option<ScoopInstallScope> {
    scoop_install_scope_for_path(path, scoop_global_roots_from_env())
}

fn scoop_global_roots_from_env() -> Vec<String> {
    std::env::var("SCOOP_GLOBAL")
        .ok()
        .into_iter()
        .filter(|root| !root.trim().is_empty())
        .collect()
}

fn scoop_install_scope_for_path(
    path: &Path,
    global_roots: impl IntoIterator<Item = impl AsRef<str>>,
) -> Option<ScoopInstallScope> {
    let lower = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if !is_scoop_rho_path(&lower) {
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
    // Default global Scoop root is %ProgramData%\scoop.
    if lower.contains("/programdata/scoop/") {
        return Some(ScoopInstallScope::Global);
    }
    Some(ScoopInstallScope::User)
}

fn is_scoop_rho_path(lower_path: &str) -> bool {
    lower_path.contains("/scoop/apps/rho/")
        || lower_path.ends_with("/scoop/shims/rho")
        || lower_path.ends_with("/scoop/shims/rho.exe")
}

async fn latest_release_tag() -> anyhow::Result<String> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("rho-coding-agent"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()?;
    let release = client
        .get(LATEST_RELEASE_URL)
        .send()
        .await?
        .error_for_status()?
        .json::<LatestRelease>()
        .await?;
    Ok(release.tag_name)
}

fn release_tag_to_version(tag: &str) -> Option<String> {
    let version = tag
        .rsplit_once('v')
        .map(|(_, version)| version)
        .unwrap_or(tag)
        .trim();
    parse_version(version)
        .is_some()
        .then(|| version.to_string())
}

fn version_is_newer(candidate: &str, current: &str) -> bool {
    let Some(candidate) = parse_version(candidate) else {
        return false;
    };
    let Some(current) = parse_version(current) else {
        return false;
    };
    candidate > current
}

fn parse_version(version: &str) -> Option<Vec<u64>> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let parts = core
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!parts.is_empty()).then_some(parts)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        cargo_install_list_contains_crate, cargo_root_from_bin_path, cargo_update_root_for_exe,
        pacman_update_command_display, release_tag_to_version, scoop_install_scope_for_path,
        scoop_update_command_display, version_is_newer, InstallMethod, ScoopInstallScope,
    };

    #[test]
    fn extracts_release_please_tag_version() {
        assert_eq!(
            release_tag_to_version("rho-coding-agent-v0.12.3").as_deref(),
            Some("0.12.3")
        );
    }

    #[test]
    fn compares_dotted_versions() {
        assert!(version_is_newer("0.12.3", "0.12.1"));
        assert!(version_is_newer("0.13.0", "0.12.9"));
        assert!(!version_is_newer("0.12.1", "0.12.1"));
        assert!(!version_is_newer("0.12.0", "0.12.1"));
    }

    #[test]
    fn script_update_command_display_uses_platform_installer() {
        let command = InstallMethod::Script.update_command();

        #[cfg(windows)]
        {
            assert!(command.contains("powershell"));
            assert!(command.contains("install.ps1"));
            assert!(!command.contains("install.sh"));
        }

        #[cfg(not(windows))]
        {
            assert!(command.contains("sh -c"));
            assert!(command.contains("install.sh"));
            assert!(!command.contains("install.ps1"));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn script_update_command_display_preserves_curl_failure_status() {
        let command = InstallMethod::Script.update_command();

        assert!(command.contains("curl"));
        assert!(command.contains("--proto"));
        assert!(command.contains("-o"));
        assert!(command.contains("$tmp"));
        assert!(command.contains("exit $status"));
        assert!(!command.contains("| sh"));
    }

    #[test]
    fn pacman_update_command_syncs_only_rho_from_mjiang_extras() {
        assert_eq!(
            pacman_update_command_display(),
            "sudo pacman -Sy mjiang-extras/rho-coding-agent"
        );
    }

    #[test]
    fn scoop_update_command_refreshes_buckets_then_updates_rho() {
        assert_eq!(
            scoop_update_command_display(ScoopInstallScope::User),
            "scoop update; scoop update rho"
        );
        assert_eq!(
            InstallMethod::Scoop.update_command(),
            "scoop update; scoop update rho"
        );
        assert_eq!(InstallMethod::Scoop.label(), "Scoop");
    }

    #[test]
    fn scoop_global_update_command_uses_global_flag() {
        assert_eq!(
            scoop_update_command_display(ScoopInstallScope::Global),
            "scoop update; scoop update -g rho"
        );
        assert_eq!(
            InstallMethod::ScoopGlobal.update_command(),
            "scoop update; scoop update -g rho"
        );
        assert_eq!(InstallMethod::ScoopGlobal.label(), "Scoop (global)");
    }

    #[test]
    fn detects_user_and_global_scoop_install_paths() {
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"C:\Users\me\scoop\apps\rho\current\rho.exe"),
                None::<&str>,
            ),
            Some(ScoopInstallScope::User)
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"C:\Users\me\scoop\apps\rho\0.26.0\rho.exe"),
                None::<&str>,
            ),
            Some(ScoopInstallScope::User)
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"C:\Users\me\scoop\shims\rho.exe"),
                None::<&str>,
            ),
            Some(ScoopInstallScope::User)
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"C:\ProgramData\scoop\apps\rho\current\rho.exe"),
                None::<&str>,
            ),
            Some(ScoopInstallScope::Global)
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"C:\ProgramData\scoop\shims\rho.exe"),
                None::<&str>,
            ),
            Some(ScoopInstallScope::Global)
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"D:\tools\apps\rho\current\rho.exe"),
                [r"D:\tools"],
            ),
            None
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"D:\tools\scoop\apps\rho\current\rho.exe"),
                [r"D:\tools\scoop"],
            ),
            Some(ScoopInstallScope::Global)
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"C:\Users\me\AppData\Local\Programs\rho\bin\rho.exe"),
                None::<&str>,
            ),
            None
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"C:\Users\me\scoop\apps\git\current\bin\git.exe"),
                None::<&str>,
            ),
            None
        );
        assert_eq!(
            scoop_install_scope_for_path(
                Path::new(r"C:\Users\me\.cargo\bin\rho.exe"),
                None::<&str>,
            ),
            None
        );
    }

    #[test]
    fn detects_cargo_root_from_parent_bin_directory() {
        let exe = Path::new("/opt/rho/bin/rho");

        assert_eq!(
            cargo_root_from_bin_path(exe).as_deref(),
            Some(Path::new("/opt/rho"))
        );
        assert_eq!(
            cargo_update_root_for_exe(exe, |root| root == Path::new("/opt/rho")).as_deref(),
            Some(Path::new("/opt/rho"))
        );
        assert!(cargo_update_root_for_exe(exe, |_| false).is_none());
        assert!(
            cargo_update_root_for_exe(Path::new("/home/me/.cargo/bin/rho"), |_| true).is_none()
        );
    }

    #[test]
    fn detects_crate_in_cargo_install_list_output() {
        let output = "ripgrep v14.1.1:\n    rg\nrho-coding-agent v0.12.3:\n    rho\n";

        assert!(cargo_install_list_contains_crate(output));
        assert!(!cargo_install_list_contains_crate(
            "rho-helper v0.1.0:\n    rho-helper\n"
        ));
    }
}
