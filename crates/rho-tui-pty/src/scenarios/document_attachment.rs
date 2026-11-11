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
    let path = home.workspace.join("dropped path report.txt");
    std::fs::write(&path, "document body from path")?;
    let path = home.workspace.join("must-not-leak.txt");
    std::fs::write(&path, "private follow-up attachment")?;
    Ok(())
}

fn drop_document_path(harness: &mut PtyHarness) -> Result<()> {
    let path = harness
        .working_directory()
        .context("document scenario has no working directory")?
        .join("dropped path report.txt");
    let shell_escaped_path = path.to_string_lossy().replace(' ', "\\ ");
    harness.paste(&shell_escaped_path)
}

const DOCUMENT_ATTACHMENT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("drop_document"),
    Step::Custom(drop_document_path),
    Step::WaitText {
        text: "[txt: dropped path report.txt · 23 chars]",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "fixture response: Attached file: dropped path report.txt (text/plain)",
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
    Step::ExitCommand,
];

pub(super) const DOCUMENT_ATTACHMENT_SCENARIO: Scenario = Scenario::new(
    "document_attachment",
    "Drop a shell-escaped document path and submit its extracted text",
    PtySize {
        rows: 28,
        cols: 100,
    },
    DOCUMENT_ATTACHMENT_STEPS,
    false,
)
.with_setup(setup_document);
