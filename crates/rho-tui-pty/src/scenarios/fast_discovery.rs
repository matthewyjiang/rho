//! Fast-mode discovery follows the active provider and model, not startup state.

use std::time::Duration;

use anyhow::{ensure, Result};

use super::{DEFAULT_SIZE, SETTLE, STARTUP};
use crate::{
    env::IsolatedHome,
    keys::Key,
    scenario::{Scenario, Step},
    PtyHarness,
};

// Covers: unsupported models must not advertise /fast, including the exact-token
// argument path, and switching back must restore discovery without restarting.
// Owner: interactive command palette. Runtime on/off semantics have separate tests.
pub(super) const FAST_DISCOVERY_SCENARIO: Scenario = Scenario::new(
    "fast_command_discovery",
    "Update fast command and argument suggestions after model switches",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "OpenAI Codex · gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(assert_supported),
        Step::Phase("unsupported_codex_model"),
        Step::SubmitText("/model openai-codex/gpt-5.3-codex-spark"),
        Step::WaitText {
            text: "OpenAI Codex · gpt-5.3-codex-spark",
            timeout: SETTLE,
        },
        Step::Custom(assert_hidden),
        Step::Phase("hidden_command_still_dispatches"),
        Step::SubmitText("/fast on"),
        Step::WaitText {
            text: "fast mode is not available for openai-codex/gpt-5.3-codex-spark",
            timeout: SETTLE,
        },
        Step::SubmitText("/fast off"),
        Step::WaitText {
            text: "fast mode is off",
            timeout: SETTLE,
        },
        Step::Phase("same_model_other_provider"),
        Step::SubmitText("/model openai/gpt-5.5"),
        Step::WaitText {
            text: "OpenAI · gpt-5.5",
            timeout: SETTLE,
        },
        Step::Custom(assert_hidden),
        Step::Phase("supported_again"),
        Step::SubmitText("/model openai-codex/gpt-5.5"),
        Step::WaitText {
            text: "OpenAI Codex · gpt-5.5",
            timeout: SETTLE,
        },
        Step::Custom(assert_supported),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_setup(setup)
.with_env(&[
    ("CODEX_ACCESS_TOKEN", "fixture-codex-token"),
    ("OPENAI_API_KEY", "fixture-openai-key"),
]);

fn setup(home: &IsolatedHome) -> Result<()> {
    // Codex uses the static model catalog. Fake auth only unlocks /model;
    // matrix mode replaces the provider transport, including after switches.
    std::fs::write(
        &home.config_path,
        r#"provider = "openai-codex"
model = "gpt-5.5"
auth = "codex"
check_for_updates = false
web_search.mode = "off"

[behavior]
credential_store = "file"
"#,
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Discovery {
    Shown,
    Hidden,
}

fn assert_supported(harness: &mut PtyHarness) -> Result<()> {
    assert_discovery(harness, Discovery::Shown)
}

fn assert_hidden(harness: &mut PtyHarness) -> Result<()> {
    assert_discovery(harness, Discovery::Hidden)
}

fn assert_discovery(harness: &mut PtyHarness, discovery: Discovery) -> Result<()> {
    // Append to exercise /fas, exact /fast, then the whitespace argument path.
    for (suffix, suggestions) in [
        ("/fas", &["/fast"][..]),
        ("t", &["/fast on", "/fast off"][..]),
        (" ", &["/fast on", "/fast off"][..]),
    ] {
        harness.type_text(suffix)?;
        harness.wait_for_quiet(Duration::from_millis(150), SETTLE)?;
        // The draft itself contains /fast. Exclude its cursor row so assertions
        // inspect suggestions rather than mistaking typed input for discovery.
        let cursor_row = usize::from(harness.screen().cursor().0);
        let palette = harness
            .screen()
            .rows_text()
            .into_iter()
            .enumerate()
            .filter(|(row, _)| *row != cursor_row)
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n");
        match discovery {
            Discovery::Shown => {
                for suggestion in suggestions {
                    ensure!(
                        palette.contains(suggestion),
                        "missing fast suggestion {suggestion:?}:\n{palette}"
                    );
                }
            }
            Discovery::Hidden => ensure!(
                !palette.contains("/fast"),
                "unsupported model advertises fast mode:\n{palette}"
            ),
        }
    }
    harness.inject_key(&Key::Esc)?;
    harness.inject_key(&Key::Ctrl('c'))?;
    harness.wait_for_text_gone("/fas", SETTLE)
}
