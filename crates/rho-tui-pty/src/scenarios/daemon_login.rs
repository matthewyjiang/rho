//! `/login` and `/logout` against a daemon session: the daemon saves and
//! deletes the credential its sessions read.

use std::{
    io::{Read, Write},
    net::TcpStream,
};

use anyhow::{ensure, Context, Result};

use super::{daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_login",
    "Save an API key and sign out through the daemon that runs the session",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

/// The onboarding's default base URL, replaced by one nothing listens on.
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000/v1";

/// Starts the daemon's offline login to `xai-oauth`, with the device code
/// the flow picker suggests in a remote shell.
fn start_fixture_login(harness: &mut PtyHarness) -> Result<()> {
    harness.submit_text("/login xai-oauth")?;
    harness.wait_for_text("Select SpaceXAI login flow", SETTLE)?;
    harness.inject_key(&Key::Enter)
}

/// Approves the fixture login whose prompt `screen` shows, as a person
/// opening its URL would.
fn approve(screen: &str) -> Result<()> {
    // The newest prompt is the last on the screen.
    let port: String = screen
        .rsplit("http://127.0.0.1:")
        .next()
        .context("the login prompt shows the fixture's URL")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let mut page = TcpStream::connect(format!("127.0.0.1:{port}"))?;
    page.write_all(b"GET /approve HTTP/1.1\r\nhost: localhost\r\n\r\n")?;
    // Read to the end, so the fixture is done with the request.
    let mut answer = String::new();
    let _ = page.read_to_string(&mut answer);
    Ok(())
}

// Covers: an API key entered at `/login` is checked and saved by the
// daemon, which says so when the host cannot be asked about it, and
// `/logout` has the daemon delete it; an interactive login the daemon runs
// shows its prompt here, Esc cancels it, and its approval finishes it.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_env("RHO_TUI_DAEMON", "1")
        // The offline login the daemon serves for `xai-oauth`.
        .with_env("RHO_AUTOMATION_TEST_LOGIN", "1")
        // A remote shell: the TUI shows the login's URL and opens no browser.
        .with_env("SSH_CONNECTION", "pty");
    let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> Result<()> {
        harness.set_phase("custom_host");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("/login")?;
        harness.wait_for_text("Select provider to login", SETTLE)?;
        harness.type_into_field("Chat Completions")?;
        harness.wait_for_text("Custom · Chat Completions", SETTLE)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("edit provider name", SETTLE)?;
        harness.type_into_field("vllm")?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("edit base URL", SETTLE)?;
        // The discard port: the daemon's question about the key gets no
        // answer, whatever else runs on this machine.
        for _ in 0..DEFAULT_BASE_URL.len() {
            harness.inject_key(&Key::Backspace)?;
        }
        harness.type_into_field("http://127.0.0.1:9/v1")?;
        harness.inject_key(&Key::Enter)?;

        harness.set_phase("daemon_saves_the_key");
        harness.wait_for_text("enter API key (optional)", SETTLE)?;
        harness.type_text("sk-local-key")?;
        harness.wait_for_text("••••••••••••", SETTLE)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("saved the vllm key without checking it", STARTUP)?;
        harness.wait_for_text("stored credentials for vllm", STARTUP)?;

        harness.set_phase("daemon_signs_out");
        harness.submit_text("/logout vllm")?;
        harness.wait_for_text("logged out of vllm", SETTLE)?;

        harness.set_phase("daemon_login_cancelled");
        start_fixture_login(&mut harness)?;
        harness.wait_for_text("FIXTURE-CODE", STARTUP)?;
        harness.wait_for_text("c copy  Esc cancel", SETTLE)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("login cancelled", SETTLE)?;

        harness.set_phase("daemon_login_approved");
        start_fixture_login(&mut harness)?;
        harness.wait_for_text("c copy  Esc cancel", STARTUP)?;
        approve(&harness.screen().contents())?;
        harness.wait_for_text("stored credentials for xai", STARTUP)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the session exited with code {code}");
        Ok(())
    })();

    if result.is_err() && harness.is_running() {
        let _ = harness.kill();
    }
    Ok(ScenarioOutcome {
        id: SCENARIO.id.into(),
        passed: result.is_ok(),
        message: result
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default(),
        timing: harness.timing().clone(),
        artifact_dir: runner.artifact_root.clone(),
    })
}
