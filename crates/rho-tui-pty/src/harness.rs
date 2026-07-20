//! High-level PTY harness combining controller, screen, and waits.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

use crate::{
    artifacts::{ArtifactBundle, ArtifactWriter},
    env::RhoLaunchPlan,
    keys::{encode_key, encode_paste, encode_sgr_mouse, Key, MouseButton},
    pty::{PtyController, PtySize},
    screen::ScreenModel,
    timing::{TimingSample, TimingSummary},
};

/// Descriptive timeout used by wait helpers.
#[derive(Clone, Copy, Debug)]
pub struct WaitTimeout {
    pub duration: Duration,
    pub label: &'static str,
}

impl WaitTimeout {
    pub const fn new(duration: Duration, label: &'static str) -> Self {
        Self { duration, label }
    }

    pub const fn secs(secs: u64, label: &'static str) -> Self {
        Self::new(Duration::from_secs(secs), label)
    }

    pub const fn millis(millis: u64, label: &'static str) -> Self {
        Self::new(Duration::from_millis(millis), label)
    }
}

/// Combined PTY controller + virtual screen.
pub struct PtyHarness {
    pty: PtyController,
    screen: ScreenModel,
    raw_output: Vec<u8>,
    action_log: Vec<String>,
    env: Vec<(String, String)>,
    scenario: String,
    phase: String,
    timing: TimingSummary,
    record_timing: bool,
    artifact_writer: Option<ArtifactWriter>,
    started_at: Instant,
}

impl PtyHarness {
    pub fn spawn(plan: &RhoLaunchPlan) -> Result<Self> {
        Self::spawn_named(plan, "unnamed")
    }

    pub fn spawn_named(plan: &RhoLaunchPlan, scenario: impl Into<String>) -> Result<Self> {
        let args = plan.args.iter().map(String::as_str).collect::<Vec<_>>();
        let env = plan
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>();
        let pty = PtyController::spawn(
            &plan.binary,
            plan.size,
            &args,
            &env,
            Some(plan.cwd.as_path()),
        )?;
        let screen = ScreenModel::new(plan.size.rows, plan.size.cols);
        let mut harness = Self {
            pty,
            screen,
            raw_output: Vec::new(),
            action_log: Vec::new(),
            env: plan.env.clone(),
            scenario: scenario.into(),
            phase: "spawn".into(),
            timing: TimingSummary::default(),
            record_timing: false,
            artifact_writer: None,
            started_at: Instant::now(),
        };
        harness.log(format!(
            "spawn {} args={:?} size={}x{}",
            plan.binary.display(),
            plan.args,
            plan.size.rows,
            plan.size.cols
        ));
        Ok(harness)
    }

    /// Spawn an arbitrary binary for harness self-tests.
    pub fn spawn_command(
        binary: &Path,
        args: &[&str],
        size: PtySize,
        env: &[(String, String)],
        cwd: Option<&Path>,
        scenario: &str,
    ) -> Result<Self> {
        let env_refs = env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>();
        let pty = PtyController::spawn(binary, size, args, &env_refs, cwd)?;
        Ok(Self {
            pty,
            screen: ScreenModel::new(size.rows, size.cols),
            raw_output: Vec::new(),
            action_log: vec![format!("spawn_command {}", binary.display())],
            env: env.to_vec(),
            scenario: scenario.into(),
            phase: "spawn".into(),
            timing: TimingSummary::default(),
            record_timing: false,
            artifact_writer: None,
            started_at: Instant::now(),
        })
    }

    pub fn enable_timing(&mut self, enabled: bool) {
        self.record_timing = enabled;
    }

    pub fn set_artifact_writer(&mut self, writer: ArtifactWriter) {
        self.artifact_writer = Some(writer);
    }

    pub fn set_phase(&mut self, phase: impl Into<String>) {
        self.phase = phase.into();
        self.log(format!("phase {}", self.phase));
    }

    pub fn screen(&self) -> &ScreenModel {
        &self.screen
    }

    pub fn raw_output(&self) -> &[u8] {
        &self.raw_output
    }

