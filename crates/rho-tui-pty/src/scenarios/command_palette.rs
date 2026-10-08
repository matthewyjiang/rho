//! Slash command palette scenarios.

use std::time::Duration;

use anyhow::{Context, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

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

// Covers: a template file written after startup expands on submit without a
// restart, with positional arguments (one double-quoted) in its placeholders.
// Owner: interactive TUI
const PROMPT_TEMPLATE_HOT_RELOAD_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("write_template_after_startup"),
    Step::Custom(write_review_template),
    Step::SubmitText("/prompt:review alpha \"beta gamma\""),
    Step::WaitText {
        text: "fixture response: Review beta gamma then alpha.",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const PROMPT_TEMPLATE_HOT_RELOAD_SCENARIO: Scenario = Scenario::new(
    "prompt_template_hot_reload",
    "Expand a prompt template added after startup with positional arguments",
    SIZE,
    PROMPT_TEMPLATE_HOT_RELOAD_STEPS,
    /* smoke */ false,
);

fn write_review_template(harness: &mut PtyHarness) -> Result<()> {
    let prompts = harness
        .working_directory()
        .and_then(std::path::Path::parent)
        .context("matrix workspace has no isolated home parent")?
        .join("home/.rho/prompts");
    std::fs::create_dir_all(&prompts)?;
    std::fs::write(prompts.join("review.md"), "Review $2 then $1.\n")?;
    Ok(())
}

// Covers: Tab defers a placeholder template, then Enter with arguments resolves
// it as steering rather than rejecting the command while a turn is running.
// Owner: interactive TUI
const PROMPT_TEMPLATE_DURING_TURN_STEPS: &[Step] = &[
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Custom(write_review_template),
    Step::SubmitText("fixture gated reply"),
    Step::WaitText {
        text: "reply waiting for release",
        timeout: STREAM,
    },
    Step::Phase("complete_template_during_turn"),
    Step::TypeText("/prompt:rev"),
    Step::WaitText {
        text: "/prompt:review",
        timeout: SETTLE,
    },
    Step::Key(Key::Tab),
    Step::SubmitText("alpha \"beta gamma\""),
    Step::WaitText {
        text: "Review beta gamma then alpha.",
        timeout: STREAM,
    },
    Step::Custom(|harness| {
        super::fixture_release::release_fixture(harness, ".rho-fixture-release-reply")
    }),
    Step::WaitText {
        text: "fixture response: Review beta gamma then alpha.",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const PROMPT_TEMPLATE_DURING_TURN_SCENARIO: Scenario = Scenario::new(
    "prompt_template_during_turn",
    "Complete a placeholder template and submit its arguments as steering",
    SIZE,
    PROMPT_TEMPLATE_DURING_TURN_STEPS,
    /* smoke */ false,
);

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
