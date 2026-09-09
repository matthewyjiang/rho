#[cfg(not(windows))]
use std::process::Stdio;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::installation::{
    self, cargo_install_root_contains_crate, cargo_update_root_for_exe, InstallHint,
    ManagedInstallation, ScoopInstallScope,
};
#[cfg(not(windows))]
use anyhow::Context;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};
use serde::Deserialize;
#[cfg(not(windows))]
use tokio::process::Command;

const RELEASES_URL: &str = "https://api.github.com/repos/matthewyjiang/rho/releases";
// GitHub documents 100 as the maximum releases page size. Use it so old drafts
// or releases for sibling workspace crates cannot hide the latest app release.
const RELEASES_PER_PAGE: u64 = 100;
const CRATE_NAME: &str = "rho-coding-agent";
const RELEASE_TAG_PREFIX: &str = "rho-coding-agent-v";
const PACMAN_PACKAGE_TARGET: &str = "mjiang-extras/rho-coding-agent";
const SCOOP_PACKAGE: &str = "rho";
/// Shell command that fetches and runs `install.sh` from `git_ref`. Pinned to
/// the release tag being installed so an update runs reviewed, released code
/// rather than whatever `main` holds at that moment.
#[cfg(not(windows))]
fn script_install_sh_command(git_ref: &str) -> String {
    format!(
        "tmp=$(mktemp) || exit; curl --proto '=https' --tlsv1.2 -LsSf \
         https://raw.githubusercontent.com/matthewyjiang/rho/{git_ref}/scripts/install.sh \
         -o \"$tmp\"; status=$?; if [ $status -eq 0 ]; then sh \"$tmp\"; status=$?; fi; \
         rm -f \"$tmp\"; exit $status"
    )
}

/// PowerShell equivalent of [`script_install_sh_command`], pinned to `git_ref`.
#[cfg(windows)]
fn script_install_ps1_command(git_ref: &str) -> String {
    format!(
        "irm https://raw.githubusercontent.com/matthewyjiang/rho/{git_ref}/scripts/install.ps1 | iex"
    )
}

