//! Rho-managed install of `agy_acp_server`, offered by `rho login antigravity`.
//!
//! Google ships the server only as per-platform zips (ACP registry entry
//! `antigravity-acp`) and publishes no checksums, so Rho pins the release its
//! protocol facts were verified against: URL, SHA-256, and size per platform,
//! recorded 2026-10-06 from the registry and dl.google.com. Bumping
//! [`PINNED_VERSION`] moves the release directory, so the server reads as not
//! installed and the next `/login antigravity` installs the new release and
//! prunes older ones.
//!
//! Layout ([`ManagedRoot`]): `$RHO_HOME/runtimes/antigravity-acp/<version>/`
//! holds the server and its `localharness_external`, which is where
//! [`super::executable::harness_location`] looks. A server on `PATH` always
//! wins over the managed copy.

use std::{
    io::{BufRead, ErrorKind, IsTerminal as _, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context as _};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use super::{
    executable::{ANTIGRAVITY_PROGRAM, HARNESS_FILE},
    ANTIGRAVITY_LABEL_NAME,
};
use crate::config_writer::edit_lock::acquire_lock_file;

/// The `antigravity-acp` registry release Rho installs.
pub(crate) const PINNED_VERSION: &str = "1.3.0";

/// No bytes for this long means a dead transfer, not a slow one; a healthy
/// download delivers chunks continuously. Not a whole-transfer budget: the
/// Linux archive is 334 MB.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

const LOCK_FILE: &str = ".install.lock";
/// Per-install scratch (download and unpack); leftovers are interrupted
/// installs.
const STAGING_PREFIX: &str = ".staging-";

/// One platform's pinned archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PinnedArchive {
    url: &'static str,
    sha256: &'static str,
    /// Exact archive size; a longer or shorter body fails the download.
    archive_bytes: u64,
    /// Unpacked size, shown before the user agrees.
    unpacked_bytes: u64,
}

