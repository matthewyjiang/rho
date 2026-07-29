use std::fs;

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

fn setup_workspace_rewind(home: &IsolatedHome) -> Result<()> {
    let mut config = fs::read_to_string(&home.config_path)?;
    if !config.ends_with('\n') {
        config.push('\n');
    }
    config.push_str("experimental_workspace_rewind = true\n");
    fs::write(&home.config_path, config)?;
    Ok(())
}

pub(super) const WORKSPACE_REWIND_SCENARIO: Scenario = Scenario::new(
    "workspace_rewind",
    "Preview, cancel, and confirm a native workspace rewind",
    PtySize {
        rows: 30,
        cols: 120,
    },
    WORKSPACE_REWIND_STEPS,
    false,
)
.with_setup(setup_workspace_rewind);

const WORKSPACE_REWIND_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("capture_native_write"),
    Step::SubmitText("fixture tool"),
    Step::WaitText {
        text: "tool lifecycle complete with one result",
        timeout: STREAM,
    },
    Step::Phase("preview_and_cancel"),
    Step::SubmitText("/rewind"),
    Step::WaitText {
        text: "Workspace rewind",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Confirm workspace rewind",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "delete  .rho-tui-fixture-output.txt",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: std::time::Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::SubmitText(
        "!test \"$(cat .rho-tui-fixture-output.txt)\" = 'deterministic tool output' && echo cancel-preserved",
    ),
    Step::WaitText {
        text: "cancel-preserved",
        timeout: SETTLE,
    },
    Step::Phase("show_conflict"),
    Step::SubmitText("!printf external > .rho-tui-fixture-output.txt && echo external-ready"),
    Step::WaitText {
        text: "external-ready",
        timeout: STREAM,
    },
    Step::SubmitText("/rewind"),
    Step::WaitText {
        text: "Workspace rewind",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "conflict  .rho-tui-fixture-output.txt",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "conversation state was not selected",
        timeout: SETTLE,
    },
    Step::SubmitText(
        "!test \"$(cat .rho-tui-fixture-output.txt)\" = external && echo conflict-preserved",
    ),
    Step::WaitText {
        text: "conflict-preserved",
        timeout: SETTLE,
    },
    Step::Phase("restore_expected_state"),
    Step::SubmitText(
        "!printf 'deterministic tool output\\n' > .rho-tui-fixture-output.txt && echo reset-ready",
    ),
    Step::WaitText {
        text: "reset-ready",
        timeout: STREAM,
    },
    Step::Phase("confirm_restore"),
    Step::SubmitText("/rewind"),
    Step::WaitText {
        text: "Workspace rewind",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "delete  .rho-tui-fixture-output.txt",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "workspace rewind audit; conversation state selected",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "delete  .rho-tui-fixture-output.txt  restored",
        timeout: SETTLE,
    },
    Step::SubmitText(
        "!test ! -e .rho-tui-fixture-output.txt && echo rewind-delete-confirmed",
    ),
    Step::WaitText {
        text: "rewind-delete-confirmed",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];
