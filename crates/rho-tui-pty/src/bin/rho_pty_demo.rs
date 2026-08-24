//! Capture a deterministic Rho TUI frame from the PTY harness and write SVG.
//!
//! Usage:
//!   cargo build -p rho-coding-agent
//!   cargo run -p rho-tui-pty --bin rho-pty-demo -- \
//!     --bin target/debug/rho \
//!     --output docs/assets/rho-ui-demo.svg \
//!     --output docs/public/assets/rho-ui-demo.svg \
//!     --light-output docs/public/assets/rho-ui-demo-light.svg

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
    svg::{render_screen_svg, SvgOptions, SvgPalette},
};

/// Default terminal size for the docs proof plate.
/// Tall enough to keep header, tool cards, and the final answer in frame.
const DEMO_SIZE: PtySize = PtySize {
    rows: 40,
    cols: 100,
};

const DEMO_MODEL: &str = "gpt-5.6-sol";
const DEMO_PROMPT: &str = "Add request IDs to API logs and update the tests.";
/// Stable header version for the proof plate. Not the live package version -
/// release bumps must not force SVG regen.
const DEMO_DISPLAY_VERSION: &str = "1.0.0";

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

    /// Write the dark SVG to this path. Repeat to mirror into docs/public.
    #[arg(long = "output", required = true)]
    outputs: Vec<PathBuf>,

    /// Write the light SVG to this path (docs site public asset).
    #[arg(long = "light-output")]
    light_outputs: Vec<PathBuf>,

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
        write_text(path, &capture.screen_text)?;
        println!("wrote {}", path.display());
    }

    if args.check {
        let mut failed = false;
        failed |= check_outputs(&args.outputs, &capture.dark_svg, "dark")?;
        if !args.light_outputs.is_empty() {
            failed |= check_outputs(&args.light_outputs, &capture.light_svg, "light")?;
        }
        if failed {
            bail!("one or more SVG outputs drifted from the live PTY capture");
        }
        return Ok(());
    }

    write_outputs(&args.outputs, &capture.dark_svg)?;
    write_outputs(&args.light_outputs, &capture.light_svg)?;
    Ok(())
}

fn check_outputs(paths: &[PathBuf], expected_svg: &str, label: &str) -> Result<bool> {
    let mut failed = false;
    for expected_path in paths {
        let expected = fs::read_to_string(expected_path).with_context(|| {
            format!(
                "failed to read existing {label} SVG for --check: {}",
                expected_path.display()
            )
        })?;
        if expected != expected_svg {
            failed = true;
            eprintln!(
                "SVG drift at {}\nregenerate with:\n  bash scripts/check_docs_ui_demo.sh --write",
                expected_path.display()
            );
        } else {
            println!("OK {}", expected_path.display());
        }
    }
    Ok(failed)
}

