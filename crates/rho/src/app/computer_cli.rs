//! Read-only CLI diagnostics; installation and access consent belong to the TUI.

use crate::{
    cli::ComputerCommand,
    tools::computer_use::{self, ComputerUseSession},
};

pub(super) fn run(command: &ComputerCommand) -> anyhow::Result<()> {
    match command {
        ComputerCommand::Setup => println!("{}", ComputerUseSession::setup_guidance()),
        ComputerCommand::Status { json } => {
            let driver = computer_use::detect_driver();
            let desktop_warning = computer_use::desktop_warning();
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "feature": "computer_use",
                        "backend": "cua_driver",
                        "driver": driver,
                        "detected": driver.is_some(),
                        "probed": false,
                        "desktop_warning": desktop_warning,
                        "authorization": "interactive_session_only"
                    }))?
                );
            } else {
                println!("Computer use, powered by Cua Driver");
                match driver {
                    Some(path) => println!("Driver detected: {}", path.display()),
                    None => println!("Driver not detected; run rho computer setup"),
                }
                println!("No connection or desktop permission probe performed. Use /computer on inside Rho to grant access for that session; /computer off revokes it.");
                if let Some(warning) = desktop_warning {
                    println!("Warning: {warning}");
                }
            }
        }
    }
    Ok(())
}
