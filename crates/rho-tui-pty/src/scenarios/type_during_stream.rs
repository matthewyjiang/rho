use super::STREAM;

use crate::{harness::PtyHarness, scenario::Step};

pub(super) fn wait_for_later_flood_event(harness: &mut PtyHarness) -> anyhow::Result<()> {
    let current = highest_visible_flood_event(harness.screen().contents());
    let target = format!(
        "input flood event {:03}",
        current.saturating_add(20).min(400)
    );
    harness.wait_for_text(&target, STREAM)?;
    if harness.screen().contains_text("model interrupted") {
        anyhow::bail!("overlay Esc aborted the live turn");
    }
    Ok(())
}

fn highest_visible_flood_event(screen: String) -> u16 {
    screen
        .split("input flood event ")
        .skip(1)
        .filter_map(|rest| rest.get(..3)?.parse().ok())
        .max()
        .unwrap_or(10)
}

pub(super) const TYPE_DURING_STREAM_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: super::STARTUP,
    },
    Step::Phase("start_flood"),
    Step::SubmitText("fixture input flood"),
    Step::WaitText {
        text: "input flood event 010",
        timeout: STREAM,
    },
    Step::Phase("query_limits"),
    Step::SubmitText("/limits"),
    Step::WaitText {
        text: "Usage limits",
        timeout: STREAM,
    },
    Step::Phase("overlay_esc_does_not_abort"),
    Step::Key(crate::keys::Key::Esc),
    Step::WaitTextGone {
        text: "Usage limits",
        timeout: super::SETTLE,
    },
    Step::Custom(wait_for_later_flood_event),
    Step::WaitTextGone {
        text: "model interrupted",
        timeout: super::SETTLE,
    },
    Step::Phase("type_draft"),
    Step::TypeText("draft while streaming"),
    Step::WaitText {
        text: "draft while streaming",
        timeout: crate::harness::WaitTimeout::secs(2, "composer input during stream"),
    },
    Step::Phase("abort_empty_composer"),
    Step::Key(crate::keys::Key::Ctrl('c')),
    Step::WaitText {
        text: "input cleared",
        timeout: super::SETTLE,
    },
    Step::WaitTextGone {
        text: "draft while streaming",
        timeout: super::SETTLE,
    },
    Step::Key(crate::keys::Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::ExitCommand,
];
