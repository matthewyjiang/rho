use std::time::{Duration, Instant};

use anyhow::Result;

use crate::{keys::Key, scenario::Step, PtyHarness};

use super::{SETTLE, STARTUP};

pub(super) const OPEN_MODEL_PICKER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/model"),
    Step::WaitText {
        text: "select model",
        timeout: STARTUP,
    },
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

/// Phrase near the end of the goal-judge prompt body. On the default scenario
/// size it starts below the first detail viewport and becomes visible after
/// paging the detail pane.
const HIDDEN_DETAIL_MARKER: &str = "empty human_steps array";

fn assert_wide_popup_divider_is_stable(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let divider_columns = screen
        .lines()
        .filter_map(|line| {
            line.find(" │ ")
                .map(|divider| line[..divider].chars().count())
        })
        .collect::<Vec<_>>();
    if divider_columns.len() < 10 {
        anyhow::bail!("agents popup divider was missing from body rows:\n{screen}");
    }
    if !divider_columns
        .iter()
        .all(|column| *column == divider_columns[0])
    {
        anyhow::bail!("agents popup divider shifted between rows:\n{screen}");
    }
    Ok(())
}

fn assert_hidden_detail_marker_absent(harness: &mut PtyHarness) -> Result<()> {
    if harness.screen().contains_text(HIDDEN_DETAIL_MARKER) {
        anyhow::bail!("detail marker was already visible before scrolling");
    }
    Ok(())
}

fn scroll_detail_until_marker_visible(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        harness.poll(Duration::from_millis(30));
        if harness.screen().contains_text(HIDDEN_DETAIL_MARKER) {
            return Ok(());
        }
        harness.inject_key(&Key::PageDown)?;
        std::thread::sleep(Duration::from_millis(50));
    }
    harness.poll(Duration::from_millis(50));
    if harness.screen().contains_text(HIDDEN_DETAIL_MARKER) {
        return Ok(());
    }
    anyhow::bail!(
        "detail marker never became visible after PageDown scrolling\n{}",
        harness.screen().contents()
    )
}

fn assert_narrow_agents_popup(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("loaded agents") {
        anyhow::bail!("narrow agents popup missing title:\n{screen}");
    }
    if !screen.contains("goal-judge") {
        anyhow::bail!("narrow agents popup missing navigation list:\n{screen}");
    }
    if !screen.contains("Internal agent that evaluates") {
        anyhow::bail!("narrow agents popup missing stacked detail:\n{screen}");
    }
    if screen.contains(" │ ") {
        anyhow::bail!("narrow agents popup still used a side-by-side separator:\n{screen}");
    }
    Ok(())
}

pub(super) const OPEN_AGENTS_PICKER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/agents"),
    Step::WaitText {
        text: "goal-judge",
        timeout: SETTLE,
    },
    Step::AssertText("Internal agent that evaluates goal completion"),
    Step::AssertText("↑↓"),
    Step::AssertText("PgUp/PgDn"),
    Step::Custom(assert_wide_popup_divider_is_stable),
    Step::Custom(assert_hidden_detail_marker_absent),
    Step::Phase("scroll_detail"),
    Step::Custom(scroll_detail_until_marker_visible),
    Step::AssertText(HIDDEN_DETAIL_MARKER),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Use conversation model",
        timeout: SETTLE,
    },
    Step::AssertText("select model for goal-judge"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "goal-judge",
        timeout: SETTLE,
    },
    Step::Phase("narrow_layout"),
    Step::Resize { rows: 32, cols: 50 },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Internal agent that evaluates",
        timeout: SETTLE,
    },
    Step::Custom(assert_narrow_agents_popup),
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
