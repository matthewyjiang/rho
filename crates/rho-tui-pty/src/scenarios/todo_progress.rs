//! Covers: the checklist overlay must stay live during nested tool execution,
//! focus the current item on a small screen, and not leak across /new.
//! Owner: interactive UX; the card scenario only covers historical tool output.

use crate::{
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP, STREAM};

pub(super) const TODO_PROGRESS_SCENARIO: Scenario = Scenario::new(
    "todo_progress",
    "Inspect live task progress without interrupting a turn or losing session boundaries",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/todo"),
        Step::WaitText {
            text: "No tasks in this session",
            timeout: STREAM,
        },
        Step::Key(Key::Esc),
        Step::WaitTextGone {
            text: "Task progress",
            timeout: STREAM,
        },
        Step::SubmitText("fixture todo"),
        Step::WaitText {
            text: "todo checklist complete",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "Worked for",
            timeout: STREAM,
        },
        Step::SubmitText("fixture todo update"),
        Step::WaitText {
            text: "waiting to update checklist",
            timeout: STREAM,
        },
        Step::SubmitText("/todo"),
        Step::WaitText {
            text: "Task progress",
            timeout: STREAM,
        },
        Step::AssertText("▸ implement checklist"),
        Step::Custom(|harness| super::release_fixture(harness, ".rho-fixture-release-todo-update")),
        Step::WaitText {
            text: "▸ verify task retention",
            timeout: STREAM,
        },
        Step::AssertText("preserve exact task details"),
        Step::Custom(assert_multiline_task),
        Step::AssertText("✓ finished step 1"),
        Step::Key(Key::Esc),
        Step::WaitTextGone {
            text: "Task progress",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "todo update complete",
            timeout: STREAM,
        },
        Step::Resize { rows: 14, cols: 52 },
        Step::SubmitText("/todo"),
        Step::WaitText {
            text: "Task progress",
            timeout: STREAM,
        },
        Step::AssertText("▸ verify task retention"),
        Step::Key(Key::Home),
        Step::WaitText {
            text: "✓ finished step 1",
            timeout: STREAM,
        },
        Step::Key(Key::End),
        Step::WaitText {
            text: "▸ verify task retention",
            timeout: STREAM,
        },
        Step::Key(Key::Esc),
        Step::WaitTextGone {
            text: "Task progress",
            timeout: STREAM,
        },
        Step::SubmitText("/new"),
        Step::WaitText {
            text: "new session",
            timeout: STREAM,
        },
        Step::SubmitText("/todo"),
        Step::WaitText {
            text: "No tasks in this session",
            timeout: STREAM,
        },
        Step::Key(Key::Esc),
        Step::WaitTextGone {
            text: "Task progress",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

fn assert_multiline_task(harness: &mut crate::harness::PtyHarness) -> anyhow::Result<()> {
    let screen = harness.screen().contents();
    let rows = screen.lines().collect::<Vec<_>>();
    let start = rows
        .iter()
        .position(|row| row.contains("▸ verify task retention"))
        .ok_or_else(|| anyhow::anyhow!("current task missing: {screen}"))?;
    anyhow::ensure!(
        rows.get(start + 1)
            .is_some_and(|row| row.contains("preserve exact task details"))
            && rows
                .get(start + 3)
                .is_some_and(|row| row.contains("finish review")),
        "multiline task lost hard line boundaries: {screen}"
    );
    Ok(())
}
