//! Covers misattributed messages between same-role tasks and lost expansion details.
//! Owner: interactive UX. Existing agent scenarios exercise launch/completion, not messaging.

use anyhow::{ensure, Result};

use super::{STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::{Key, MouseButton},
    pty::PtySize,
    scenario::{Scenario, Step},
    PtyHarness,
};

fn setup(home: &IsolatedHome) -> Result<()> {
    let mut config = std::fs::read_to_string(&home.config_path)?;
    // Explicitly exercise the user's preview setting rather than a second message-only cap.
    config.push_str("\n[display]\nmax_tool_output_lines = 2\n");
    std::fs::write(&home.config_path, config)?;
    Ok(())
}

fn assert_task_routing(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let messages = screen.split("↳ ").skip(1).collect::<Vec<_>>();
    ensure!(
        messages.len() == 2,
        "expected two task-first messages:\n{screen}"
    );
    for (card, task, excerpt) in [
        (
            messages[0],
            "Inspect message delivery cache",
            "Keep cache changes isolated from routing.",
        ),
        (
            messages[1],
            "Review message delivery routing",
            "Check the parent-to-child route",
        ),
    ] {
        ensure!(card.starts_with(task), "wrong message task:\n{card}");
        ensure!(
            card.contains("parent → worker · queued"),
            "wrong message route:\n{card}"
        );
        ensure!(card.contains(excerpt), "message body missing:\n{card}");
        ensure!(
            !card.contains("attach: rho attach"),
            "run details leaked into preview:\n{card}"
        );
    }
    ensure!(
        !screen.contains("Delivery detail beyond"),
        "preview ignored configured budget:\n{screen}"
    );
    Ok(())
}

fn expand_first_message(harness: &mut PtyHarness) -> Result<()> {
    let row = harness
        .screen()
        .rows_text()
        .iter()
        .position(|line| line.contains("↳ Inspect message delivery cache"))
        .ok_or_else(|| anyhow::anyhow!("first message missing"))? as u16
        + 1;
    harness.mouse(MouseButton::Left, 3, row, true)?;
    harness.mouse(MouseButton::Left, 3, row, false)?;
    harness.wait_for_text("attach: rho attach", STREAM)?;
    let screen = harness.screen().contents();
    let normalized = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(
        normalized
            .contains("task: Inspect message delivery cache invalidation across resumed sessions"),
        "expanded task title lost its narrow-width tail:\n{screen}"
    );
    Ok(())
}

const STEPS: &[Step] = &[
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture agent messages"),
    Step::WaitText {
        text: "message deliveries queued",
        timeout: STREAM,
    },
    Step::Custom(assert_task_routing),
    Step::Key(Key::Ctrl('o')),
    Step::WaitText {
        text: "Delivery detail beyond the collapsed preview.",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "attach: rho attach",
        timeout: STREAM,
    },
    Step::Key(Key::Ctrl('o')),
    Step::WaitTextGone {
        text: "Delivery detail beyond",
        timeout: STREAM,
    },
    Step::Resize { rows: 50, cols: 48 },
    Step::WaitText {
        text: "↳ Inspect message delivery cache",
        timeout: STREAM,
    },
    Step::Custom(expand_first_message),
    Step::ExitCommand,
];

pub(super) const AGENT_MESSAGES_SCENARIO: Scenario = Scenario::new(
    "agent_messages",
    "Keep same-role task messages distinct and expand the message and run details",
    PtySize {
        rows: 50,
        cols: 100,
    },
    STEPS,
    /*smoke*/ true,
)
.with_setup(setup);
