//! `/hooks` shows the resolved spawn contract for configured hooks.
//!
//! Trusting a workspace means trusting the programs listed here, so the argv,
//! working directory, and timeout a hook will actually run with have to reach
//! the screen. That is an interactive guarantee, not a rendering detail.

use std::fs;

use anyhow::{Context, Result};

use crate::{
    env::IsolatedHome,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

pub(super) const HOOKS_CONTRACT_SCENARIO: Scenario = Scenario::new(
    "hooks_contract",
    "Show the resolved spawn contract for a configured hook",
    PtySize {
        rows: 30,
        cols: 120,
    },
    HOOKS_CONTRACT_STEPS,
    false,
)
.with_setup(setup_hooks);

const HOOKS_CONTRACT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("show_contract"),
    Step::SubmitText("/hooks"),
    // The hook ID, its resolved argv, and its timeout are the three facts a
    // user needs before granting trust.
    Step::WaitText {
        text: "user:deny-force-push",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "deny-force-push.sh",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "timeout: 2s",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

fn setup_hooks(home: &IsolatedHome) -> Result<()> {
    let rho_dir = home.home.join(".rho");
    let program = rho_dir.join("deny-force-push.sh");
    fs::write(&program, "#!/bin/sh\nexit 0\n").context("failed to write the hook program")?;
    fs::write(
        rho_dir.join("hooks.toml"),
        format!(
            r#"version = 1

[[hook]]
id = "deny-force-push"
on = "before_tool_use"
tools = ["bash"]
command = ["/bin/sh", "{}"]
timeout = "2s"
"#,
            program.display()
        ),
    )
    .context("failed to write the isolated hooks.toml")?;
    Ok(())
}
