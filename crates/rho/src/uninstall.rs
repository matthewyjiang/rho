//! Conservative local uninstall. Unknown installations require manual removal.

use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};

struct Paths {
    home: PathBuf,
    executable: PathBuf,
    install_method: Option<String>,
    custom_data: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum Installation {
    Script,
    Managed(String),
    Unknown,
}

struct Removal {
    path: PathBuf,
    kind: TargetKind,
    identity: same_file::Handle,
}

#[derive(Clone, Copy)]
enum TargetKind {
    Executable,
    DataDirectory,
}

/// Preview local removal and ask for confirmation. Never runs a package manager.
pub(crate) fn run(purge: bool, dry_run: bool) -> Result<()> {
    let paths = Paths {
        home: fs::canonicalize(
            crate::paths::home_dir().context("could not determine home directory")?,
        )
        .context("could not resolve home directory")?,
        executable: fs::canonicalize(
            std::env::current_exe().context("could not locate running executable")?,
        )
        .context("could not resolve running executable")?,
        install_method: std::env::var("RHO_INSTALL_METHOD").ok(),
        custom_data: std::env::var_os("RHO_HOME").map(PathBuf::from),
    };
    run_with_io(
        &paths,
        purge,
        dry_run,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
    )
}

fn installation(paths: &Paths) -> Installation {
    // Overrides may disable deletion, but cannot authorize it. The installer has
    // no provenance receipt, so only its default per-user location is recognized.
    let hint = paths
        .install_method
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let normalized = paths
        .executable
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let root = paths.executable.parent().and_then(Path::parent);
    if hint == "cargo"
        || normalized.contains("/.cargo/bin/")
        || root.is_some_and(|root| {
            root.join(".crates.toml").exists() || root.join(".crates2.json").exists()
        })
    {
        let root = root.unwrap_or(Path::new("."));
        return Installation::Managed(format!(
            "run cargo uninstall rho-coding-agent --root <install-root>, using install-root {root:?}"
        ));
    }
    if matches!(hint.as_str(), "scoop" | "scoop-global" | "scoop_global")
        || normalized.contains("/scoop/")
    {
        let global = matches!(hint.as_str(), "scoop-global" | "scoop_global")
            || normalized.contains("/programdata/scoop/");
        return Installation::Managed(
            if global {
                "run scoop uninstall rho --global"
            } else {
                "run scoop uninstall rho"
            }
            .into(),
        );
    }
    if hint == "pacman" {
        return Installation::Managed("run sudo pacman -R rho-coding-agent".into());
    }
    if normalized.starts_with("/usr/")
        || normalized.starts_with("/opt/")
        || normalized.contains("/cellar/")
    {
        return Installation::Managed("remove it manually after rho exits if installed by script, or use the package manager that installed it".into());
    }
    if !matches!(hint.as_str(), "" | "script" | "install-script") {
        return Installation::Unknown;
    }
    if !cfg!(windows) && paths.executable == paths.home.join(".local/bin/rho") {
        Installation::Script
    } else {
        Installation::Unknown
    }
}

// Refuse roots, relative paths, traversal, and linked ancestors rather than
// turning an ambiguous path into authority to delete a different directory.
fn validate_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.parent().is_none() {
        bail!("refusing unsafe uninstall path {path:?}: expected an absolute non-root path");
    }
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            bail!("refusing unsafe uninstall path {path:?}: traversal component");
        }
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if is_link(&metadata) {
                    bail!("refusing unsafe uninstall path {path:?}: linked ancestor {ancestor:?}");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("could not inspect {ancestor:?}"))
            }
        }
    }
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse points also redirect filesystem access.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn removal(path: PathBuf, kind: TargetKind) -> Result<Option<Removal>> {
    validate_path(&path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("could not inspect {path:?}")),
    };
    let expected_type = match kind {
        TargetKind::Executable => metadata.is_file(),
        TargetKind::DataDirectory => metadata.is_dir(),
    };
    if !expected_type {
        bail!("refusing unexpected file type at {path:?}");
    }
    let identity = same_file::Handle::from_path(&path)?;
    Ok(Some(Removal {
        path,
        kind,
        identity,
    }))
}