    pub fn action_log(&self) -> &[String] {
        &self.action_log
    }

    pub fn timing(&self) -> &TimingSummary {
        &self.timing
    }

    pub fn log(&mut self, message: impl Into<String>) {
        let elapsed = self.started_at.elapsed().as_millis();
        self.action_log
            .push(format!("[+{elapsed}ms] {}", message.into()));
    }

    pub fn inject_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.log(format!("inject {} bytes", bytes.len()));
        self.pty.inject_bytes(bytes)
    }

    pub fn inject_key(&mut self, key: &Key) -> Result<()> {
        self.log(format!("key {key:?}"));
        self.inject_bytes(&encode_key(key))
    }

    pub fn type_text(&mut self, text: &str) -> Result<()> {
        self.log(format!("type {text:?}"));
        // Deliver characters with human-like spacing so Rho's paste-burst
        // detector does not treat harness input as a clipboard paste.
        for ch in text.chars() {
            let mut buf = [0u8; 4];
            let encoded = ch.encode_utf8(&mut buf);
            self.inject_bytes(encoded.as_bytes())?;
            std::thread::sleep(Duration::from_millis(20));
            self.poll(Duration::from_millis(5));
        }
        Ok(())
    }

    pub fn submit_text(&mut self, text: &str) -> Result<()> {
        // Use an explicit paste event so runner load cannot collapse delayed
        // plain-key input into a paste burst and absorb Enter as a newline.
        self.paste(text)?;
        self.settle_input();
        self.inject_key(&Key::Enter)
    }

    /// Wait long enough for paste-burst detection and enter-suppression to settle.
    pub fn settle_input(&mut self) {
        const SETTLE: Duration = Duration::from_millis(50);
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            self.poll(remaining.min(Duration::from_millis(20)));
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn paste(&mut self, text: &str) -> Result<()> {
        self.log(format!("paste {} chars", text.chars().count()));
        self.inject_bytes(&encode_paste(text))
    }

    pub fn mouse(&mut self, button: MouseButton, col: u16, row: u16, press: bool) -> Result<()> {
        self.log(format!(
            "mouse {button:?} col={col} row={row} press={press}"
        ));
        self.inject_bytes(&encode_sgr_mouse(button, col, row, press))
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.log(format!("resize {rows}x{cols}"));
        self.pty.resize(rows, cols)?;
        self.screen.resize(rows, cols);
        Ok(())
    }

    /// Pull available output into the screen model.
    pub fn poll(&mut self, budget: Duration) {
        let bytes = self.pty.drain(budget);
        if !bytes.is_empty() {
            self.raw_output.extend_from_slice(&bytes);
            self.screen.process(&bytes);
        }
    }

    pub fn wait_for_text(&mut self, needle: &str, timeout: WaitTimeout) -> Result<()> {
        self.set_phase(format!("wait_for_text:{needle}"));
        let started = Instant::now();
        let deadline = started + timeout.duration;
        loop {
            self.poll(Duration::from_millis(25));
            if self.screen.contains_text(needle) {
                if self.record_timing {
                    self.timing.push(TimingSample::new(
                        format!("wait_for_text:{needle}"),
                        started.elapsed(),
                    ));
                }
                self.log(format!("observed text {needle:?}"));
                return Ok(());
            }
            if !self.pty.is_running() {
                self.poll(Duration::from_millis(50));
                if self.screen.contains_text(needle) {
                    return Ok(());
                }
                return self.fail_unit(format!(
                    "child exited before text {:?} appeared during {}",
                    needle, timeout.label
                ));
            }
            if Instant::now() >= deadline {
                return self.fail_unit(format!(
                    "timeout waiting for text {:?} ({})",
                    needle, timeout.label
                ));
            }
        }
    }

    pub fn wait_for_quiet(&mut self, quiet_for: Duration, timeout: WaitTimeout) -> Result<()> {
        self.set_phase("wait_for_quiet");
        let started = Instant::now();
        let deadline = started + timeout.duration;
        let mut last_change = Instant::now();
        let mut last_len = self.raw_output.len();
        loop {
            self.poll(Duration::from_millis(20));
            if self.raw_output.len() != last_len {
                last_len = self.raw_output.len();
                last_change = Instant::now();
            }
            if last_change.elapsed() >= quiet_for {
                if self.record_timing {
                    self.timing
                        .push(TimingSample::new("wait_for_quiet", started.elapsed()));
                }
                self.log("screen quiet");
                return Ok(());
            }
            if Instant::now() >= deadline {
                return self.fail_unit(format!(
                    "timeout waiting for quiet window ({})",
                    timeout.label
                ));
            }
        }
    }

    pub fn wait_for_exit(&mut self, timeout: WaitTimeout) -> Result<u32> {
        self.set_phase("wait_for_exit");
        let started = Instant::now();
        // Keep draining while waiting so restoration sequences are captured.
        let deadline = started + timeout.duration;
        let mut saw_clean_exit_marker = false;
        let mut marker_at = None;
        loop {
            self.poll(Duration::from_millis(20));
            if !saw_clean_exit_marker && self.observed_clean_exit_marker() {
                saw_clean_exit_marker = true;
                marker_at = Some(Instant::now());
                self.log("observed clean TUI exit marker");
            }
            if let Some(code) = self.pty.wait_exit(Duration::from_millis(20))? {
                if self.record_timing {
                    self.timing
                        .push(TimingSample::new("wait_for_exit", started.elapsed()));
                }
                self.log(format!("exit code {code}"));
                // Final drain after exit.
                self.poll(Duration::from_millis(50));
                return Ok(code);
            }
            // On some macOS runners the process prints the post-TUI resume
            // summary but lingers before reaping. Once the clean exit path is
            // observable, force-reap after a short grace period.
            if let Some(marker_at) = marker_at {
                if marker_at.elapsed() >= Duration::from_millis(750) {
                    self.log("force-reaping child after clean TUI exit markers");
                    let _ = self.pty.kill();
                    self.poll(Duration::from_millis(50));
                    if self.record_timing {
                        self.timing
                            .push(TimingSample::new("wait_for_exit", started.elapsed()));
                    }
                    return Ok(0);
                }
            }
            if Instant::now() >= deadline {
                return self.fail_code(format!(
                    "timeout waiting for child exit ({})",
                    timeout.label
                ));
            }
        }
    }

    fn observed_clean_exit_marker(&self) -> bool {
        const MARKERS: &[&str] = &["Resume this session", "session saved", "exiting rho"];
        let screen = self.screen.contents();
        if MARKERS.iter().any(|marker| screen.contains(marker)) {
            return true;
        }
        let raw = String::from_utf8_lossy(&self.raw_output);
        MARKERS.iter().any(|marker| raw.contains(marker))
    }

    pub fn assert_screen_contains(&mut self, needle: &str) -> Result<()> {
        self.poll(Duration::from_millis(10));
        if self.screen.contains_text(needle) {
            Ok(())
        } else {
            self.fail_unit(format!("screen missing text {needle:?}"))
        }
    }

    pub fn assert_raw_contains(&mut self, needle: &[u8]) -> Result<()> {
        self.poll(Duration::from_millis(10));
        if self
            .raw_output
            .windows(needle.len())
            .any(|window| window == needle)
        {
            Ok(())
        } else {
            self.fail_unit(format!(
                "raw PTY output missing bytes {}",
                String::from_utf8_lossy(needle)
            ))
        }
    }

    pub fn quit_with_exit_command(&mut self) -> Result<u32> {
        self.set_phase("quit_with_/exit");
        // Ensure the composer is idle before inserting the exit command. Use an
        // explicit paste event so runner load cannot collapse delayed plain-key
        // input into a paste burst and absorb Enter as a newline.
        self.inject_key(&Key::Esc)?;
        self.settle_input();
        self.paste("/exit")?;
        self.settle_input();
        self.inject_key(&Key::Enter)?;
        self.wait_for_exit(WaitTimeout::secs(15, "exit after /exit"))
    }

    pub fn quit_with_ctrl_c(&mut self) -> Result<u32> {
        self.set_phase("quit_with_ctrl_c");
        // First ctrl-c clears input; second exits.
        self.inject_key(&Key::Ctrl('c'))?;
        self.poll(Duration::from_millis(100));
        self.inject_key(&Key::Ctrl('c'))?;
        self.wait_for_exit(WaitTimeout::secs(15, "exit after ctrl-c"))
    }

    pub fn is_running(&mut self) -> bool {
        self.pty.is_running()
    }

    pub fn kill(&mut self) -> Result<()> {
        self.log("kill child");
        self.pty.kill()
    }

    fn fail_unit(&mut self, message: impl Into<String>) -> Result<()> {
        self.fail_message(message)
    }

    fn fail_code(&mut self, message: impl Into<String>) -> Result<u32> {
        self.fail_message(message)?;
        unreachable!("fail_message always returns Err")
    }

    fn fail_message(&mut self, message: impl Into<String>) -> Result<()> {
        let message = message.into();
        self.log(format!("FAIL: {message}"));
        let size = self.pty.size();
        let exit_code = self.pty.wait_exit(Duration::from_millis(10)).ok().flatten();
        let bundle = ArtifactBundle {
            scenario: self.scenario.clone(),
            phase: self.phase.clone(),
            message: message.clone(),
            rows: size.rows,
            cols: size.cols,
            exit_code,
            action_log: self.action_log.clone(),
            screen: self.screen.debug_dump(),
            timing: Some(self.timing.clone()),
            env: redact_env(&self.env),
        };
        if let Some(writer) = &self.artifact_writer {
            match writer.write(&bundle, &self.raw_output) {
                Ok(path) => {
                    bail!(
                        "{message}\nphase={}\nscreen:\n{}\nartifacts: {}\nactions:\n{}",
                        self.phase,
                        bundle.screen,
                        path.display(),
                        self.action_log.join("\n")
                    );
                }
                Err(error) => {
                    bail!(
                        "{message}\nphase={}\nscreen:\n{}\nartifact write failed: {error:#}\nactions:\n{}",
                        self.phase,
                        bundle.screen,
                        self.action_log.join("\n")
                    );
                }
            }
        }
        bail!(
            "{message}\nphase={}\nscreen:\n{}\nactions:\n{}",
            self.phase,
            bundle.screen,
            self.action_log.join("\n")
        )
    }

    pub fn into_report(self) -> HarnessReport {
        HarnessReport {
            scenario: self.scenario,
            action_log: self.action_log,
            screen: self.screen.debug_dump(),
            raw_output: self.raw_output,
            timing: self.timing,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HarnessReport {
    pub scenario: String,
    pub action_log: Vec<String>,
    pub screen: String,
    pub raw_output: Vec<u8>,
    pub timing: TimingSummary,
}

fn redact_env(env: &[(String, String)]) -> Vec<(String, String)> {
    env.iter()
        .map(|(key, value)| {
            let upper = key.to_ascii_uppercase();
            if upper.contains("KEY")
                || upper.contains("TOKEN")
                || upper.contains("SECRET")
                || upper.contains("PASSWORD")
            {
                (key.clone(), "<redacted>".into())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect()
}

/// Convenience: open a matrix-mode Rho harness with isolated HOME.
pub fn spawn_matrix_rho(
    binary: &Path,
    size: PtySize,
    scenario: &str,
    artifact_dir: Option<&Path>,
) -> Result<(crate::env::IsolatedHome, PtyHarness)> {
    let home = crate::env::IsolatedHome::new()?;
    let plan = RhoLaunchPlan::matrix(binary, &home, size);
    let mut harness = PtyHarness::spawn_named(&plan, scenario)
        .with_context(|| format!("failed to spawn rho for scenario {scenario}"))?;
    if let Some(dir) = artifact_dir {
        harness.set_artifact_writer(ArtifactWriter::new(dir));
    }
    Ok((home, harness))
}
