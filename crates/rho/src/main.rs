use std::process::ExitCode;

use clap::Parser;
use rho_coding_agent::{run, AutomationExit, AutomationInterrupted, Cli};

/// Runs the command-line agent and maps its outcome to a process exit code.
///
/// # Examples
///
/// ```
/// use std::process::ExitCode;
///
/// let success = ExitCode::SUCCESS;
/// assert_eq!(success, ExitCode::from(0));
/// ```
#[tokio::main]
async fn main() -> ExitCode {
    rho_providers::set_rho_version(env!("CARGO_PKG_VERSION"))
        .expect("provider version must be configured before provider initialization");

    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let exit_code = error
                .downcast_ref::<AutomationExit>()
                .map(AutomationExit::exit_code)
                .or_else(|| {
                    error
                        .downcast_ref::<AutomationInterrupted>()
                        .map(AutomationInterrupted::exit_code)
                })
                .unwrap_or(1);
            eprintln!("Error: {error:?}");
            ExitCode::from(exit_code)
        }
    }
}
