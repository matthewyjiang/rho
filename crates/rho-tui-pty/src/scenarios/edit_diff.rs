use anyhow::Result;

use crate::{
    harness::PtyHarness,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP, STREAM};

pub(super) const EDIT_DIFF_SCENARIO: Scenario = Scenario::new(
    "edit_diff",
    "Keep one diff card through edit completion and interruption",
    DEFAULT_SIZE,
    EDIT_DIFF_STEPS,
    false,
);

const EDIT_DIFF_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture edit"),
    Step::WaitText {
        text: "edit(",
        timeout: STREAM,
    },
    Step::Custom(assert_edit_is_still_streaming),
    Step::WaitText {
        text: "edit lifecycle complete with one result",
        timeout: STREAM,
    },
    Step::AssertText("streamed edit line"),
    Step::Custom(assert_one_edit_card),
    Step::SubmitText("fixture cancel edit"),
    // Do not WaitText("edit(") here: the completed first card still matches and
    // Esc can race ahead of the second stream. Wait for cancel-fixture content.
    Step::WaitText {
        text: "cancelled edit line",
        timeout: STREAM,
    },
    Step::Key(crate::keys::Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::Custom(assert_two_edit_cards),
    Step::SubmitText("fixture questionnaire"),
    Step::WaitText {
        text: "Choose one color",
        timeout: STREAM,
    },
    // The question is rendered in history before the interactive modal becomes
    // active. Wait for the modal controls so Esc cannot race with host-input
    // activation and interrupt the model instead of cancelling the question.
    Step::WaitText {
        text: "Esc cancel",
        timeout: STREAM,
    },
    Step::Key(crate::keys::Key::Esc),
    Step::WaitText {
        text: "questionnaire cancelled",
        timeout: STREAM,
    },
    Step::AssertText("Choose one color"),
    Step::Custom(assert_one_questionnaire_card),
    Step::ExitCommand,
];

fn assert_edit_is_still_streaming(harness: &mut PtyHarness) -> Result<()> {
    if harness
        .screen()
        .contains_text("edit lifecycle complete with one result")
    {
        anyhow::bail!("edit finished before the streamed diff assertion");
    }
    Ok(())
}

fn assert_one_edit_card(harness: &mut PtyHarness) -> Result<()> {
    assert_edit_card_count(harness, 1)
}

fn assert_two_edit_cards(harness: &mut PtyHarness) -> Result<()> {
    let contents = harness.screen().contents();
    let count = contents.matches("edit(").count();
    if count != 2 {
        anyhow::bail!("expected 2 edit cards, found {count}");
    }
    // Unique fixture payloads prove both cards are still on-screen, not just
    // two header hits from wrap or a single card rendered twice.
    if !contents.contains("streamed edit line") {
        anyhow::bail!("missing completed edit card content on screen");
    }
    if !contents.contains("cancelled edit line") {
        anyhow::bail!("missing interrupted edit card content on screen");
    }
    Ok(())
}

fn assert_one_questionnaire_card(harness: &mut PtyHarness) -> Result<()> {
    let count = harness
        .screen()
        .contents()
        .matches("questionnaire(")
        .count();
    if count != 1 {
        anyhow::bail!("expected one questionnaire card, found {count}");
    }
    Ok(())
}

fn assert_edit_card_count(harness: &mut PtyHarness, expected: usize) -> Result<()> {
    let count = harness.screen().contents().matches("edit(").count();
    if count != expected {
        anyhow::bail!("expected {expected} edit cards, found {count}");
    }
    Ok(())
}
