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
                        "authorization": "machine_local_interactive_consent"
                    }))?
                );
            } else {
                println!("Computer use, powered by Cua Driver");
                match driver {
                    Some(path) => println!("Driver detected: {}", path.display()),
                    None => println!("Driver not detected; run rho computer setup"),
                }
                println!("No connection or desktop permission probe performed. Inside Rho, /computer on saves access for this session and defaults new sessions to on; /computer off revokes access and saves both off. Choices stay on this machine. Resumed sessions keep their own saved choice.");
                if let Some(warning) = desktop_warning {
                    println!("Warning: {warning}");
                }
            }
        }
    }
    Ok(())
}
