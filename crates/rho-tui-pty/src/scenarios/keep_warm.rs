//! `behavior.keep_cache_warm`: a `process start` that asked for `keep_warm`
//! holds the provider prompt cache, and the session's host pings it while
//! the session idles. Each scenario runs in process and with
//! `RHO_TUI_DAEMON=1`, where the daemon's session owns the warming.

use anyhow::Result;

#[cfg(unix)]
use super::daemon_resume::DAEMON_ENV;
use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    harness::WaitTimeout,
    scenario::{Scenario, Step},
};

/// The custom host's model, as the status line names it.
const MODEL: &str = "warm-model";
const PROMPT: &str = "fixture keep warm";
const DISPATCHED: &str = "keep warm fixture dispatched";

/// A custom host, whose prompt cache lifetime the config can set and whose
/// model has no catalog price, so its ping budget is the fallback 5.
fn write_config(home: &IsolatedHome, provider: &str, behavior: &str) -> Result<()> {
    std::fs::write(
        &home.config_path,
        format!(
            r#"web_search.mode = "off"

[model]
provider = "warm"
model = "{MODEL}"
auth = "warm-api-key"

[providers.custom.warm]
base_url = "http://127.0.0.1:9/v1"
{provider}
[behavior]
credential_store = "file"
{behavior}"#
        ),
    )?;
    Ok(())
}

/// A ping is due half a lifetime after the last request: 3 s here, and the
/// cache is cold 3 s after that, which leaves a loaded host room to send it.
fn short_lived_cache(home: &IsolatedHome) -> Result<()> {
    write_config(home, "prompt_cache_ttl = \"6s\"\n", "")
}

fn setting_off(home: &IsolatedHome) -> Result<()> {
    write_config(home, "", "keep_cache_warm = false\n")
}

// Covers: a `keep_warm` process holds the cache once its turn ends, the
// host pings on the cache's schedule with nobody typing, a ping that read
// the cache earns another, one that missed stops the warming with a notice,
// and `/info` totals what both cost.
// Owner: interactive UX (PTY)
pub(super) const KEEP_WARM_PINGS_SCENARIO: Scenario = Scenario::new(
    "keep_warm_pings",
    "Ping the prompt cache while idle, stop on a miss, and total the spend",
    DEFAULT_SIZE,
    &[
        Step::Phase("startup"),
        Step::WaitText {
            text: MODEL,
            timeout: STARTUP,
        },
        Step::Phase("start_held_process"),
        Step::SubmitText(PROMPT),
        Step::WaitText {
            text: DISPATCHED,
            timeout: STREAM,
        },
        Step::Phase("second_ping_misses"),
        // The fixture's first ping reads the cache and its second misses,
        // 3 s and 6 s after the turn.
        Step::WaitText {
            text: "cache warm stopped: a ping missed the prompt cache",
            timeout: WaitTimeout::secs(30, "two pings on a 6 s cache"),
        },
        Step::Phase("spend"),
        Step::SubmitText("/info"),
        // Two pings at the fixture's $0.001 each.
        Step::WaitText {
            text: "$0.002",
            timeout: SETTLE,
        },
        Step::Key(crate::keys::Key::Esc),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(short_lived_cache);

// Covers: with the setting off, `process start` offers the model no
// `keep_warm`, so nothing can hold the cache.
// Owner: interactive UX (PTY)
pub(super) const KEEP_WARM_OFF_SCENARIO: Scenario = Scenario::new(
    "keep_warm_off",
    "Offer no keep_warm on process start when keep_cache_warm is off",
    DEFAULT_SIZE,
    &[
        Step::Phase("startup"),
        Step::WaitText {
            text: MODEL,
            timeout: STARTUP,
        },
        Step::Phase("start_held_process"),
        Step::SubmitText(PROMPT),
        Step::WaitText {
            text: "keep warm fixture: keep_warm not offered",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setting_off);

#[cfg(unix)]
pub(super) const DAEMON_KEEP_WARM_PINGS_SCENARIO: Scenario = Scenario {
    id: "daemon_keep_warm_pings",
    description: "Ping a daemon session's prompt cache, stop on a miss, and total the spend",
    env: DAEMON_ENV,
    ..KEEP_WARM_PINGS_SCENARIO
};

#[cfg(unix)]
pub(super) const DAEMON_KEEP_WARM_OFF_SCENARIO: Scenario = Scenario {
    id: "daemon_keep_warm_off",
    description: "Offer a daemon session no keep_warm when keep_cache_warm is off",
    env: DAEMON_ENV,
    ..KEEP_WARM_OFF_SCENARIO
};
