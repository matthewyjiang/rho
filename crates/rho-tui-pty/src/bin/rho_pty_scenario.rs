//! Local debugging entrypoint for a single named PTY scenario.
//!
//! Usage:
//!   cargo run -p rho-tui-pty --bin rho-pty-scenario -- startup_stream_exit
//!   cargo run -p rho-tui-pty --bin rho-pty-scenario -- --list
//!   cargo run -p rho-tui-pty --bin rho-pty-scenario -- --timing startup_stream_exit

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use rho_tui_pty::{all_scenarios, run_named, ScenarioRunner};

#[derive(Debug, Parser)]
#[command(name = "rho-pty-scenario", about = "Run a Rho TUI PTY scenario")]
struct Args {
    /// Scenario id to run. Use --list to print available ids.
    scenario: Option<String>,

    /// List available scenarios and exit.
    #[arg(long)]
    list: bool,

    /// Run the CI smoke subset.
    #[arg(long)]
    smoke: bool,

    /// Record wait timing samples and print percentiles.
    #[arg(long)]
    timing: bool,

    /// Directory for failure artifacts.
    #[arg(long)]
    artifacts: Option<PathBuf>,

    /// Override the Rho binary path.
    #[arg(long)]
    bin: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.list {
        for scenario in all_scenarios() {
            let smoke = if scenario.smoke { "smoke" } else { "extra" };
            println!("{:<24} [{smoke}] {}", scenario.id, scenario.description);
        }
        return ExitCode::SUCCESS;
    }

    let binary = match args.bin {
        Some(path) => path,
        None => match rho_tui_pty::env::resolve_rho_binary() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("error: {error:#}");
                return ExitCode::FAILURE;
            }
        },
    };

    let mut runner = ScenarioRunner::new(binary);
    runner = runner.with_timing(args.timing);
    if let Some(root) = args.artifacts {
        runner = runner.with_artifacts(root);
    } else if let Ok(dir) = std::env::temp_dir().canonicalize() {
        runner = runner.with_artifacts(dir.join("rho-pty-artifacts"));
    }

    let names: Vec<String> = if args.smoke {
        all_scenarios()
            .iter()
            .filter(|scenario| scenario.smoke)
            .map(|scenario| scenario.id.to_string())
            .collect()
    } else if let Some(name) = args.scenario {
        vec![name]
    } else {
        eprintln!("error: provide a scenario id, --smoke, or --list");
        return ExitCode::FAILURE;
    };

    let mut failed = 0usize;
    for name in names {
        match run_named(&runner, &name) {
            Ok(outcome) if outcome.passed => {
                println!("PASS {name}");
                if args.timing {
                    for line in outcome.timing.report_lines() {
                        println!("  {line}");
                    }
                }
            }
            Ok(outcome) => {
                failed += 1;
                eprintln!("FAIL {name}\n{}", outcome.message);
            }
            Err(error) => {
                failed += 1;
                eprintln!("FAIL {name}\n{error:#}");
            }
        }
    }

    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
