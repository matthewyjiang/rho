//! `/compact` scenarios: the composer stays live while compaction runs.
//!
//! The daemon scenarios reuse the in-process steps unchanged: what `/compact`
//! shows and how it queues must not depend on which side runs the session.

#[cfg(unix)]
use crate::scenario::Scenario;
use crate::{harness::WaitTimeout, keys::Key, scenario::Step};

#[cfg(unix)]
use super::{daemon_resume::DAEMON_ENV, DEFAULT_SIZE};
use super::{fixture_release::release_compact_fixture, STARTUP, STREAM};

#[cfg(unix)]
pub(super) const DAEMON_TYPE_DURING_COMPACT: Scenario = Scenario::new(
    "daemon_type_during_compact",
    "Keep composer input responsive while a daemon session's /compact is running",
    DEFAULT_SIZE,
    TYPE_DURING_COMPACT_STEPS,
    /*smoke*/ false,
)
.with_env(DAEMON_ENV);

#[cfg(unix)]
pub(super) const DAEMON_SUBMIT_DURING_COMPACT: Scenario = Scenario::new(
    "daemon_submit_during_compact",
    "A prompt submitted during a daemon session's /compact starts after compaction ends",
    DEFAULT_SIZE,
    SUBMIT_DURING_COMPACT_STEPS,
    /*smoke*/ false,
)
.with_env(DAEMON_ENV);

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
