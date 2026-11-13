//! `/compact` scenarios: the composer stays live while compaction runs.

use anyhow::{ensure, Context, Result};

use crate::{
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{fixture_release::release_compact_fixture, DEFAULT_SIZE, STARTUP, STREAM};

pub(super) const COMPACT_COMPLETES: Scenario = Scenario::new(
    "compact_completes",
    "A completed /compact shows its card alone and says the context compacted",
    DEFAULT_SIZE,
    COMPACT_COMPLETES_STEPS,
    /*smoke*/ false,
);

pub(super) const TYPE_DURING_COMPACT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("seed_history"),
    Step::SubmitText("fixture compact until cancel"),
    Step::WaitText {
        text: "fixture response: fixture compact until cancel",
        timeout: STREAM,
    },
    Step::Phase("compact"),
    Step::SubmitText("/compact"),
    Step::WaitText {
        text: "compacting context",
        timeout: STREAM,
    },
    Step::Phase("type_draft"),
    Step::TypeText("draft during compact"),
    Step::WaitText {
        text: "draft during compact",
        timeout: WaitTimeout::secs(2, "composer input during compact"),
    },
    Step::Phase("cancel_compact"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "context compaction cancelled",
        timeout: WaitTimeout::secs(2, "esc cancels compact"),
    },
    Step::WaitText {
        text: "draft during compact",
        timeout: WaitTimeout::secs(2, "draft survives compact cancel"),
    },
    Step::CtrlCExit,
];

pub(super) const SUBMIT_DURING_COMPACT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("seed_history"),
    Step::SubmitText("fixture compact until release"),
    Step::WaitText {
        text: "fixture response: fixture compact until release",
        timeout: STREAM,
    },
    Step::Phase("compact"),
    Step::SubmitText("/compact"),
    Step::WaitText {
        text: "compacting context",
        timeout: STREAM,
    },
    Step::Phase("submit_follow_up"),
    Step::SubmitText("after compact please"),
    Step::WaitText {
        text: "1 follow-up",
        timeout: WaitTimeout::secs(2, "queued follow-up during compact"),
    },
    Step::Phase("release_compact"),
    Step::Custom(release_compact_fixture),
    Step::Phase("drain_after_failed_compact"),
    Step::WaitText {
        text: "fixture response: after compact please",
        timeout: STREAM,
    },
    Step::CtrlCExit,
];

/// The prompt sent once `/compact` ended, whose row bounds what it showed.
const AFTER_COMPACT: &str = "after the compact";

const COMPACT_COMPLETES_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("seed_history"),
    Step::SubmitText("fixture calibrated context history fresh"),
    Step::WaitText {
        text: "Calibration history ready: fresh.",
        timeout: STREAM,
    },
    Step::SubmitText("fixture calibrated context history resumed"),
    Step::WaitText {
        text: "Calibration history ready: resumed.",
        timeout: STREAM,
    },
    Step::Phase("compact"),
    Step::SubmitText("/compact"),
    Step::WaitText {
        text: "context compacted",
        timeout: STREAM,
    },
    Step::Phase("next_turn"),
    Step::SubmitText(AFTER_COMPACT),
    Step::WaitText {
        text: "fixture response: after the compact",
        timeout: STREAM,
    },
    Step::Custom(compact_showed_its_card_alone),
    Step::CtrlCExit,
];

/// Between the seed turn's footer and the next prompt, `/compact` left its
/// card with the token reduction and nothing else: no row for the command,
/// no reply repeating the card, and no footer of its own. Run once the next
/// turn answered, so anything `/compact` would still show is on screen.
fn compact_showed_its_card_alone(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let rows = harness.screen().rows_text();
    let next_prompt = rows
        .iter()
        .position(|row| row.trim() == AFTER_COMPACT)
        .with_context(|| format!("the next prompt's row is missing:\n{screen}"))?;
    let seed_footer = rows[..next_prompt]
        .iter()
        .rposition(|row| row.contains("Worked for"))
        .with_context(|| format!("the seed turn's footer is missing:\n{screen}"))?;
    let shown: Vec<_> = rows[seed_footer + 1..next_prompt]
        .iter()
        .map(|row| row.trim())
        .filter(|row| !row.is_empty() && !row.starts_with("session titled:"))
        .collect();
    ensure!(
        matches!(
            shown.as_slice(),
            [header, tokens, messages]
                if header.ends_with("compact")
                    && tokens.contains('→') && tokens.contains("tokens") && tokens.contains('−')
                    && messages.contains("messages")
        ),
        "/compact showed more or less than its card: {shown:#?}\n{screen}"
    );
    Ok(())
}
