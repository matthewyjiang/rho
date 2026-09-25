//! `/compact` scenarios: the composer stays live while compaction runs.

use crate::{harness::WaitTimeout, keys::Key, scenario::Step};

use super::{fixture_release::release_compact_fixture, STARTUP, STREAM};

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