fn confirm(purge: bool, input: &mut impl BufRead, output: &mut impl Write) -> Result<bool> {
    loop {
        let prompt = if purge {
            "Uninstall Rho and permanently delete local config, sessions, and file-backed credentials? [y/N] "
        } else {
            "Uninstall Rho? [y/N] "
        };
        write!(output, "{prompt}")?;
        output.flush()?;
        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            return Ok(false);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => writeln!(output, "enter y or n")?,
        }
    }
}

fn run_with_io(
    paths: &Paths,
    purge: bool,
    dry_run: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<()> {
    validate_path(&paths.home)?;
    if !paths.home.is_dir() {
        bail!(
            "refusing unsafe home directory {:?}: not a directory",
            paths.home
        );
    }
    let data = paths.home.join(".rho");
    let mut removals = Vec::new();
    let install = installation(paths);
    writeln!(output, "running executable: {:?}", paths.executable)?;
    match install {
        Installation::Script => {
            if let Some(target) = removal(paths.executable.clone(), TargetKind::Executable)? {
                removals.push(target);
            }
        }
        Installation::Managed(instruction) => writeln!(output, "executable will not be removed automatically; {instruction}")?,
        Installation::Unknown => writeln!(output, "installation is not recognized; after rho exits, remove the executable shown above manually if it is not package-managed")?,
    }
    if cfg!(windows) {
        writeln!(
            output,
            "Windows: no running executable is deleted and no background shell is launched"
        )?;
    }
    if purge {
        if let Some(target) = removal(data.clone(), TargetKind::DataDirectory)? {
            // Do not let a recursive data purge indirectly remove an executable
            // that we deliberately left to its package manager or Windows user.
            if paths.executable.starts_with(&data) {
                bail!("refusing to purge {data:?}: running executable is inside it; move or uninstall that executable first");
            }
            removals.push(target);
        }
    } else {
        writeln!(
            output,
            "preserving local data: {data:?}; use --purge to remove it"
        )?;
    }
    writeln!(output, "left untouched: OS keyring credentials under service 'rho', environment credentials, shell/PATH edits, shared ~/.agents, project files, and data outside {data:?}")?;
    if let Some(custom_data) = &paths.custom_data {
        writeln!(output, "RHO_HOME is set to {custom_data:?}; only {data:?} is eligible for purge, no additional custom data path is removed")?;
    }
    writeln!(output, "log out of providers before uninstalling or remove keyring entries with your OS credential manager; credentials are not revoked remotely")?;
    writeln!(output, "paths to remove:")?;
    for target in &removals {
        let kind = match target.kind {
            TargetKind::DataDirectory => {
                "directory and its contents; includes file-backed credentials"
            }
            TargetKind::Executable => "executable",
        };
        writeln!(output, "  {:?} ({kind})", target.path)?;
    }
    if removals.is_empty() {
        writeln!(output, "  none")?;
        return Ok(());
    }
    if dry_run {
        writeln!(output, "dry run: nothing removed")?;
        return Ok(());
    }
    if !confirm(purge, input, output)? {
        writeln!(output, "uninstall cancelled; nothing removed")?;
        return Ok(());
    }
    // Check every target again before the first deletion, including identity:
    // a directory replaced while the user reads the prompt is not authorized.
    for target in &removals {
        validate_path(&target.path)?;
        if same_file::Handle::from_path(&target.path)? != target.identity {
            bail!("refusing changed uninstall target {:?}", target.path);
        }
    }
    // Data first: an unsuccessful purge should not prevent retrying with rho.
    for target in removals.into_iter().rev() {
        let Removal {
            path,
            kind,
            identity,
        } = target;
        // Do not keep a directory handle open while Windows removes it.
        drop(identity);
        match kind {
            TargetKind::DataDirectory => fs::remove_dir_all(&path),
            TargetKind::Executable => fs::remove_file(&path),
        }
        .with_context(|| {
            format!("could not remove {path:?}; earlier removals are not rolled back")
        })?;
        writeln!(output, "removed {path:?}")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "uninstall_tests.rs"]
mod tests;
