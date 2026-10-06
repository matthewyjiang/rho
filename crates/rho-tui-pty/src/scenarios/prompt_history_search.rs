//! Ctrl+R searches prompt history and recalls the chosen prompt into the
//! composer, where Up/Down history navigation continues from it.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::{
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{assert_helpers::wait_for_turn_completion_after, DEFAULT_SIZE, SETTLE, STARTUP};

// Covers: Ctrl+R opens history search instead of resetting the session, the
// filter finds an older prompt, Enter recalls it into the composer, and Down
// steps forward through history back to the saved draft.
// Owner: interactive TUI composer and picker wiring.
pub(super) const PROMPT_HISTORY_SEARCH_SCENARIO: Scenario = Scenario::new(
    "prompt_history_search",
    "Search prompt history with Ctrl+R and recall an older prompt",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("seed_history"),
        Step::Custom(submit_history_prompts),
        Step::Phase("search_and_recall"),
        Step::TypeText("half typed draft"),
        Step::Key(Key::Ctrl('r')),
        Step::WaitText {
            text: "Prompt history",
            timeout: SETTLE,
        },
        Step::TypeText("alpha"),
        Step::Key(Key::Enter),
        Step::WaitTextGone {
            text: "Prompt history",
            timeout: SETTLE,
        },
        Step::Custom(composer_shows_alpha),
        Step::Phase("history_navigation_continues"),
        Step::Key(Key::Down),
        Step::Custom(composer_shows_beta),
        Step::Key(Key::Down),
        Step::Custom(composer_shows_draft),
        // Clear the restored draft so /exit is not appended to it.
        Step::Key(Key::Ctrl('c')),
        Step::Custom(composer_is_empty),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
);

fn submit_history_prompts(harness: &mut PtyHarness) -> Result<()> {
    for prompt in ["history alpha", "history beta"] {
        harness.submit_text(prompt)?;
        // Submitting before the turn ends would queue the next prompt
        // instead of sending it.
        wait_for_turn_completion_after(harness, &format!("fixture response: {prompt}"))?;
    }
    Ok(())
}

fn composer_shows_alpha(harness: &mut PtyHarness) -> Result<()> {
    wait_for_composer(harness, "history alpha")
}

fn composer_shows_beta(harness: &mut PtyHarness) -> Result<()> {
    wait_for_composer(harness, "history beta")
}

fn composer_shows_draft(harness: &mut PtyHarness) -> Result<()> {
    wait_for_composer(harness, "half typed draft")
}

fn composer_is_empty(harness: &mut PtyHarness) -> Result<()> {
    wait_for_composer(harness, "Type a message")
}

/// Waits until the cursor's row, the composer line, shows `text`. The
/// transcript repeats submitted prompts, so screen-wide text is ambiguous.
fn wait_for_composer(harness: &mut PtyHarness, text: &str) -> Result<()> {
    const COMPOSER: WaitTimeout = WaitTimeout::secs(5, "composer text");
    let deadline = Instant::now() + COMPOSER.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        let screen = harness.screen();
        let (row, _) = screen.cursor();
        let line = screen.rows_text().get(usize::from(row)).cloned();
        if line.as_deref().is_some_and(|line| line.contains(text)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "composer never showed {text:?}; cursor row: {line:?}\n{}",
                screen.debug_dump()
            );
        }
    }
}
