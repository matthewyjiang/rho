//! Capture a deterministic Rho TUI frame from the PTY harness and write SVG.
//!
//! Usage:
//!   cargo build -p rho-coding-agent
//!   cargo run -p rho-tui-pty --bin rho-pty-demo -- \
//!     --bin target/debug/rho \
//!     --output docs/assets/rho-ui-demo.svg \
//!     --output docs/public/assets/rho-ui-demo.svg

use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use clap::Parser;
use rho_tui_pty::{
    env::{resolve_rho_binary, IsolatedHome, RhoLaunchPlan},
    harness::{PtyHarness, WaitTimeout},
    pty::PtySize,
    svg::{render_screen_svg, SvgOptions},
};

/// Default terminal size for the docs proof plate.
/// Tall enough to keep header, tool cards, and the final answer in frame.
const DEMO_SIZE: PtySize = PtySize {
    rows: 40,
    cols: 100,
};

const DEMO_MODEL: &str = "gpt-5.6-sol";
const DEMO_PROMPT: &str = "Add request IDs to API logs and update the tests.";

const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const STREAM: WaitTimeout = WaitTimeout::secs(30, "stream response");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

const DEMO_CONFIG: &str = r#"provider = "openai"
model = "gpt-5.6-sol"
auth = "api-key"
check_for_updates = false
web_search_provider = "disabled"

[behavior]
# Keep matrix runs on the isolated file store so /login never prompts and
# never touches the developer OS keyring.
credential_store = "file"
"#;

#[derive(Debug, Parser)]
#[command(
    name = "rho-pty-demo",
    about = "Capture the docs Interactive TUI proof plate from a PTY session"
)]
struct Args {
    /// Override the Rho binary path (must be a debug build for matrix mode).
    #[arg(long)]
    bin: Option<PathBuf>,

    /// Write the SVG to this path. Repeat to mirror into docs/public.
    #[arg(long = "output", required = true)]
    outputs: Vec<PathBuf>,

    /// Compare against existing outputs and exit non-zero on drift.
    #[arg(long)]
    check: bool,

    /// Optional path for a plain-text screen dump (debug aid).
    #[arg(long)]
    screen_text: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    if args.outputs.is_empty() {
        bail!("provide at least one --output path");
    }

    let binary = match args.bin {
        Some(path) => path
            .canonicalize()
            .with_context(|| format!("could not resolve --bin {}", path.display()))?,
        None => resolve_rho_binary()?,
    };

    let capture = capture_demo(&binary)?;

    if let Some(path) = &args.screen_text {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(path, &capture.screen_text)
            .with_context(|| format!("failed to write {}", path.display()))?;
        println!("wrote {}", path.display());
    }

    if args.check {
        let mut failed = false;
        for expected_path in &args.outputs {
            let expected = fs::read_to_string(expected_path).with_context(|| {
                format!(
                    "failed to read existing SVG for --check: {}",
                    expected_path.display()
                )
            })?;
            if expected != capture.svg {
                failed = true;
                eprintln!(
                    "SVG drift at {}\nregenerate with:\n  cargo run -p rho-tui-pty --bin rho-pty-demo -- --bin target/debug/rho --output docs/assets/rho-ui-demo.svg --output docs/public/assets/rho-ui-demo.svg",
                    expected_path.display()
                );
            } else {
                println!("OK {}", expected_path.display());
            }
        }
        if failed {
            bail!("one or more SVG outputs drifted from the live PTY capture");
        }
        return Ok(());
    }

    for path in &args.outputs {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(path, &capture.svg)
            .with_context(|| format!("failed to write {}", path.display()))?;
        println!("wrote {}", path.display());
    }

    Ok(())
}

struct DemoCapture {
    svg: String,
    screen_text: String,
}

fn capture_demo(binary: &Path) -> Result<DemoCapture> {
    let home = IsolatedHome::new()?;
    // Keep the statusline cwd under HOME so the plate shows a stable `~/rho`.
    let workspace = home.home.join("rho");
    fs::create_dir_all(&workspace)
        .with_context(|| format!("failed to create demo workspace {}", workspace.display()))?;
    // Demo-only model label for the statusline; leave shared PTY defaults alone.
    fs::write(&home.config_path, DEMO_CONFIG)
        .with_context(|| format!("failed to write demo config {}", home.config_path.display()))?;

    let mut plan = RhoLaunchPlan::matrix(binary, &home, DEMO_SIZE);
    plan.cwd = workspace;
    // Set the model in config only. A CLI `--model` flag forces a credentialed
    // catalog refresh and exits before matrix fixture mode can attach.

    let mut harness = PtyHarness::spawn_named(&plan, "docs_ui_demo")?;
    harness.set_phase("startup");
    harness.wait_for_text("rho", STARTUP)?;
    harness.wait_for_text(DEMO_MODEL, STARTUP)?;

    // One natural user turn drives read -> edit -> bash -> final answer.
    harness.set_phase("docs_demo_turn");
    harness.submit_text(DEMO_PROMPT)?;
    harness.wait_for_text(
        "Focused tests cover both generated and forwarded IDs.",
        STREAM,
    )?;
    harness.wait_for_quiet(Duration::from_millis(300), SETTLE)?;

    let screen = harness.screen();
    for needle in [
        DEMO_PROMPT,
        "read_file(",
        "edit(",
        "check-request-id.sh",
        "request_id",
        DEMO_MODEL,
        "~/rho",
    ] {
        if !screen.contains_text(needle) {
            bail!("demo frame missing {needle:?}:\n{}", screen.debug_dump());
        }
    }
    if screen.contains_text("fixture ") {
        bail!(
            "demo frame still shows fixture-test wording:\n{}",
            screen.debug_dump()
        );
    }

    let options = SvgOptions {
        description: "Rho interactive TUI captured from the deterministic PTY harness after a request-ID middleware turn.".into(),
        ..SvgOptions::default()
    };
    let svg = stabilize_demo_svg(&render_screen_svg(screen, &options));
    let screen_text = screen.debug_dump();

    // Leave cleanly so the child does not linger under the generator.
    let _ = harness.quit_with_exit_command();
    Ok(DemoCapture { svg, screen_text })
}

/// Pin wall-clock tool durations so the SVG stays load-stable.
fn stabilize_demo_svg(svg: &str) -> String {
    let marker = "exit 0 · ";
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(idx) = rest.find(marker) {
        out.push_str(&rest[..idx]);
        out.push_str(marker);
        out.push_str("0.1s");
        rest = &rest[idx + marker.len()..];
        if let Some(end) = rest.find('s') {
            let token = &rest[..=end];
            if token
                .chars()
                .all(|ch| ch.is_ascii_digit() || ch == '.' || ch == 's')
            {
                rest = &rest[end + 1..];
                continue;
            }
        }
        break;
    }
    out.push_str(rest);
    out
}
