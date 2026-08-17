use crate::harness::WaitTimeout;
use crate::{keys::Key, scenario::Step, PtyHarness};
use anyhow::Result;

const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

fn assert_transcript_has(screen: &str, text: &str, missing: &str) -> Result<()> {
    if !screen.contains(text) {
        anyhow::bail!("{missing}:\n{screen}");
    }
    Ok(())
}

fn assert_transcript_lacks(screen: &str, text: &str, present: &str) -> Result<()> {
    if screen.contains(text) {
        anyhow::bail!("{present}:\n{screen}");
    }
    Ok(())
}

fn assert_restored_first_turn_transcript(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    assert_transcript_has(
        &screen,
        "fixture response: tree first",
        "restored transcript missing first turn",
    )?;
    assert_transcript_lacks(
        &screen,
        "tree second",
        "restored transcript still showed the later turn",
    )
}

fn assert_branch_kept_selected_parent(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    assert_transcript_has(
        &screen,
        "fixture response: tree first",
        "branched transcript missing restored parent",
    )?;
    assert_transcript_has(
        &screen,
        "fixture response: tree branch",
        "branched transcript missing new turn",
    )?;
    assert_transcript_lacks(
        &screen,
        "tree second",
        "branched transcript included the abandoned turn",
    )
}

fn assert_tree_list_only_popup(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("Conversation tree") {
        anyhow::bail!("tree popup missing title:\n{screen}");
    }
    if !screen.contains(" TREE") {
        anyhow::bail!("tree popup missing list header:\n{screen}");
    }
    if screen.contains(" DETAILS") {
        anyhow::bail!("tree popup still showed a details pane:\n{screen}");
    }
    // Side-by-side layout joins the column divider to the frame with `┬`;
    // a nav-only popup has none. Bare `│` checks no longer work because the
    // sized-to-content frame is surrounded by blank margin.
    if screen.contains('┬') {
        anyhow::bail!("tree popup still used a side-by-side separator:\n{screen}");
    }
    if !screen.contains("Enter restore") {
        anyhow::bail!("tree popup missing restore footer:\n{screen}");
    }
    if !screen.contains("PgUp/PgDn") {
        anyhow::bail!("tree popup missing page keys hint:\n{screen}");
    }
    Ok(())
}

pub(super) const CONVERSATION_TREE_STEPS: &[Step] = &[
    Step::Phase("create_linear_history"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("tree first"),
    Step::WaitText {
        text: "fixture response: tree first",
        timeout: STREAM,
    },
    Step::SubmitText("tree second"),
    Step::WaitText {
        text: "fixture response: tree second",
        timeout: STREAM,
    },
    Step::Phase("restore_first_turn"),
    Step::SubmitText("/tree"),
    Step::WaitText {
        text: "Conversation tree",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: " TREE",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "tree first",
        timeout: SETTLE,
    },
    Step::Custom(assert_tree_list_only_popup),
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "restored conversation state",
        timeout: STREAM,
    },
    Step::Custom(assert_restored_first_turn_transcript),
    Step::Phase("create_branch"),
    Step::SubmitText("tree branch"),
    Step::WaitText {
        text: "fixture response: tree branch",
        timeout: STREAM,
    },
    Step::Custom(assert_branch_kept_selected_parent),
    Step::SubmitText("/tree"),
    Step::WaitText {
        text: "tree second",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "tree branch",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
