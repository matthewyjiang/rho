//! Asking the service manager that runs a saved `rho mcp serve --http`
//! service to stop it: systemd's user manager, or launchd on macOS; and
//! running a request to a service manager, which `user_scope` shares.

use std::{
    io,
    process::{Command, Stdio},
};

use crate::daemon::service::{ServiceManager, LAUNCHD_LABEL, SYSTEMD_UNIT};

/// Stops the saved service and keeps its manager from starting it again.
/// The error carries what the manager printed, as when no user manager
/// answers in this login or the service was never loaded.
pub(in crate::daemon) fn stop_service(manager: ServiceManager) -> io::Result<()> {
    let (program, args) = match manager {
        ServiceManager::Systemd => (
            "systemctl",
            vec![
                "--user".to_owned(),
                "disable".to_owned(),
                "--now".to_owned(),
                SYSTEMD_UNIT.to_owned(),
            ],
        ),
        ServiceManager::Launchd => {
            // SAFETY: geteuid has no arguments or memory-safety preconditions.
            let user = unsafe { libc::geteuid() };
            (
                "launchctl",
                vec!["bootout".to_owned(), format!("gui/{user}/{LAUNCHD_LABEL}")],
            )
        }
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_manager_command(program, &args)
}

/// Runs `program` with `args`, a request to a service manager. The error
/// names the command and carries what it printed.
pub(super) fn run_manager_command(program: &str, args: &[&str]) -> io::Result<()> {
    let ran = format!("{program} {}", args.join(" "));
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| io::Error::new(error.kind(), format!("could not run `{ran}`: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "`{ran}` failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}
