use std::time::Duration;

use anyhow::Result;

use crate::{keys::Key, scenario::Step};

use super::{SETTLE, STARTUP, STREAM};

fn assert_agent_tool_hides_raw_json(harness: &mut crate::PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("\"agent_id\"")
        || screen.contains("\"background\":true")
        || screen.contains("\"action\":\"list\"")
    {
        anyhow::bail!("agent tool exposed raw JSON:\n{screen}");
    }
    Ok(())
}

pub(super) const AUTO_DELIVERY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_agent"),
    Step::SubmitText("fixture background agent"),
    Step::WaitText {
        text: "● wor  starting",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "● worker  running in background",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "fixture stream",
        timeout: STREAM,
    },
    Step::Custom(assert_agent_tool_hides_raw_json),
    // The fixture echoes the spawn receipt's first line, proving the tool
    // resolved with a start line and the parent turn ended.
    Step::WaitText {
        text: "background agent dispatched: agent",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "(worker) started in background",
        timeout: STREAM,
    },
    Step::Phase("automatic_completion_delivery"),
    // The fixture validates the notification's real payload (agent identity,
    // terminal state, delegated result) and counts notification turns, so
    // this asserts a well-formed, exactly-once delivery.
    Step::WaitText {
        text: "background agent completion received with delegated result (delivery 1)",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::Phase("list_agents"),
    Step::SubmitText("fixture agents list"),
    Step::WaitText {
        text: "delegated agents",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "worker  completed",
        timeout: STREAM,
    },
    Step::Custom(assert_agent_tool_hides_raw_json),
    Step::ExitCommand,
];

fn assert_user_turn_precedes_draft_safe_notification(
    harness: &mut crate::PtyHarness,
) -> Result<()> {
    let raw = String::from_utf8_lossy(harness.raw_output());
    let user_turn = raw
        .find("fixture response: user turn wins")
        .ok_or_else(|| anyhow::anyhow!("user turn response was not rendered"))?;
    let notification = raw
        .find("draft-safe background completion received")
        .ok_or_else(|| anyhow::anyhow!("background completion was not rendered"))?;
    if user_turn >= notification {
        anyhow::bail!("background completion ran before the pending user turn");
    }
    Ok(())
}

pub(super) const DRAFT_PRIORITY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_slow_background_agent"),
    Step::SubmitText("fixture background agent draft race"),
    Step::WaitText {
        text: "background agent dispatched: agent",
        timeout: STREAM,
    },
    Step::Phase("hold_user_draft_until_agent_finishes"),
    Step::TypeText("user turn wins"),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(800),
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "fixture response: user turn wins",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "draft-safe background completion received (delivery 1)",
        timeout: STREAM,
    },
    Step::Custom(assert_user_turn_precedes_draft_safe_notification),
    Step::ExitCommand,
];