/// Accepts a release tag for interpolation into the installer shell command only
/// when it is a plain git ref. An unexpected tag (metacharacters, spaces) would
/// otherwise inject into `sh -c`, so fall back to `main` rather than run it.
fn install_script_ref(tag: String) -> String {
    let is_plain_ref = !tag.is_empty()
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/'));
    if is_plain_ref {
        tag
    } else {
        "main".to_string()
    }
}

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

    pub fn update_command(self, git_ref: &str) -> String {
        match self {
            Self::Cargo => cargo_update_command_display(),
            Self::Pacman => pacman_update_command_display(),
            Self::Scoop => scoop_update_command_display(ScoopInstallScope::User),
            Self::ScoopGlobal => scoop_update_command_display(ScoopInstallScope::Global),
            Self::Script => script_update_command_display(git_ref),
        }
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn latest_app_release_tag(releases: &[Release]) -> Option<&str> {
    releases
        .iter()
        .find(|release| {
            !release.draft
                && !release.prerelease
                && release_tag_to_version(&release.tag_name).is_some()
        })
        .map(|release| release.tag_name.as_str())
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

    // Pin the install script to the release tag being installed. If the release
    // check fails we cannot know the tag, so fall back to `main` rather than
    // block the update entirely.
    let git_ref = match available_update(current_version).await {
        Ok(Some(update)) => {
            println!(
                "rho v{} is available (current v{}).",
                update.latest_version, update.current_version
            );
            install_script_ref(update.latest_tag)
        }
        Ok(None) => {
            println!("rho is up to date (v{current_version}).");
            return Ok(());
        }
        Err(err) => {
            eprintln!("warning: could not check latest release: {err}");
            println!("continuing with {} update command.", method.label());
            "main".to_string()
        }
    };

    println!("update command: {}", method.update_command(&git_ref));
    if method == InstallMethod::Pacman {
        println!("pacman may prompt for your sudo password.");
    }

    run_update_command(method, &git_ref).await
}

async fn run_update_command(method: InstallMethod, git_ref: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        println!(
            "automatic updates are disabled on Windows to avoid launching background shells that can trigger security software."
        );
        println!("copy and run this command yourself to update:");
        println!("{}", method.update_command(git_ref));
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let status = update_command(method, git_ref)
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
fn update_command(method: InstallMethod, git_ref: &str) -> Command {
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
            command.args(["-c", &method.update_command(git_ref)]);
            command
        }
        InstallMethod::Script => script_update_command(git_ref),
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
fn script_update_command_display(git_ref: &str) -> String {
    let ps1 = script_install_ps1_command(git_ref);
    let Some(install_dir) = current_exe_parent() else {
        return format!(
            "powershell -NoProfile -ExecutionPolicy Bypass -Command {}",
            powershell_quote(&ps1)
        );
    };
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -Command {command}",
        command = powershell_quote(&format!(
            "$env:RHO_INSTALL_DIR={}; {ps1}",
            powershell_quote_path(&install_dir)
        ))
    )
}

#[cfg(not(windows))]
fn script_update_command_display(git_ref: &str) -> String {
    let command = format!("sh -c {}", shell_quote(&script_install_sh_command(git_ref)));
    let Some(install_dir) = current_exe_parent() else {
        return command;
    };
    format!(
        "RHO_INSTALL_DIR={} {command}",
        shell_quote_path(&install_dir)
    )
}

#[cfg(not(windows))]
fn script_update_command(git_ref: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", &script_install_sh_command(git_ref)]);
    if let Some(install_dir) = current_exe_parent() {
        command.env("RHO_INSTALL_DIR", install_dir);
    }
    command
}

pub fn detect_install_method() -> InstallMethod {
    if let Ok(method) = std::env::var("RHO_INSTALL_METHOD") {
        match installation::install_hint(&method) {
            InstallHint::Cargo => return InstallMethod::Cargo,
            InstallHint::Pacman => return InstallMethod::Pacman,
            InstallHint::Scoop(ScoopInstallScope::User) => return InstallMethod::Scoop,
            InstallHint::Scoop(ScoopInstallScope::Global) => return InstallMethod::ScoopGlobal,
            InstallHint::Script => return InstallMethod::Script,
            InstallHint::Unknown => {}
        }
    }

    let current_exe = std::env::current_exe().ok();
    match current_exe
        .as_deref()
        .and_then(|path| installation::detect(path).managed)
    {
        Some(ManagedInstallation::Cargo { .. }) => InstallMethod::Cargo,
        Some(ManagedInstallation::Pacman) => InstallMethod::Pacman,
        Some(ManagedInstallation::Scoop(ScoopInstallScope::User)) => InstallMethod::Scoop,
        Some(ManagedInstallation::Scoop(ScoopInstallScope::Global)) => InstallMethod::ScoopGlobal,
        None => InstallMethod::Script,
    }
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

pub(crate) async fn latest_release_tag() -> anyhow::Result<String> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("rho-coding-agent"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    let client = crate::reqwest_client_builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut page = 1_u64;
    loop {
        let releases = client
            .get(RELEASES_URL)
            .query(&[("per_page", RELEASES_PER_PAGE), ("page", page)])
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<Release>>()
            .await?;
        if let Some(tag) = latest_app_release_tag(&releases) {
            return Ok(tag.to_string());
        }
        if releases.len() < RELEASES_PER_PAGE as usize {
            anyhow::bail!("GitHub releases did not include a published {CRATE_NAME} release");
        }
        page += 1;
    }
}

pub(crate) fn release_tag_to_version(tag: &str) -> Option<String> {
    let version = tag.strip_prefix(RELEASE_TAG_PREFIX)?.trim();
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
    use super::{
        install_script_ref, latest_app_release_tag, pacman_update_command_display,
        release_tag_to_version, scoop_update_command_display, version_is_newer, InstallMethod,
        Release, ScoopInstallScope,
    };

    #[test]
    fn install_script_ref_accepts_release_tags_and_rejects_shell_metacharacters() {
        assert_eq!(
            install_script_ref("rho-coding-agent-v1.13.0".into()),
            "rho-coding-agent-v1.13.0"
        );
        // A tag that would inject into `sh -c` falls back to the safe `main` ref.
        assert_eq!(install_script_ref("$(rm -rf ~)v1.0.0".into()), "main");
        assert_eq!(install_script_ref("v1.0.0; echo pwned".into()), "main");
        assert_eq!(install_script_ref(String::new()), "main");
    }

    // Covers: another workspace package's major release must not become a rho update.
    // Owner: update release selection
    #[test]
    fn selects_latest_published_app_release() {
        let releases = [
            Release {
                tag_name: "rho-providers-v3.0.0".into(),
                draft: false,
                prerelease: false,
            },
            Release {
                tag_name: "rho-coding-agent-v2.3.0".into(),
                draft: true,
                prerelease: false,
            },
            Release {
                tag_name: "rho-coding-agent-vbroken".into(),
                draft: false,
                prerelease: false,
            },
            Release {
                tag_name: "rho-coding-agent-v2.2.0".into(),
                draft: false,
                prerelease: false,
            },
        ];

        assert_eq!(
            latest_app_release_tag(&releases),
            Some("rho-coding-agent-v2.2.0")
        );
        assert_eq!(
            release_tag_to_version("rho-coding-agent-v2.2.0").as_deref(),
            Some("2.2.0")
        );
        assert_eq!(release_tag_to_version("rho-providers-v3.0.0"), None);
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
        let command = InstallMethod::Script.update_command("rho-coding-agent-v1.2.3");

        // The install script is fetched from the targeted release tag, never
        // from mutable `main`.
        assert!(command.contains("rho-coding-agent-v1.2.3/scripts/install"));
        assert!(!command.contains("/main/scripts/install"));

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
        let command = InstallMethod::Script.update_command("rho-coding-agent-v1.2.3");

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
            InstallMethod::Scoop.update_command("main"),
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
            InstallMethod::ScoopGlobal.update_command("main"),
            "scoop update; scoop update -g rho"
        );
        assert_eq!(InstallMethod::ScoopGlobal.label(), "Scoop (global)");
    }
}