fn write_outputs(paths: &[PathBuf], svg: &str) -> Result<()> {
    for path in paths {
        write_text(path, svg)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn write_text(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

struct DemoCapture {
    dark_svg: String,
    light_svg: String,
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
    // Pin the header version at the capture input so release bumps do not force
    // proof-plate regen and the plate does not show a placeholder 0.0.0.
    plan = plan.with_env("RHO_TUI_DISPLAY_VERSION", DEMO_DISPLAY_VERSION);
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
    // Title and context usage lag the final assistant text under CI load. The
    // checked-in plate includes both, and pin_statusline_usage only rewrites a
    // present `K (…%)` run - so wait until they paint before settling.
    harness.wait_for_text("session titled: Request ID middleware", STREAM)?;
    harness.wait_for_text("K (", STREAM)?;
    harness.wait_for_quiet(Duration::from_millis(500), SETTLE)?;

    let screen = harness.screen();
    for needle in [
        DEMO_PROMPT,
        "read_file(",
        "edit(",
        "check-request-id.sh",
        "request_id",
        DEMO_MODEL,
        "~/rho",
        DEMO_DISPLAY_VERSION,
        "session titled: Request ID middleware",
        "K (",
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

    let description: String =
        "Rho interactive TUI captured from the deterministic PTY harness after a request-ID middleware turn."
            .into();
    let dark_options = SvgOptions {
        description: description.clone(),
        ..SvgOptions::with_palette(SvgPalette::github_dark())
    };
    let light_options = SvgOptions {
        description,
        ..SvgOptions::with_palette(SvgPalette::primer_light())
    };
    let dark_svg = stabilize_demo_svg(&render_screen_svg(screen, &dark_options));
    let light_svg = stabilize_demo_svg(&render_screen_svg(screen, &light_options));
    let screen_text = screen.debug_dump();

    // Leave cleanly so the child does not linger under the generator.
    let _ = harness.quit_with_exit_command();
    Ok(DemoCapture {
        dark_svg,
        light_svg,
        screen_text,
    })
}

/// Pin load-volatile status and duration fragments so CI matches local captures.
///
/// Tool durations and statusline usage are split across styled SVG text runs, so
/// stabilize by rewriting those fragments rather than whole-row matches. Header
/// package version is pinned at capture input via `RHO_TUI_DISPLAY_VERSION`.
fn stabilize_demo_svg(svg: &str) -> String {
    // Styled tool-card runs look like ` · 0.2s`, not contiguous `exit 0 · 0.2s`.
    let mut out = pin_prefixed_duration(svg, " · ", "0.1s");
    out = pin_duration_receipts(&out);
    out = pin_statusline_usage(&out);
    out
}

fn pin_duration_receipts(svg: &str) -> String {
    let mut out = svg.to_string();
    for prefix in ["Worked for ", "Thought for "] {
        out = pin_prefixed_duration(&out, prefix, "0.2s");
    }
    out
}

fn pin_prefixed_duration(svg: &str, prefix: &str, pinned: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(idx) = rest.find(prefix) {
        out.push_str(&rest[..idx + prefix.len()]);
        rest = &rest[idx + prefix.len()..];
        if let Some(end) = rest.find('s') {
            let token = &rest[..=end];
            if is_duration_token(token) {
                out.push_str(pinned);
                rest = &rest[end + 1..];
                continue;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_duration_token(token: &str) -> bool {
    let Some(body) = token.strip_suffix('s') else {
        return false;
    };
    if body.is_empty() {
        return false;
    }
    let mut seen_dot = false;
    for ch in body.chars() {
        match ch {
            '0'..='9' => {}
            '.' if !seen_dot => seen_dot = true,
            _ => return false,
        }
    }
    true
}

fn pin_statusline_usage(svg: &str) -> String {
    // Example: `6.0K (0.6%)` followed by trailing spaces in one text run.
    let mut out = String::with_capacity(svg.len());
    let bytes = svg.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some((end, spaces)) = match_usage_run(&svg[i..]) {
            out.push_str("6.0K (0.6%)");
            out.push_str(spaces);
            i += end;
            continue;
        }
        out.push(svg[i..].chars().next().unwrap());
        i += svg[i..].chars().next().unwrap().len_utf8();
    }
    out
}

fn match_usage_run(rest: &str) -> Option<(usize, &str)> {
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    // digits
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return None;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // optional .digits
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    if i + 1 >= bytes.len() || &rest[i..i + 2] != "K " {
        return None;
    }
    i += 2;
    if i >= bytes.len() || bytes[i] != b'(' {
        return None;
    }
    i += 1;
    let pct_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    if i == pct_start || i >= bytes.len() || bytes[i] != b'%' {
        return None;
    }
    i += 1;
    if i >= bytes.len() || bytes[i] != b')' {
        return None;
    }
    i += 1;
    let space_start = i;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    if i == space_start {
        return None;
    }
    Some((i, &rest[space_start..i]))
}
