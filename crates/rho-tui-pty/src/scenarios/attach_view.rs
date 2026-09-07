//! In-place attach view from `/attach` and the activity rail.

use super::assert_helpers::assert_terminal_restored;
use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    harness::WaitTimeout,
    keys::Key,
    scenario::{Scenario, Step},
    PtyHarness,
};

fn cycle_to_the_other_running_subagent(harness: &mut PtyHarness) -> anyhow::Result<()> {
    let screen = harness.screen().contents();
    let on_explorer = screen.contains(" · explorer");
    let on_worker = screen.contains(" · worker");
    if on_explorer == on_worker {
        anyhow::bail!("attach view should show exactly one of worker or explorer:\n{screen}");
    }
    harness.inject_key(&Key::Tab)?;
    let next = if on_worker {
        " · explorer"
    } else {
        " · worker"
    };
    harness.wait_for_text(next, WaitTimeout::secs(10, "cycle to the other subagent"))?;
    Ok(())
}

fn setup_supervised(home: &IsolatedHome) -> anyhow::Result<()> {
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search_provider = "disabled"
permission_mode = "supervised"

[behavior]
credential_store = "file"
"#,
    )?;
    Ok(())
}

// Covers: real agents(message) delivery disappears from the child's attach view.
// Owner: interactive incoming-message presentation and replay.
fn parent_message_card(harness: &mut PtyHarness) -> anyhow::Result<()> {
    harness.submit_text("fixture parent message attach")?;
    harness.wait_for_text("parent message queued", STREAM)?;
    harness.submit_text("/attach")?;
    harness.wait_for_text("attach subagent", STREAM)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("q back", STREAM)?;
    harness.wait_for_text("Inspect incoming parent messages in attach", STREAM)?;
    anyhow::ensure!(
        !harness
            .screen()
            .contents()
            .contains("Keep the correction visible."),
        "a queued correction must not appear as applied input"
    );
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text_gone("q back", STREAM)?;
    harness.submit_text("fixture apply parent message")?;
    harness.wait_for_text("parent correction released", STREAM)?;
    for _ in 0..2 {
        harness.submit_text("/attach")?;
        harness.wait_for_text("attach subagent", STREAM)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("q back", STREAM)?;
        harness.wait_for_text("Keep the correction visible.", STREAM)?;
        let rows = harness.screen().rows_text();
        let bodies = rows
            .iter()
            .filter(|row| row.contains("Keep the correction visible."))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            bodies.len() == 1 && bodies[0].trim_start().starts_with('│'),
            "parent input must appear once as a card, not a prompt or notice:\n{}",
            harness.screen().contents()
        );
        harness.inject_key(&Key::Ctrl('o'))?;
        harness.wait_for_text("applied to conversation history", STREAM)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text_gone("q back", STREAM)?;
    }
    Ok(())
}

pub(super) const ATTACH_PARENT_MESSAGE_SCENARIO: Scenario = Scenario::new(
    "attach_parent_message",
    "Show applied parent messages as incoming cards and preserve them on reattach",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(parent_message_card),
        Step::ExitCommand,
    ],
    /* smoke */ true,
);

const ATTACH_VIEW_FROM_COMMAND_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_subagent"),
    Step::SubmitText("fixture subagent rail"),
    Step::WaitText {
        text: "subagent rail fixture dispatched",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "worker",
        timeout: SETTLE,
    },
    Step::Phase("enter_attach_view"),
    Step::SubmitText("/attach"),
    Step::WaitText {
        text: "attach subagent",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "q back",
        timeout: SETTLE,
    },
    Step::AssertText("attach "),
    Step::Phase("keys_do_not_reach_composer"),
    Step::TypeText("zzzattachleak"),
    Step::AssertText("q back"),
    Step::Phase("return_to_composer"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "q back",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Type a message",
        timeout: SETTLE,
    },
    Step::WaitTextGone {
        text: "zzzattachleak",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const ATTACH_VIEW_CYCLE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_two_subagents"),
    Step::SubmitText("fixture two subagents"),
    Step::WaitText {
        text: "two subagents dispatched",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "worker",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "explorer",
        timeout: SETTLE,
    },
    Step::Phase("attach_and_cycle"),
    Step::SubmitText("/attach"),
    Step::WaitText {
        text: "attach subagent",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "q back",
        timeout: SETTLE,
    },
    Step::Custom(cycle_to_the_other_running_subagent),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "q back",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const ATTACH_VIEW_PARENT_APPROVAL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "Supervised",
        timeout: STARTUP,
    },
    Step::Phase("spawn_then_attach"),
    Step::SubmitText("fixture attach then approval"),
    // The agent tool card prints "worker" before the subagent rail ingests
    // the run, and the attach picker lists only rail candidates. The parent
    // turn stays busy here (no completion text to wait on), so wait for the
    // rail row's title instead of the card.
    Step::WaitText {
        text: "Fixture run title",
        timeout: STREAM,
    },
    Step::SubmitText("/attach"),
    Step::WaitText {
        text: "attach subagent",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "q back",
        timeout: SETTLE,
    },
    Step::Phase("parent_approval_badges_without_yanking"),
    Step::WaitText {
        text: "parent approval waiting",
        timeout: STREAM,
    },
    Step::AssertText("q back"),
    Step::Phase("approval_keys_stay_in_attach"),
    Step::Key(Key::Char('y')),
    Step::AssertText("q back"),
    Step::AssertText("parent approval waiting"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "bash wants to run a command",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const ATTACH_VIEW_QUIT_RESTORES_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture subagent rail"),
    Step::WaitText {
        text: "subagent rail fixture dispatched",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "worker",
        timeout: SETTLE,
    },
    Step::SubmitText("/attach"),
    Step::WaitText {
        text: "attach subagent",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "q back",
        timeout: SETTLE,
    },
    Step::Phase("leave_then_quit_from_attach_view"),
    Step::Key(Key::Ctrl('c')),
    Step::WaitTextGone {
        text: "q back",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "left attach view; press ctrl-c again to quit",
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::WaitExit { timeout: SETTLE },
    Step::Phase("assert_restore"),
    Step::Custom(assert_terminal_restored),
];

pub(super) const ATTACH_VIEW_FROM_COMMAND_SCENARIO: Scenario = Scenario::new(
    "attach_view_from_command",
    "Enter the in-place attach view from /attach and return to the composer",
    DEFAULT_SIZE,
    ATTACH_VIEW_FROM_COMMAND_STEPS,
    /*smoke*/ false,
);

pub(super) const ATTACH_VIEW_CYCLE_SCENARIO: Scenario = Scenario::new(
    "attach_view_cycle",
    "Cycle between two running subagents inside the attach view",
    DEFAULT_SIZE,
    ATTACH_VIEW_CYCLE_STEPS,
    /*smoke*/ false,
);

pub(super) const ATTACH_VIEW_PARENT_APPROVAL_SCENARIO: Scenario = Scenario::new(
    "attach_view_parent_approval",
    "Badge a parent approval in the attach view without yanking the user back",
    DEFAULT_SIZE,
    ATTACH_VIEW_PARENT_APPROVAL_STEPS,
    /*smoke*/ false,
)
.with_setup(setup_supervised);

pub(super) const ATTACH_VIEW_QUIT_RESTORES_SCENARIO: Scenario = Scenario::new(
    "attach_view_quit_restores",
    "Leave the attach view on Ctrl-C, then quit and restore the terminal",
    DEFAULT_SIZE,
    ATTACH_VIEW_QUIT_RESTORES_STEPS,
    /*smoke*/ false,
);
