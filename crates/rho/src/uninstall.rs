//! Conservative local uninstall. Unknown installations require manual removal.

use crate::installation::{ManagedInstallation, ScoopInstallScope};
use anyhow::{bail, Context, Result};
use std::{
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
};

#[path = "uninstall_plan.rs"]
mod planning;
use planning::{plan, validate_path, Installation, Removal, RemovalPlan, TargetKind};

struct Paths {
    home: PathBuf,
    executable: PathBuf,
    install_method: Option<String>,
    custom_data: Option<PathBuf>,
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

fn managed_instruction(managed: &ManagedInstallation) -> String {
    match managed {
        ManagedInstallation::Cargo { root: Some(root) } => format!(
            "run cargo uninstall rho-coding-agent --root <install-root>, using install-root {root:?}"
        ),
        ManagedInstallation::Cargo { root: None } => "run cargo uninstall rho-coding-agent".into(),
        ManagedInstallation::Pacman => "run sudo pacman -R rho-coding-agent".into(),
        ManagedInstallation::Scoop(ScoopInstallScope::User) => "run scoop uninstall rho".into(),
        ManagedInstallation::Scoop(ScoopInstallScope::Global) => "run scoop uninstall rho --global".into(),
    }
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
    let plan = plan(paths, purge)?;
    let data = paths.home.join(".rho");
    writeln!(output, "running executable: {:?}", paths.executable)?;
    match &plan.installation {
        Installation::Script => {}
        Installation::Managed(managed) => writeln!(output, "executable will not be removed automatically; {}", managed_instruction(managed))?,
        Installation::Unknown => writeln!(output, "installation is not recognized; after rho exits, remove the executable shown above manually if it is not package-managed")?,
    }
    if cfg!(windows) {
        writeln!(
            output,
            "Windows: no running executable is deleted and no background shell is launched"
        )?;
    }
    if !purge {
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
    for target in &plan.removals {
        let kind = match target.kind {
            TargetKind::DataDirectory => {
                "directory and its contents; includes file-backed credentials"
            }
            TargetKind::Executable => "executable",
        };
        writeln!(output, "  {:?} ({kind})", target.path)?;
    }
    if plan.removals.is_empty() {
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
    execute(plan, output, |path, kind| match kind {
        TargetKind::DataDirectory => fs::remove_dir_all(path),
        TargetKind::Executable => fs::remove_file(path),
    })
}

/// Revalidate every target before any deletion. The remover seam allows testing
/// I/O failures deterministically, including when tests run as root.
fn execute(
    plan: RemovalPlan,
    output: &mut impl Write,
    mut remove: impl FnMut(&std::path::Path, TargetKind) -> io::Result<()>,
) -> Result<()> {
    for target in &plan.removals {
        validate_path(&target.path)?;
        if same_file::Handle::from_path(&target.path)? != target.identity {
            bail!("refusing changed uninstall target {:?}", target.path);
        }
    }
    for target in plan.removals {
        let Removal {
            path,
            kind,
            identity,
        } = target;
        // Do not keep a directory handle open while Windows removes it.
        drop(identity);
        remove(&path, kind).with_context(|| {
            format!("could not remove {path:?}; earlier removals are not rolled back")
        })?;
        writeln!(output, "removed {path:?}")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "uninstall_tests.rs"]
mod tests;