/// The pinned archive for this machine, if Google ships one.
pub(crate) fn pinned_archive() -> Option<PinnedArchive> {
    archive_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn archive_for(os: &str, arch: &str) -> Option<PinnedArchive> {
    Some(match (os, arch) {
        ("linux", "x86_64") => PinnedArchive {
            url: "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.3.0-linux-x86_64.zip",
            sha256: "9fb60956af0a9d76220a4db91ca9ac88e2a2372ad68f985ab5fceace6b825b96",
            archive_bytes: 333_727_150,
            unpacked_bytes: 1_056_922_005,
        },
        ("linux", "aarch64") => PinnedArchive {
            url: "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.3.0-linux-arm64.zip",
            sha256: "500b0bc0fb858e88f4df404d4cedf80bf9298c178291e39e383d6c50b111cbdf",
            archive_bytes: 321_690_363,
            unpacked_bytes: 1_054_073_960,
        },
        ("macos", "aarch64") => PinnedArchive {
            url: "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.3.0-darwin-arm64.zip",
            sha256: "7cd97045f7b4fe81175a107cdf16f9c51484e3c78a5162cae415338bb6aa5b88",
            archive_bytes: 111_456_962,
            unpacked_bytes: 397_146_848,
        },
        ("macos", "x86_64") => PinnedArchive {
            url: "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.3.0-darwin-x86_64.zip",
            sha256: "bb23956b89984bf5d354af2c3725e6c57f0cc1b7228e77a0e91c9c2bc1d47646",
            archive_bytes: 117_245_544,
            unpacked_bytes: 407_016_080,
        },
        ("windows", "x86_64") => PinnedArchive {
            url: "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.3.0-windows-x86_64.zip",
            sha256: "65215e0688681fa3116e048a9eab27ef53af1bbd6f3da3f1c52bd4911d8b17f9",
            archive_bytes: 124_509_787,
            unpacked_bytes: 226_986_288,
        },
        ("windows", "aarch64") => PinnedArchive {
            url: "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.3.0-windows-arm64.zip",
            sha256: "4a0f469720e9beb9438a979f543fdbfad5022ebe0992c052c590bd78b3144ca3",
            archive_bytes: 124_654_803,
            unpacked_bytes: 221_533_688,
        },
        _ => return None,
    })
}

/// `$RHO_HOME/runtimes/antigravity-acp`, which Rho owns entirely. Always
/// absolute: a relative `RHO_HOME` is anchored once here, so a workflow
/// workspace or a child's working directory cannot reinterpret the path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManagedRoot(PathBuf);

impl ManagedRoot {
    pub(crate) fn new(rho_home: &Path) -> std::io::Result<Self> {
        Ok(Self(
            crate::paths::user_runtimes_dir(&std::path::absolute(rho_home)?)
                .join("antigravity-acp"),
        ))
    }

    pub(crate) fn from_env() -> anyhow::Result<Self> {
        Ok(Self::new(&crate::paths::rho_dir()?)?)
    }

    fn release_dir(&self) -> PathBuf {
        self.0.join(PINNED_VERSION)
    }

    /// Where the pinned server lives once installed.
    pub(crate) fn server(&self) -> PathBuf {
        self.release_dir().join(ANTIGRAVITY_PROGRAM)
    }

    pub(crate) fn installed_server(&self) -> Option<PathBuf> {
        let server = self.server();
        server.is_file().then_some(server)
    }
}

/// For a missing server: ask on the terminal, then install the pinned
/// release. Returns the installed server's path.
pub(crate) async fn offer_install() -> anyhow::Result<PathBuf> {
    let missing = format!("{ANTIGRAVITY_LABEL_NAME}: {ANTIGRAVITY_PROGRAM} is not installed");
    let Some(archive) = pinned_archive() else {
        bail!(
            "{missing} and Rho has no download for {}-{}; install it on PATH manually (see docs/subagents/antigravity.md)",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    };
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("{missing}; run `rho login antigravity` in a terminal to install it");
    }
    let root = ManagedRoot::from_env()?;
    if !confirm(
        &archive,
        &root.release_dir(),
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )? {
        bail!("{missing}; installation declined");
    }
    install(&archive, &root).await
}

/// The consent prompt; only `y` or `yes` agrees.
fn confirm(
    archive: &PinnedArchive,
    target: &Path,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<bool> {
    write!(
        output,
        "Antigravity's ACP server is not installed.\n\
Rho can download Google's {ANTIGRAVITY_PROGRAM} {PINNED_VERSION} ({} MB, {} MB unpacked) from\n\
{}\ninto {}.\nInstall it? [y/N] ",
        archive.archive_bytes / 1_000_000,
        archive.unpacked_bytes / 1_000_000,
        archive.url,
        crate::paths::display(target),
    )?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    let answer = answer.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

/// Download and verify `archive` into a staging directory, publish it as the
/// pinned release with one rename, then prune. Nothing outside staging
/// changes unless every check passed. Installs hold the root's lock file.
async fn install(archive: &PinnedArchive, root: &ManagedRoot) -> anyhow::Result<PathBuf> {
    let lock_path = root.0.join(LOCK_FILE);
    let _lock = match acquire_lock_file(&lock_path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == ErrorKind::WouldBlock => bail!(
            "another `rho login antigravity` is installing {ANTIGRAVITY_PROGRAM}; wait for it to finish"
        ),
        Err(error) => Err(error).with_context(|| {
            format!("could not lock {}", crate::paths::display(&lock_path))
        })?,
    };
    // Another login may have finished while this one waited at the prompt.
    if let Some(server) = root.installed_server() {
        return Ok(server);
    }
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(&root.0)?;
    eprintln!("downloading {}", archive.url);
    download_verified(archive, &staging.path().join("archive.zip")).await?;
    let publish_root = root.clone();
    tokio::task::spawn_blocking(move || publish(&publish_root, staging)).await??;
    eprintln!(
        "installed {ANTIGRAVITY_PROGRAM} {PINNED_VERSION} in {}",
        crate::paths::display(&root.release_dir())
    );
    Ok(root.server())
}

/// Stream the archive to `dest`, failing on a size or SHA-256 mismatch.
async fn download_verified(archive: &PinnedArchive, dest: &Path) -> anyhow::Result<()> {
    let client = crate::reqwest_client_builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STALL_TIMEOUT)
        .build()?;
    let mut response = client
        .get(archive.url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("could not download {}", archive.url))?;
    let mut file = tokio::fs::File::create(dest).await?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut progress = Progress::new(archive.archive_bytes);
    while let Some(chunk) = response.chunk().await.with_context(|| {
        format!(
            "download stopped after {} of {} MB (no data for {} s ends it)",
            received / 1_000_000,
            archive.archive_bytes / 1_000_000,
            STALL_TIMEOUT.as_secs()
        )
    })? {
        received += chunk.len() as u64;
        if received > archive.archive_bytes {
            bail!(
                "download exceeded the pinned archive size of {} bytes",
                archive.archive_bytes
            );
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        progress.update(received);
    }
    file.flush().await?;
    progress.finish();
    if received != archive.archive_bytes {
        bail!(
            "download ended at {received} bytes; the pinned archive is {} bytes",
            archive.archive_bytes
        );
    }
    let digest = hex::encode(hasher.finalize());
    if digest != archive.sha256 {
        bail!(
            "downloaded archive has SHA-256 {digest}, expected {}; nothing was installed",
            archive.sha256
        );
    }
    Ok(())
}

/// Whole-percent download progress on one rewritten stderr line. Installs
/// only run on a terminal, so there is no non-interactive mode.
struct Progress {
    total: u64,
    shown: Option<u64>,
}

impl Progress {
    fn new(total: u64) -> Self {
        Self { total, shown: None }
    }

    fn update(&mut self, received: u64) {
        let percent = received.saturating_mul(100) / self.total.max(1);
        if self.shown == Some(percent) {
            return;
        }
        self.shown = Some(percent);
        eprint!(
            "\r{percent:>3}% ({} / {} MB)",
            received / 1_000_000,
            self.total / 1_000_000
        );
    }

    fn finish(&self) {
        if self.shown.is_some() {
            eprintln!();
        }
    }
}

/// Unpack the staged archive, rename it into place as the pinned release,
/// then prune. Callers hold the root's lock. The release is durable before
/// the rename publishes it, because [`ManagedRoot::installed_server`] trusts
/// any server file it finds: a crash must leave either no release or a whole
/// one, never a truncated binary later logins would skip reinstalling.
fn publish(root: &ManagedRoot, staging: tempfile::TempDir) -> anyhow::Result<()> {
    let release = staging.path().join("release");
    std::fs::create_dir(&release)?;
    unpack(&staging.path().join("archive.zip"), &release)?;
    sync_dir(&release)?;
    let target = root.release_dir();
    // Under the lock, a release directory without the server is an earlier
    // interrupted publish.
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .with_context(|| format!("could not replace {}", crate::paths::display(&target)))?;
    }
    std::fs::rename(&release, &target).with_context(|| {
        format!(
            "could not move the server into {}",
            crate::paths::display(&target)
        )
    })?;
    sync_dir(&root.0)?;
    for entry in std::fs::read_dir(&root.0)?.flatten() {
        let path = entry.path();
        if path == staging.path() || !prunable(&entry.file_name().to_string_lossy()) {
            continue;
        }
        // Best effort: a running server can hold an old release open on Windows.
        let _ = std::fs::remove_dir_all(&path);
    }
    Ok(())
}

/// Leftover staging and releases older than the pin. A newer release belongs
/// to a newer Rho sharing this home, and unknown names are not Rho's to judge.
fn prunable(name: &str) -> bool {
    fn parts(version: &str) -> Option<Vec<u64>> {
        version.split('.').map(|part| part.parse().ok()).collect()
    }
    name.starts_with(STAGING_PREFIX)
        || parts(name)
            .zip(parts(PINNED_VERSION))
            .is_some_and(|(release, pinned)| release < pinned)
}

/// Flush a directory's entries. Windows cannot open a directory as a file
/// without extra flags, and NTFS journals renames, so this is Unix-only.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Extract exactly the server and its harness, flat, into `dir`. Any other
/// layout fails, so a re-pinned archive that moved its files cannot install
/// a server the runtime would not find.
fn unpack(archive: &Path, dir: &Path) -> anyhow::Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)
        .context("downloaded archive is not a zip file")?;
    let mut names: Vec<&str> = zip.file_names().collect();
    names.sort_unstable();
    let mut expected = [ANTIGRAVITY_PROGRAM, HARNESS_FILE];
    expected.sort_unstable();
    if names != expected {
        bail!("unexpected archive contents {names:?}; expected {expected:?}");
    }
    for name in expected {
        let mut entry = zip.by_name(name)?;
        let path = dir.join(name);
        let mut out = std::fs::File::create_new(&path)?;
        std::io::copy(&mut entry, &mut out).with_context(|| format!("could not unpack {name}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            out.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        }
        // After the mode change, so the executable bit is durable too.
        out.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
