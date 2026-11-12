//! Session persistence boundary for interactive conversations that must not reach disk.

use anyhow::{ensure, Context, Result};

use crate::{
    harness::PtyHarness,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{assert_helpers::wait_for_turn_completion_after, SETTLE, STREAM};

// Covers: startup prompts, later turns, and /new must not re-enable persistence.
// Owner: interactive lifecycle and its on-disk effects.
pub(super) const NO_SAVE_SESSION_SCENARIO: Scenario = Scenario::new(
    "no_save_session",
    "Keep startup and subsequent prompts unsaved across /new",
    SIZE,
    STEPS,
    /*smoke*/ true,
)
.with_args(ARGS);

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};
const ARGS: &[&str] = &["--no-save", "--prompt", "private startup"];
const STEPS: &[Step] = &[
    Step::Phase("startup_prompt"),
    Step::WaitText {
        text: "fixture response: private startup",
        timeout: STREAM,
    },
    Step::AssertText("not saved"),
    // /resume and /new are idle-only; during a turn they only set a status.
    // The response text can paint before the turn returns to idle.
    Step::Custom(wait_for_startup_turn),
    Step::Phase("resume_blocked"),
    Step::SubmitText("/resume"),
    Step::WaitText {
        text: "resume unavailable with --no-save",
        timeout: SETTLE,
    },
    Step::Phase("second_turn"),
    Step::SubmitText("private follow-up"),
    Step::WaitText {
        text: "fixture response: private follow-up",
        timeout: STREAM,
    },
    Step::AssertText("not saved"),
    Step::Custom(assert_no_conversation_files),
    Step::Custom(wait_for_follow_up_turn),
    Step::Phase("new_conversation"),
    Step::SubmitText("/new"),
    Step::WaitTextGone {
        text: "fixture response: private follow-up",
        timeout: SETTLE,
    },
    Step::AssertText("not saved"),
    Step::SubmitText("private after new"),
    Step::WaitText {
        text: "fixture response: private after new",
        timeout: STREAM,
    },
    Step::AssertText("not saved"),
    Step::Custom(assert_no_conversation_files),
    Step::ExitCommand,
    // Check after process exit as well, catching deferred and shutdown writes.
    Step::Custom(assert_no_conversation_files),
];

fn wait_for_startup_turn(harness: &mut PtyHarness) -> Result<()> {
    wait_for_turn_completion_after(harness, "fixture response: private startup")
}

fn wait_for_follow_up_turn(harness: &mut PtyHarness) -> Result<()> {
    wait_for_turn_completion_after(harness, "fixture response: private follow-up")
}

fn assert_no_conversation_files(harness: &mut PtyHarness) -> Result<()> {
    let root = harness
        .working_directory()
        .and_then(std::path::Path::parent)
        .context("matrix workspace has no isolated home parent")?
        .join("home/.rho");
    let sessions = root.join("sessions");
    if sessions.try_exists()? {
        let mut pending = vec![sessions];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                ensure!(
                    entry.file_type()?.is_dir(),
                    "--no-save wrote session state: {}",
                    entry.path().display()
                );
                pending.push(entry.path());
            }
        }
    }
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        ensure!(
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with("prompt-history.sqlite3"),
            "--no-save created prompt history: {}",
            entry.path().display()
        );
    }
    // Wherever a file is, it holds none of the conversation: every prompt
    // and answer of the scenario says `private`.
    let mut pending = vec![root];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry.file_type()?.is_file() {
                let held = std::fs::read(entry.path())?;
                ensure!(
                    !held
                        .windows(CONVERSATION.len())
                        .any(|bytes| bytes == CONVERSATION),
                    "--no-save wrote the conversation to {}",
                    entry.path().display()
                );
            }
        }
    }
    Ok(())
}

/// What every prompt and answer of the scenario holds.
const CONVERSATION: &[u8] = b"private ";
