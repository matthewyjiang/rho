//! Background delegated agent scenarios.
//!
//! Spawning, automatic completion delivery, and a child questionnaire that
//! outlives its parent turn all exercise the same background rail, so they stay
//! together.

use std::time::Duration;

use anyhow::Result;

use crate::{
    harness::WaitTimeout,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

pub(super) const BACKGROUND_AGENT_AUTO_DELIVERY_SCENARIO: Scenario = Scenario::new(
    "background_agent_auto_delivery",
    "Spawn a background agent, end the turn, and receive its completion automatically",
    DEFAULT_SIZE,
    BACKGROUND_AGENT_AUTO_DELIVERY_STEPS,
    /*smoke*/ false,
);

pub(super) const BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO: Scenario = Scenario::new(
    "background_agent_questionnaire",
    "Answer a questionnaire raised by a background agent and deliver its completion",
    DEFAULT_SIZE,
    BACKGROUND_AGENT_QUESTIONNAIRE_STEPS,
    /*smoke*/ false,
);

pub(super) const LATE_BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO: Scenario = Scenario::new(
    "late_background_agent_questionnaire",
    "Answer a questionnaire a background agent raises after its parent's turn ended",
    DEFAULT_SIZE,
    LATE_BACKGROUND_AGENT_QUESTIONNAIRE_STEPS,
    /*smoke*/ false,
);

pub(super) const CANCEL_BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO: Scenario = Scenario::new(
    "cancel_background_agent_questionnaire",
    "Cancel a background agent's questionnaire while its parent's turn runs",
    DEFAULT_SIZE,
    CANCEL_BACKGROUND_AGENT_QUESTIONNAIRE_STEPS,
    /*smoke*/ false,
);

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

const BACKGROUND_AGENT_AUTO_DELIVERY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_agent"),
    Step::SubmitText("fixture background agent"),
    // Partial tool arguments can become a completed spawn card between frames.
    // Wait for the durable receipt instead of requiring that transient state.
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
    // The finished rail row leaves in the repaint that lands the result card,
    // so it is already gone once the parent replies. The short budget only
    // absorbs a partially read frame; a timed linger would outlast it.
    Step::WaitTextGone {
        text: "✓ done",
        timeout: WaitTimeout::millis(300, "finished rail row left with delivery"),
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

fn release_background_questionnaire_parent(harness: &mut crate::PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("background questionnaire agent dispatched: agent") {
        anyhow::bail!("parent turn ended before the child questionnaire appeared:\n{screen}");
    }
    // Keep the parent live until the form is visibly open, then let it finish
    // while the unanswered child is still waiting for input.
    super::release_fixture(harness, ".rho-fixture-release-questionnaire-parent")
}

const BACKGROUND_AGENT_QUESTIONNAIRE_STEPS: &[Step] = &[
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
    Step::Custom(release_background_questionnaire_parent),
    Step::Phase("parent_finishes_while_questionnaire_remains_open"),
    Step::WaitText {
        text: "background questionnaire agent dispatched: agent",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "(worker) started in background",
        timeout: STREAM,
    },
    // Check dispatch before answering. Completion can scroll the dispatch
    // receipt out of view as soon as the child receives the answer.
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "answered questionnaire for agent",
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

fn release_child_questionnaire(harness: &mut crate::PtyHarness) -> Result<()> {
    super::release_fixture(harness, ".rho-fixture-release-child-questionnaire")
}

// Covers: a subagent that asks after its parent's turn ended must still
// reach the person, and its answer must reach the subagent, without a
// parent turn in between.
// Owner: interactive delegated-questionnaire routing. The scenario above
// holds the parent's turn open until the form shows.
const LATE_BACKGROUND_AGENT_QUESTIONNAIRE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("parent_turn_ends_before_the_child_asks"),
    Step::SubmitText("fixture late background questionnaire"),
    Step::WaitText {
        text: "background questionnaire agent dispatched: agent",
        timeout: STREAM,
    },
    // The completion footer proves the parent turn ended before the release.
    Step::WaitText {
        text: "Worked for",
        timeout: STREAM,
    },
    Step::Phase("answer_child_questionnaire_while_idle"),
    Step::Custom(release_child_questionnaire),
    Step::WaitText {
        text: "asks: Background questionnaire",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Choose one color",
        timeout: STREAM,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "answered questionnaire for agent",
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

fn release_refused_child(harness: &mut crate::PtyHarness) -> Result<()> {
    super::release_fixture(harness, ".rho-fixture-release-refused-child")
}

/// Fails once anything said the parent's turn was cancelled. Reads the raw
/// output: the status can be repainted or scrolled off the screen since.
fn assert_parent_turn_not_cancelled(harness: &mut crate::PtyHarness) -> Result<()> {
    let output = String::from_utf8_lossy(harness.raw_output());
    for said in ["model interrupted", "questionnaire cancelled"] {
        if output.contains(said) {
            anyhow::bail!("cancelling the child's questionnaire ended the parent turn: {said}");
        }
    }
    Ok(())
}

// Covers: Esc at a subagent's questionnaire while its parent's turn runs
// dismisses only the questionnaire; the parent's turn runs on to its end.
// Owner: interactive delegated-questionnaire routing. Holds the parent's
// turn open until the form is cancelled, as the answering scenario does.
const CANCEL_BACKGROUND_AGENT_QUESTIONNAIRE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_questionnaire_agent"),
    Step::SubmitText("fixture background questionnaire"),
    Step::WaitText {
        text: "asks: Background questionnaire",
        timeout: STREAM,
    },
    // The modal controls show once the form takes keys, so Esc reaches it
    // rather than the running turn.
    Step::WaitText {
        text: "Esc cancel",
        timeout: STREAM,
    },
    Step::Phase("cancel_child_questionnaire"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "cancelled questionnaire for agent",
        timeout: STREAM,
    },
    // The composer offers to abort only while a turn runs.
    Step::WaitText {
        text: "esc abort",
        timeout: STREAM,
    },
    Step::Custom(assert_parent_turn_not_cancelled),
    Step::Phase("parent_turn_runs_to_its_end"),
    Step::Custom(release_background_questionnaire_parent),
    // Only the held parent request's own reply says this; a cancelled
    // parent never gives it. The fixture holds the child's result meanwhile.
    Step::WaitText {
        text: "background questionnaire agent dispatched: agent",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Worked for",
        timeout: STREAM,
    },
    Step::Custom(assert_parent_turn_not_cancelled),
    Step::Phase("child_result_follows"),
    Step::Custom(release_refused_child),
    // The fixture's own line for a child whose question went unanswered,
    // whichever way the refusal reached it.
    Step::WaitText {
        text: "background agent questionnaire received wrong answer",
        timeout: STREAM,
    },
    Step::ExitCommand,
];
