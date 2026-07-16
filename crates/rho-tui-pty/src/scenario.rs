//! Scripted scenario runner.

use std::{path::PathBuf, time::Duration};

use anyhow::Result;

use crate::{
    artifacts::ArtifactWriter,
    env::{resolve_rho_binary, IsolatedHome, RhoLaunchPlan},
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    pty::PtySize,
    timing::TimingSummary,
};

/// One bounded action or assertion inside a scenario.
#[derive(Clone, Debug)]
pub enum Step {
    Phase(&'static str),
    WaitText {
        text: &'static str,
        timeout: WaitTimeout,
    },
    WaitQuiet {
        quiet_for: Duration,
        timeout: WaitTimeout,
    },
    TypeText(&'static str),
    SubmitText(&'static str),
    Key(Key),
    Paste(&'static str),
    Resize {
        rows: u16,
        cols: u16,
    },
    AssertText(&'static str),
    AssertRawContains(&'static str),
    ExitCommand,
    CtrlCExit,
    WaitExit {
        timeout: WaitTimeout,
    },
    Custom(fn(&mut PtyHarness) -> Result<()>),
}

/// Named scenario definition.
#[derive(Clone, Debug)]
pub struct Scenario {
    pub id: &'static str,
    pub description: &'static str,
    pub size: PtySize,
    pub steps: &'static [Step],
    pub smoke: bool,
}

#[derive(Clone, Debug)]
pub struct ScenarioOutcome {
    pub id: String,
    pub passed: bool,
    pub message: String,
    pub timing: TimingSummary,
    pub artifact_dir: Option<PathBuf>,
}

/// Runs scenarios against a resolved Rho binary.
pub struct ScenarioRunner {
    pub binary: PathBuf,
    pub artifact_root: Option<PathBuf>,
    pub record_timing: bool,
}

impl ScenarioRunner {
    pub fn new(binary: PathBuf) -> Self {
        let binary = binary.canonicalize().unwrap_or(binary);
        Self {
            binary,
            artifact_root: None,
            record_timing: false,
        }
    }

    pub fn from_env() -> Result<Self> {
        Ok(Self::new(resolve_rho_binary()?))
    }

    pub fn with_artifacts(mut self, root: impl Into<PathBuf>) -> Self {
        self.artifact_root = Some(root.into());
        self
    }

    pub fn with_timing(mut self, enabled: bool) -> Self {
        self.record_timing = enabled;
        self
    }

    pub fn run(&self, scenario: &Scenario) -> Result<ScenarioOutcome> {
        let home = IsolatedHome::new()?;
        let plan = RhoLaunchPlan::matrix(&self.binary, &home, scenario.size);
        let mut harness = PtyHarness::spawn_named(&plan, scenario.id)?;
        harness.enable_timing(self.record_timing);
        if let Some(root) = &self.artifact_root {
            harness.set_artifact_writer(ArtifactWriter::new(root));
        }

        let mut exited = false;
        let result = (|| -> Result<()> {
            for step in scenario.steps {
                if matches!(
                    step,
                    Step::ExitCommand | Step::CtrlCExit | Step::WaitExit { .. }
                ) {
                    exited = true;
                }
                apply_step(&mut harness, step)?;
            }
            if !exited && harness.is_running() {
                harness.quit_with_exit_command()?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => Ok(ScenarioOutcome {
                id: scenario.id.into(),
                passed: true,
                message: "ok".into(),
                timing: harness.timing().clone(),
                artifact_dir: None,
            }),
            Err(error) => {
                if harness.is_running() {
                    let _ = harness.kill();
                }
                Ok(ScenarioOutcome {
                    id: scenario.id.into(),
                    passed: false,
                    message: format!("{error:#}"),
                    timing: harness.timing().clone(),
                    artifact_dir: self.artifact_root.clone(),
                })
            }
        }
    }
}

fn apply_step(harness: &mut PtyHarness, step: &Step) -> Result<()> {
    match step {
        Step::Phase(name) => {
            harness.set_phase(*name);
            Ok(())
        }
        Step::WaitText { text, timeout } => harness.wait_for_text(text, *timeout),
        Step::WaitQuiet { quiet_for, timeout } => harness.wait_for_quiet(*quiet_for, *timeout),
        Step::TypeText(text) => harness.type_text(text),
        Step::SubmitText(text) => harness.submit_text(text),
        Step::Key(key) => harness.inject_key(key),
        Step::Paste(text) => harness.paste(text),
        Step::Resize { rows, cols } => harness.resize(*rows, *cols),
        Step::AssertText(text) => harness.assert_screen_contains(text),
        Step::AssertRawContains(text) => harness.assert_raw_contains(text.as_bytes()),
        Step::ExitCommand => {
            let code = harness.quit_with_exit_command()?;
            ensure_clean_exit(code, "ExitCommand")
        }
        Step::CtrlCExit => {
            let code = harness.quit_with_ctrl_c()?;
            ensure_clean_exit(code, "CtrlCExit")
        }
        Step::WaitExit { timeout } => {
            let code = harness.wait_for_exit(*timeout)?;
            ensure_clean_exit(code, "WaitExit")
        }
        Step::Custom(func) => func(harness),
    }
}

fn ensure_clean_exit(code: u32, step: &str) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        anyhow::bail!("{step} expected clean exit code 0, got {code}")
    }
}
