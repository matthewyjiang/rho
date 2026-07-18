use std::process::ExitCode;

use clap::Parser;
use rho_coding_agent::{run, AutomationInterrupted, Cli};

#[tokio::main]
async fn main() -> ExitCode {
    rho_providers::set_rho_version(env!("CARGO_PKG_VERSION"))
        .expect("provider version must be configured before provider initialization");

    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let exit_code = error
                .downcast_ref::<AutomationInterrupted>()
                .map_or(1, AutomationInterrupted::exit_code);
            eprintln!("Error: {error:?}");
            ExitCode::from(exit_code)
        }
    }
}
