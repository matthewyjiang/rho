use std::time::Duration;

use anyhow::{Context, Result};

use crate::{
    env::IsolatedHome,
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const RESPONSE: WaitTimeout = WaitTimeout::secs(20, "document response");

fn setup_document(home: &IsolatedHome) -> Result<()> {
    let path = home.workspace.join("absolute-path-report.txt");
    std::fs::write(&path, "document body from path")?;
    let path = home.workspace.join("must-not-leak.txt");
    std::fs::write(&path, "private follow-up attachment")?;
    Ok(())
}

fn paste_absolute_document_path(harness: &mut PtyHarness) -> Result<()> {
    let path = harness
        .working_directory()
        .context("document scenario has no working directory")?
        .join("absolute-path-report.txt");
    harness.paste(&path.to_string_lossy())
}

fn paste_attachment_that_goal_clear_must_discard(harness: &mut PtyHarness) -> Result<()> {
    let path = harness
        .working_directory()
        .context("document scenario has no working directory")?
        .join("must-not-leak.txt");
    harness.paste(&path.to_string_lossy())
}

fn assert_goal_clear_discarded_attachment(harness: &mut PtyHarness) -> Result<()> {
    let label = "[txt: must-not-leak.txt · 28 chars]";
    let composer_rows = harness
        .screen()
        .rows_text()
        .into_iter()
        .rev()
        .take(6)
        .collect::<Vec<_>>();
    if composer_rows.iter().any(|row| row.contains(label)) {
        anyhow::bail!(
            "/goal clear left queued media in the composer:\n{}",
            harness.screen().contents()
        );
    }
    Ok(())
}

const DOCUMENT_ATTACHMENT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("paste_absolute_document"),
    Step::Custom(paste_absolute_document_path),
    Step::WaitText {
        text: "[txt: absolute-path-report.txt · 23 chars]",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "fixture response: Attached file: absolute-path-report.txt (text/plain)",
        timeout: RESPONSE,
    },
    Step::WaitText {
        text: "document body from path",
        timeout: RESPONSE,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::Phase("goal_clear_discards_attachment"),
    Step::Custom(paste_attachment_that_goal_clear_must_discard),
    Step::WaitText {
        text: "[txt: must-not-leak.txt · 28 chars]",
        timeout: SETTLE,
    },
    Step::SubmitText("/goal clear"),
    Step::WaitText {
        text: "no active goal",
        timeout: SETTLE,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(100),
        timeout: SETTLE,
    },
    Step::Custom(assert_goal_clear_discarded_attachment),
    Step::ExitCommand,
];

pub(super) const DOCUMENT_ATTACHMENT_SCENARIO: Scenario = Scenario::new(
    "document_attachment",
    "Paste an absolute document path and submit its extracted text",
    PtySize {
        rows: 28,
        cols: 100,
    },
    DOCUMENT_ATTACHMENT_STEPS,
    false,
)
.with_setup(setup_document);
