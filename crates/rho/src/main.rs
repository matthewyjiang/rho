use std::process::ExitCode;

use clap::Parser;
use rho_coding_agent::{run, AutomationExit, AutomationInterrupted, Cli};

#[tokio::main]
async fn main() -> ExitCode {
    rho_providers::set_rho_version(env!("CARGO_PKG_VERSION"))
        .expect("provider version must be configured before provider initialization");
    rho_providers::install_managed_credential_env_vars();

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
