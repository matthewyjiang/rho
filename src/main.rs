use clap::Parser;
use rho_coding_agent::{run, Cli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run(Cli::parse()).await
}
