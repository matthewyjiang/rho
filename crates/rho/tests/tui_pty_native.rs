//! Small native-PTY product gate shared by Unix and Windows ConPTY.
//! The larger Unix suite intentionally keeps its shell/signal-specific scenarios.

#![cfg(any(unix, windows))]

use std::path::PathBuf;

use anyhow::{ensure, Result};
use rho_tui_pty::{
    ArtifactWriter, IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout,
};

// Reuse the existing startup/paste-response and exit budgets from the Unix
// scenarios. These are failure bounds, not delays or Windows latency claims.
const STARTUP: WaitTimeout = WaitTimeout::secs(20, "native PTY startup");
const RESPONSE: WaitTimeout = WaitTimeout::secs(20, "native PTY paste response");
const EXIT: WaitTimeout = WaitTimeout::secs(10, "native PTY exit");

// Covers: cargo-built Windows Rho must start in ConPTY and preserve multiline
// Unicode paste as one draft, without submitting embedded Enter or slash text.
// Owner: interactive TUI. The existing product PTY suite is Unix-only.
#[test]
fn native_pty_startup_multiline_paste_exit() -> Result<()> {
    let home = IsolatedHome::new()?;
    let plan = RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        &home,
        PtySize::new(28, 100),
    );
    let mut harness = PtyHarness::spawn_named(&plan, "native_startup_multiline_paste_exit")?;
    let artifacts = std::env::var_os("RHO_PTY_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rho-pty-native-artifacts"));
    harness.set_artifact_writer(ArtifactWriter::new(artifacts));
    harness.wait_for_text("gpt-5.5", STARTUP)?;

    harness.paste("café first line\n/not-a-command\nlast λ line")?;
    // The final line stays in the composer until we advance. Seeing it proves
    // the complete paste was processed, unlike a quiet-period timing guess.
    harness.wait_for_text("last λ line", RESPONSE)?;
    harness.wait_for_text("/not-a-command", RESPONSE)?;
    ensure!(
        !harness.screen().contains_text("fixture response:"),
        "embedded paste Enter submitted a turn:\n{}",
        harness.screen().contents()
    );
    // Navigation must still decode after enabling Windows VT input. Home and
    // a bracketed insertion produce a visible edit before the explicit Enter.
    harness.inject_key(&Key::Home)?;
    harness.paste("edited ")?;
    harness.wait_for_text("edited café first line", RESPONSE)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("fixture response: edited café first line", RESPONSE)?;

    // Avoid submit_text's settling delay: the observed command is our ack.
    harness.paste("/exit")?;
    harness.wait_for_text("/exit", EXIT)?;
    harness.inject_key(&Key::Enter)?;
    let code = harness.wait_for_exit(EXIT)?;
    ensure!(code == 0, "native PTY Rho exited with code {code}");
    // Drop must also complete: ClosePseudoConsole flushes output synchronously.
    drop(harness);
    Ok(())
}
