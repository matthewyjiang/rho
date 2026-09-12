//! Exercise reasoning policy through composer keys, not pre-resolved selections.

use anyhow::Result;

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::Key,
    scenario::{Scenario, Step},
    PtyHarness,
};

const AUTH_ENV: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "fixture-openai-key"),
    ("XAI_API_KEY", "fixture-xai-key"),
    ("POOLSIDE_API_KEY", "fixture-poolside-key"),
];

fn setup(home: &IsolatedHome) -> Result<()> {
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"
favorite_models = ["openai/gpt-5.5", "xai/grok-4.6", "poolside/laguna-m.1"]

[behavior]
credential_store = "file"
"#,
    )?;
    Ok(())
}

fn cycle_reverse(harness: &mut PtyHarness) -> Result<()> {
    harness.inject_key(&Key::Text("\x1b[112;6u".into()))
}

// Covers: a compatible cycle must preserve explicit reasoning so a later /model
// still rejects; an incompatible cycle must normalize instead of rejecting.
// Owner: interactive model cycling
pub(super) const IDLE: Scenario = Scenario::new(
    "cycle_reasoning_idle",
    "Preserve explicit reasoning across compatible pins and normalize incompatible pins",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("compatible_cycle"),
        Step::Key(Key::Ctrl('p')),
        Step::WaitText {
            text: "model switched to xai/grok-4.6 with reasoning medium",
            timeout: SETTLE,
        },
        Step::Phase("deliberate_switch_still_rejects"),
        Step::SubmitText("/model poolside/laguna-m.1"),
        Step::WaitText {
            text: "reasoning level 'medium' is not supported",
            timeout: SETTLE,
        },
        Step::Phase("incompatible_cycle_normalizes"),
        Step::Key(Key::Ctrl('p')),
        Step::WaitText {
            text: "model switched to poolside/laguna-m.1 with reasoning max",
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_setup(setup)
.with_env(AUTH_ENV)
.with_args(&["--reasoning", "medium"]);

// Covers: reasoning policy must survive queueing a cycle until a live run ends.
// Owner: interactive queued model cycling
pub(super) const QUEUED: Scenario = Scenario::new(
    "cycle_reasoning_queued",
    "Normalize a pinned model switch queued during a provider turn",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture delay"),
        Step::WaitText {
            text: "partial assistant before cancellation",
            timeout: STREAM,
        },
        // Reverse from the first pin goes directly to Poolside. CSI-u carries
        // Ctrl+Shift+P, which legacy Ctrl byte encoding cannot distinguish.
        Step::Custom(cycle_reverse),
        Step::WaitText {
            text: "model change to poolside/laguna-m.1 queued",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::WaitText {
            text: "model switched to poolside/laguna-m.1 with reasoning max",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_setup(setup)
.with_env(AUTH_ENV)
.with_args(&["--reasoning", "medium"]);
