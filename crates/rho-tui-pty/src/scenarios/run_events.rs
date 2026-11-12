//! Run events beside the reply: a provider retry, the provider's own web
//! search, and a failed turn's provider diagnostic, from a turn the daemon
//! runs (#1667, #1685).

use anyhow::Result;

use crate::{
    harness::PtyHarness,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

/// The idle composer's placeholder; a running turn shows `esc abort`.
const IDLE_COMPOSER: &str = "Type a message";

// Covers: a request the provider rate limited is retried, the retry's
// status shows while the retried request runs, and its reply follows.
// Owner: interactive TUI
pub(super) const PROVIDER_RETRY_SCENARIO: Scenario = Scenario::new(
    "provider_retry",
    "Show a rate-limited provider retry, then its reply",
    DEFAULT_SIZE,
    PROVIDER_RETRY_STEPS,
    /*smoke*/ false,
);

// Covers: the provider's own web search shows as a finished card with its
// query, before the reply.
// Owner: interactive TUI
pub(super) const WEB_SEARCH_ACTIVITY_SCENARIO: Scenario = Scenario::new(
    "web_search_activity",
    "Show the provider's web search as a finished card",
    DEFAULT_SIZE,
    WEB_SEARCH_STEPS,
    /*smoke*/ false,
);

// Covers: a turn the provider failed with a diagnostic shows the
// diagnostic, and the composer is free again.
// Owner: interactive TUI
pub(super) const PROVIDER_DIAGNOSTIC_SCENARIO: Scenario = Scenario::new(
    "provider_diagnostic",
    "Show a failed turn's provider diagnostic",
    DEFAULT_SIZE,
    PROVIDER_DIAGNOSTIC_STEPS,
    /*smoke*/ false,
);

const PROVIDER_RETRY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("retry"),
    Step::SubmitText("fixture provider retry"),
    // The fixture holds the retried request until the release below, so the
    // status stays until then.
    Step::WaitText {
        text: "rate limited · retrying",
        timeout: STREAM,
    },
    Step::Custom(release_retry),
    Step::Phase("reply"),
    Step::WaitText {
        text: "reply after provider retry",
        timeout: STREAM,
    },
    Step::WaitText {
        text: IDLE_COMPOSER,
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const WEB_SEARCH_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("search"),
    Step::SubmitText("fixture web search"),
    Step::WaitText {
        text: "web search reply",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "web_search",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "fixture search query",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: IDLE_COMPOSER,
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const PROVIDER_DIAGNOSTIC_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("failure"),
    Step::SubmitText("fixture provider diagnostic"),
    Step::WaitText {
        text: "provider diagnostic:",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "fixture diagnostic detail",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: IDLE_COMPOSER,
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

/// Lets the fixture's held retry reply.
fn release_retry(harness: &mut PtyHarness) -> Result<()> {
    // Marker owner: rho-providers/src/providers/tui_fixture/stream_scenarios.rs.
    super::release_fixture(harness, ".rho-fixture-release-provider-retry")
}
