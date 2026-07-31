use anyhow::Result;

use crate::{
    harness::PtyHarness,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP, STREAM};

pub(super) const APPLY_PATCH_DIFF_SCENARIO: Scenario = Scenario::new(
    "apply_patch_diff",
    "Keep one diff card through apply_patch completion and interruption",
    DEFAULT_SIZE,
    APPLY_PATCH_DIFF_STEPS,
    false,
);

const APPLY_PATCH_DIFF_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture apply patch"),
    Step::WaitText {
        text: "+ streamed patch line",
        timeout: STREAM,
    },
    Step::Custom(assert_patch_is_still_streaming),
    Step::WaitText {
        text: "patch lifecycle complete with one result",
        timeout: STREAM,
    },
    Step::AssertText("+ streamed patch line"),
    Step::Custom(assert_one_apply_patch_card),
    Step::SubmitText("fixture cancel apply patch"),
    Step::WaitText {
        text: "+ cancelled patch line",
        timeout: STREAM,
    },
    Step::Key(crate::keys::Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::AssertText("+ cancelled patch line"),
    Step::Custom(assert_two_apply_patch_cards),
    Step::SubmitText("fixture questionnaire"),
    Step::WaitText {
        text: "Choose one color",
        timeout: STREAM,
    },
    // The question is rendered in history before the interactive modal becomes
    // active. Wait for the modal controls so Esc cannot race with host-input
    // activation and interrupt the model instead of cancelling the question.
    Step::WaitText {
        text: "esc cancel",
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

fn assert_patch_is_still_streaming(harness: &mut PtyHarness) -> Result<()> {
    if harness
        .screen()
        .contains_text("patch lifecycle complete with one result")
    {
        anyhow::bail!("apply_patch finished before the streamed diff assertion");
    }
    Ok(())
}

fn assert_one_apply_patch_card(harness: &mut PtyHarness) -> Result<()> {
    assert_apply_patch_card_count(harness, 1)
}

fn assert_two_apply_patch_cards(harness: &mut PtyHarness) -> Result<()> {
    assert_apply_patch_card_count(harness, 2)
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

fn assert_apply_patch_card_count(harness: &mut PtyHarness, expected: usize) -> Result<()> {
    let count = harness.screen().contents().matches("apply_patch(").count();
    if count != expected {
        anyhow::bail!("expected {expected} apply_patch cards, found {count}");
    }
    Ok(())
}
