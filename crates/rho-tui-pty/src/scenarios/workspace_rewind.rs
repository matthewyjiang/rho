use std::fs;

use anyhow::{ensure, Context, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{assert_helpers::wait_for_turn_completion_after, SETTLE, STARTUP, STREAM};

fn setup_workspace_rewind_off(home: &IsolatedHome) -> Result<()> {
    let mut config = fs::read_to_string(&home.config_path)?;
    if !config.ends_with('\n') {
        config.push('\n');
    }
    config.push_str("workspace_rewind = false\n");
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
);

pub(super) const WORKSPACE_REWIND_OFF_SCENARIO: Scenario = Scenario::new(
    "workspace_rewind_off",
    "Explicit opt-out disables workspace rewind",
    PtySize {
        rows: 30,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture tool"),
        Step::Custom(|harness| {
            wait_for_turn_completion_after(harness, "tool lifecycle complete with one result")
        }),
        Step::SubmitText("/rewind"),
        Step::WaitText {
            text: "workspace rewind is off",
            timeout: SETTLE,
        },
        Step::ExitCommand,
        // Process exit joins deferred persistence before the disk assertion.
        Step::Custom(assert_no_checkpoint_journal),
    ],
    false,
)
.with_setup(setup_workspace_rewind_off);

fn assert_no_checkpoint_journal(harness: &mut PtyHarness) -> Result<()> {
    let workspace = harness
        .working_directory()
        .context("matrix workspace is unavailable")?;
    ensure!(
        fs::read(workspace.join(".rho-tui-fixture-output.txt"))? == b"deterministic tool output\n",
        "native write did not complete with workspace rewind disabled"
    );
    let sessions = harness
        .working_directory()
        .and_then(std::path::Path::parent)
        .context("matrix workspace has no isolated home parent")?
        .join("home/.rho/sessions");
    ensure!(
        sessions.is_dir(),
        "native fixture turn did not create isolated session storage"
    );
    let mut pending = vec![sessions];
    let mut saved_session = false;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let journal = entry.path().join("workspace-checkpoints/checkpoints.jsonl");
                ensure!(
                    !journal.try_exists()?,
                    "workspace rewind opt-out created a checkpoint journal: {}",
                    journal.display()
                );
                pending.push(entry.path());
            } else if entry.file_name() == "session.jsonl" {
                saved_session = true;
            }
        }
    }
    ensure!(
        saved_session,
        "native fixture turn did not save a session transcript"
    );
    Ok(())
}

const WORKSPACE_REWIND_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("capture_native_write"),
    Step::SubmitText("fixture tool"),
    // Final response text can arrive before the turn returns to idle.
    Step::Custom(|harness| {
        wait_for_turn_completion_after(harness, "tool lifecycle complete with one result")
    }),
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
        text: "workspace rewind audit; conversation restored to before the turn",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "delete  .rho-tui-fixture-output.txt  restored",
        timeout: SETTLE,
    },
    Step::WaitTextGone {
        text: "fixture tool",
        timeout: SETTLE,
    },
    Step::SubmitText(
        "!test ! -e .rho-tui-fixture-output.txt && echo rewind-delete-confirmed",
    ),
    Step::WaitText {
        text: "rewind-delete-confirmed",
        timeout: SETTLE,
    },
    Step::Phase("preserve_old_branch"),
    Step::SubmitText("/tree"),
    Step::WaitText {
        text: "fixture tool",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
