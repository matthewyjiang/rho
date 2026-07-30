//! Background delegated agent scenarios.
//!
//! Spawning, automatic completion delivery, and a child questionnaire that
//! outlives its parent turn all exercise the same background rail, so they stay
//! together.

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

pub(super) const BACKGROUND_AGENT_AUTO_DELIVERY_STEPS: &[Step] = &[
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

fn assert_background_questionnaire_parent_active(harness: &mut crate::PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("background questionnaire agent dispatched: agent") {
        anyhow::bail!("parent turn ended before the child questionnaire appeared:\n{screen}");
    }
    Ok(())
}

pub(super) const BACKGROUND_AGENT_QUESTIONNAIRE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_questionnaire_agent"),
    Step::SubmitText("fixture background questionnaire"),
    Step::Phase("answer_child_questionnaire"),
    Step::WaitText {
        text: "asks: Background questionnaire",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Choose one color",
        timeout: STREAM,
    },
    Step::Custom(assert_background_questionnaire_parent_active),
    Step::Phase("parent_finishes_while_questionnaire_remains_open"),
    Step::WaitText {
        text: "background questionnaire agent dispatched: agent",
        timeout: STREAM,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "answered questionnaire for agent",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "background questionnaire agent dispatched: agent",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "(worker) started in background",
        timeout: STREAM,
    },
    Step::Phase("automatic_completion_delivery"),
    Step::WaitText {
        text: "background agent questionnaire completion received (delivery 1)",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];
