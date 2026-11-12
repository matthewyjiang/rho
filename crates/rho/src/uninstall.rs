//! Conservative local uninstall. Unknown installations require manual removal.
//! The running daemon is stopped, and a saved `rho mcp serve --http` service
//! is stopped and its files removed, before anything they run from goes.

use crate::app::daemon_install::{self, RunningDaemon};
use crate::daemon::{find_services, ServiceFiles, ServiceManager};
use crate::installation::{ManagedInstallation, ScoopInstallScope};
use anyhow::{bail, Context, Result};
use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
};

#[path = "uninstall_plan.rs"]
mod planning;
use planning::{plan, validate_path, Installation, Removal, RemovalPlan, TargetKind};

struct Paths {
    home: PathBuf,
    executable: PathBuf,
    install_method: Option<String>,
    custom_data: Option<PathBuf>,
    /// Names the mise tool owning a mise install; injected so tests never run mise.
    mise_tool: fn(&Path) -> Result<String>,
}

/// What uninstall found running or saved besides the installation: the
/// daemon to stop and the service files to remove.
#[derive(Default)]
struct Found {
    daemon: Option<RunningDaemon>,
    services: Vec<ServiceFiles>,
}

/// Preview local removal and ask for confirmation, then stop the saved
/// service and the daemon before removing anything they run from. Never
/// runs a package manager.
pub(crate) async fn run(purge: bool, dry_run: bool) -> Result<()> {
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
        mise_tool: crate::installation::mise_tool,
    };
    let found = Found {
        daemon: daemon_install::running_daemon(),
        services: find_services(
            &paths.home,
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        ),
    };
    let Some(plan) = review(
        &paths,
        &found,
        purge,
        dry_run,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
    )?
    else {
        return Ok(());
    };
    // The service first: its server would start the daemon again.
    remove_services(&found.services, &mut io::stdout().lock())?;
    if found.daemon.is_some() {
        daemon_install::stop_for_uninstall().await?;
    }
    execute(plan, &mut io::stdout().lock(), |path, kind| match kind {
        TargetKind::DataDirectory => fs::remove_dir_all(path),
        TargetKind::Executable => fs::remove_file(path),
    })
}

/// Stops each saved service through its manager, then deletes its files. A
/// manager that could not be asked does not keep the files: the note says
/// what may still run.
fn remove_services(services: &[ServiceFiles], output: &mut impl Write) -> Result<()> {
    for service in services {
        if let Err(error) = service.stop() {
            writeln!(
                output,
                "note: could not stop the service {:?} ({error}); stop it yourself if it still runs",
                service.definition
            )?;
        }
        service.delete().with_context(|| {
            format!(
                "could not remove the service files of {:?}",
                service.definition
            )
        })?;
        for path in service.paths() {
            writeln!(output, "removed {path:?}")?;
        }
    }
    Ok(())
}

fn managed_instruction(managed: &ManagedInstallation, paths: &Paths) -> String {
    match managed {
        ManagedInstallation::Cargo { root: Some(root) } => format!(
            "run cargo uninstall rho-coding-agent --root <install-root>, using install-root {root:?}"
        ),
        ManagedInstallation::Cargo { root: None } => "run cargo uninstall rho-coding-agent".into(),
        // `unuse` drops the global request, so a lazy shim cannot reinstall it,
        // and prunes versions no other config needs.
        ManagedInstallation::Mise { install_path } => match (paths.mise_tool)(install_path) {
            Ok(tool) => format!("run mise unuse -g {tool}"),
            Err(err) => format!("uninstall it with mise, which installed it at {install_path:?} ({err:#})"),
        },
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

/// Prints what uninstalling does and asks for confirmation. Returns the
/// plan to carry out, or `None` when there is nothing to do, on a dry run,
/// and when the user declined.
fn review(
    paths: &Paths,
    found: &Found,
    purge: bool,
    dry_run: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Option<RemovalPlan>> {
    let plan = plan(paths, purge)?;
    let data = paths.home.join(".rho");
    writeln!(output, "running executable: {:?}", paths.executable)?;
    match &plan.installation {
        Installation::Script => {}
        Installation::Managed(managed) => writeln!(output, "executable will not be removed automatically; {}", managed_instruction(managed, paths))?,
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
    match found.daemon {
        Some(RunningDaemon { pid: Some(pid) }) => writeln!(
            output,
            "the rho daemon is running (pid {pid}); it will be stopped, which interrupts the turns under way"
        )?,
        Some(RunningDaemon { pid: None }) => writeln!(
            output,
            "the rho daemon is running; it will be stopped, which interrupts the turns under way"
        )?,
        None => {}
    }
    writeln!(output, "paths to remove:")?;
    for service in &found.services {
        let manager = match service.manager {
            ServiceManager::Systemd => "systemd user unit",
            ServiceManager::Launchd => "launchd agent",
        };
        for link in &service.enabled_by {
            writeln!(output, "  {link:?} (link that enables the {manager})")?;
        }
        writeln!(
            output,
            "  {:?} ({manager}; the service is stopped first)",
            service.definition
        )?;
    }
    for target in &plan.removals {
        let kind = match target.kind {
            TargetKind::DataDirectory => {
                "directory and its contents; includes file-backed credentials"
            }
            TargetKind::Executable => "executable",
        };
        writeln!(output, "  {:?} ({kind})", target.path)?;
    }
    if plan.removals.is_empty() && found.services.is_empty() {
        writeln!(output, "  none")?;
        if found.daemon.is_none() {
            return Ok(None);
        }
    }
    if dry_run {
        writeln!(output, "dry run: nothing stopped or removed")?;
        return Ok(None);
    }
    if !confirm(purge, input, output)? {
        writeln!(output, "uninstall cancelled; nothing stopped or removed")?;
        return Ok(None);
    }
    Ok(Some(plan))
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
