//! `rho update --check`: reports the latest release and how `rho update`
//! would install it, without installing anything or touching the daemon.
//! Desktop clients run it, locally or over ssh, to offer an update.

use anyhow::Context;
use serde::Serialize;

use super::{detect_install_method, install_script_ref, latest_release, InstallMethod, UpdateInfo};

/// How `rho update --check` prints its report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckFormat {
    /// A short summary for a person.
    Text,
    /// One JSON object, `UpdateCheck`, for a program.
    Json,
}

/// The report `rho update --check --json` prints. Clients such as rho-ui
/// parse these field names, so keep them stable.
#[derive(Debug, Serialize)]
struct UpdateCheck {
    current: String,
    /// The latest release's version, without the tag prefix.
    latest: String,
    update_available: bool,
    method: InstallMethod,
    /// The command `rho update` runs (on Windows, prints) for `method`.
    command: String,
    /// `rho update` cannot finish unattended, so a client should run it in
    /// a terminal rather than in the background.
    needs_terminal: bool,
}

impl UpdateCheck {
    /// Fails when `method`'s command cannot be resolved, as `rho update` would.
    fn new(update: UpdateInfo, method: InstallMethod) -> anyhow::Result<Self> {
        let update_available = update.is_available();
        let UpdateInfo {
            latest_tag,
            latest_version,
            current_version,
        } = update;
        Ok(Self {
            current: current_version,
            latest: latest_version,
            update_available,
            method,
            command: method.update_command(&install_script_ref(latest_tag))?,
            needs_terminal: method.needs_terminal(),
        })
    }
}

/// Runs `rho update --check`. Unlike `rho update`, which goes on without the
/// release, a failed lookup is an error: the check has nothing else to report.
pub(crate) async fn run_check(current_version: &str, format: CheckFormat) -> anyhow::Result<()> {
    let update = latest_release(current_version)
        .await
        .context("could not check the latest rho release")?;
    let check = UpdateCheck::new(update, detect_install_method())?;
    match format {
        CheckFormat::Json => println!("{}", serde_json::to_string(&check)?),
        CheckFormat::Text => print_summary(&check),
    }
    Ok(())
}

fn print_summary(check: &UpdateCheck) {
    println!("install method: {}", check.method.label());
    if !check.update_available {
        println!("rho is up to date (v{}).", check.current);
        return;
    }
    println!(
        "rho v{} is available (current v{}).",
        check.latest, check.current
    );
    println!("update command: {}", check.command);
    if check.needs_terminal {
        println!("run `rho update` in a terminal to install it.");
    } else {
        println!("run `rho update` to install it.");
    }
}

#[cfg(test)]
#[path = "update_check_tests.rs"]
mod tests;
