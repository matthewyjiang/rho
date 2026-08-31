//! Slash command palette and /help overlay scenarios.

use std::time::Duration;

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

// Covers: /help opens the shortcuts overlay and Esc returns to the session.
// Owner: interactive TUI
const HELP_OVERLAY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_help"),
    Step::SubmitText("/help"),
    Step::WaitText {
        text: "Keyboard shortcuts",
        timeout: SETTLE,
    },
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Custom(assert_help_overlay_dismissed),
    Step::ExitCommand,
];

// Covers: /agents create must load the guided creator instead of opening the agents catalog.
// Owner: interactive TUI
const CREATE_AGENT_COMMAND_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("create_agent"),
    Step::SubmitText("/agents create a read-only reviewer"),
    Step::WaitText {
        text: "skill(rho-agent-creator)",
        timeout: STARTUP,
    },
    Step::ExitCommand,
];

pub(super) fn setup_read_only_agent(home: &IsolatedHome) -> Result<()> {
    let agents = home.home.join(".rho/agents");
    std::fs::create_dir_all(&agents)?;
    std::fs::write(
        agents.join("read-only-fixture.md"),
        "---\nid: read-only-fixture\ndescription: fixture agent with read-only tools\ntools: [read_file]\n---\nRead files only.\n",
    )?;
    Ok(())
}

// Covers: the creator must fail before starting when the active agent cannot
// provide the tools required by the guided workflow.
// Owner: interactive TUI
const CREATE_AGENT_MISSING_TOOLS_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/create-agent"),
    Step::WaitText {
        text: "active agent is missing required tools",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

// Covers: typing / opens the command palette; filtering narrows matches.
// Owner: interactive TUI
const SLASH_COMMAND_PALETTE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_palette"),
    Step::TypeText("/"),
    // The palette shows a short top slice in name order; /advisor is first.
    Step::WaitText {
        text: "/advisor",
        timeout: SETTLE,
    },
    Step::Phase("filter"),
    Step::TypeText("mod"),
    Step::WaitText {
        text: "/model",
        timeout: SETTLE,
    },
    Step::Custom(assert_slash_palette_filtered_to_model),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];

// Covers: tab-completing /agents leaves the argument palette open, and a
// plain Enter must run the bare command instead of the first argument row.
// Owner: interactive TUI
const TAB_COMPLETE_ENTER_BARE_COMMAND_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("tab_complete"),
    Step::TypeText("/agents"),
    // The palette offers the command and its `/agents create` argument row.
    Step::WaitText {
        text: "/agents create",
        timeout: SETTLE,
    },
    Step::Key(Key::Tab),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Phase("enter_runs_bare_command"),
    Step::Key(Key::Enter),
    // The agents catalog opens only for the bare command; `/agents create`
    // would start the guided creator turn instead.
    Step::WaitText {
        text: "goal-judge",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "goal-judge",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const CREATE_AGENT_COMMAND_SCENARIO: Scenario = Scenario::new(
    "create_agent_command",
    "Start the guided agent creator without opening the agents catalog",
    SIZE,
    CREATE_AGENT_COMMAND_STEPS,
    /* smoke */ false,
);

pub(super) const CREATE_AGENT_MISSING_TOOLS_SCENARIO: Scenario = Scenario::new(
    "create_agent_missing_tools",
    "Name the tools a focused active agent needs before creation can start",
    SIZE,
    CREATE_AGENT_MISSING_TOOLS_STEPS,
    /* smoke */ false,
)
.with_setup(setup_read_only_agent)
.with_args(&["--agent", "read-only-fixture"]);

pub(super) const HELP_OVERLAY_SCENARIO: Scenario = Scenario::new(
    "help_overlay",
    "Open the keyboard shortcuts overlay and dismiss it cleanly",
    SIZE,
    HELP_OVERLAY_STEPS,
    /* smoke */ false,
);

pub(super) const SLASH_COMMAND_PALETTE_SCENARIO: Scenario = Scenario::new(
    "slash_command_palette",
    "Open the slash command palette and filter to a matching command",
    SIZE,
    SLASH_COMMAND_PALETTE_STEPS,
    /* smoke */ false,
);

pub(super) const TAB_COMPLETE_ENTER_BARE_COMMAND_SCENARIO: Scenario = Scenario::new(
    "tab_complete_enter_bare_command",
    "Tab completion leaves Enter running the bare slash command",
    SIZE,
    TAB_COMPLETE_ENTER_BARE_COMMAND_STEPS,
    /* smoke */ false,
);

fn assert_help_overlay_dismissed(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("Keyboard shortcuts") {
        anyhow::bail!("help overlay still visible after Esc:\n{screen}");
    }
    if !screen.contains("gpt-5.5") {
        anyhow::bail!("session chrome missing after dismissing help:\n{screen}");
    }
    Ok(())
}

fn assert_slash_palette_filtered_to_model(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("/model") {
        anyhow::bail!("filtered slash palette missing /model:\n{screen}");
    }
    // /advisor is first in the unfiltered short list; it must leave after /mod.
    if screen.contains("/advisor") {
        anyhow::bail!("slash palette still listed /advisor after /mod filter:\n{screen}");
    }
    Ok(())
}
