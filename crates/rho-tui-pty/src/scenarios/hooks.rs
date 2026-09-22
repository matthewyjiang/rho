//! `/hooks` opens a single-pane overlay with the resolved spawn contract.
//!
//! Trusting a workspace means trusting the programs listed here, so the argv,
//! working directory, and timeout a hook will actually run with have to reach
//! the screen. That contract belongs in the overlay, not a transcript notice
//! that stays after dismiss.

use std::fs;

use anyhow::{Context, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

pub(super) const HOOKS_CONTRACT_SCENARIO: Scenario = Scenario::new(
    "hooks_contract",
    "Open the hooks overlay and show the resolved spawn contract",
    PtySize {
        rows: 32,
        cols: 140,
    },
    HOOKS_CONTRACT_STEPS,
    /* smoke */ false,
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
        text: "2s",
        timeout: SETTLE,
    },
    Step::Custom(assert_hooks_overlay_is_single_pane),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "user:deny-force-push",
        timeout: SETTLE,
    },
    Step::Custom(assert_hooks_overlay_dismissed),
    Step::ExitCommand,
];

fn assert_hooks_overlay_is_single_pane(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_hidden_cursor(SETTLE)?;
    let screen = harness.screen().contents();
    if !screen.contains("Hooks") {
        anyhow::bail!("hooks overlay title missing:\n{screen}");
    }
    if screen.contains("Search") || screen.contains("DETAILS") {
        anyhow::bail!("hooks overlay used picker chrome:\n{screen}");
    }
    if !harness.screen().hide_cursor() {
        anyhow::bail!(
            "hooks overlay must hide the terminal caret, cursor at {:?}:\n{screen}",
            harness.screen().cursor()
        );
    }
    Ok(())
}

fn assert_hooks_overlay_dismissed(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_visible_cursor(SETTLE)?;
    let screen = harness.screen().contents();
    if screen.contains("Hooks") {
        anyhow::bail!("hooks overlay still visible after Esc:\n{screen}");
    }
    if screen.contains("deny-force-push.sh") {
        anyhow::bail!("hook contract stayed in the transcript after Esc:\n{screen}");
    }
    if !screen.contains("gpt-5.5") {
        anyhow::bail!("session chrome missing after dismissing hooks:\n{screen}");
    }
    if harness.screen().hide_cursor() {
        anyhow::bail!("composer caret still hidden after dismissing hooks:\n{screen}");
    }
    Ok(())
}

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
