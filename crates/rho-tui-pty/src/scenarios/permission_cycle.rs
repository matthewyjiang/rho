//! Permission shortcuts change the session policy without changing saved defaults.

use anyhow::{ensure, Context, Result};

use super::{DEFAULT_SIZE, OPENAI_KEY_ENV, SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::Key,
    scenario::{Scenario, Step},
    PtyHarness,
};

fn setup_remapped_with_classifier(home: &IsolatedHome) -> Result<()> {
    let mut config = std::fs::read_to_string(&home.config_path)?;
    config.push_str(
        r#"
[keybindings]
cycle_permission_mode = "alt+n"

[internal_agents.permission-classifier]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
"#,
    );
    std::fs::write(&home.config_path, config)?;
    remember_config(home)
}

fn remember_config(home: &IsolatedHome) -> Result<()> {
    std::fs::write(
        home.workspace.join(".permission-config-path"),
        home.config_path
            .to_str()
            .context("config path is not UTF-8")?,
    )?;
    std::fs::copy(
        &home.config_path,
        home.workspace.join(".permission-original-config"),
    )?;
    Ok(())
}

fn assert_config_unchanged(harness: &mut PtyHarness) -> Result<()> {
    let workspace = harness.working_directory().context("missing workspace")?;
    let path = std::fs::read_to_string(workspace.join(".permission-config-path"))?;
    ensure!(
        std::fs::read(path)? == std::fs::read(workspace.join(".permission-original-config"))?,
        "session-only permission cycling changed the config file"
    );
    Ok(())
}

fn cycle_idle(harness: &mut PtyHarness, key: char, labels: &[&str]) -> Result<()> {
    for label in labels {
        harness.inject_key(&Key::Alt(key))?;
        harness.wait_for_text(label, SETTLE)?;
        assert_config_unchanged(harness)?;
    }
    Ok(())
}

fn cycle_default(harness: &mut PtyHarness) -> Result<()> {
    cycle_idle(
        harness,
        'm',
        &[
            "Plan ·",
            "Supervised ·",
            "Allow edits ·",
            "Bypass ·",
            "Plan ·",
        ],
    )
}

fn cycle_remapped(harness: &mut PtyHarness) -> Result<()> {
    cycle_idle(
        harness,
        'n',
        &[
            "Plan ·",
            "Supervised ·",
            "Allow edits ·",
            "Auto ·",
            "Bypass ·",
            "Plan ·",
        ],
    )
}

// Covers: idle keys apply to the statusline without persisting, skipping Auto
// without a classifier, and wrapping back to Plan.
// Owner: interactive permission shortcut
pub(super) const IDLE: Scenario = Scenario::new(
    "permission_cycle_idle",
    "Cycle session permission modes without saving or enabling unconfigured Auto",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::Custom(cycle_default),
        Step::ExitCommand,
        Step::Custom(assert_config_unchanged),
    ],
    /*smoke*/ true,
)
.with_setup(remember_config)
.with_env(OPENAI_KEY_ENV);

// Covers: a configured shortcut reaches the runtime and can enter configured
// Auto without opening a classifier picker or saving the session's mode.
// Owner: interactive permission shortcut
pub(super) const REMAPPED: Scenario = Scenario::new(
    "permission_cycle_remapped",
    "Cycle through configured Auto using a remapped session-only shortcut",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::Custom(cycle_remapped),
        Step::ExitCommand,
        Step::Custom(assert_config_unchanged),
    ],
    /*smoke*/ true,
)
.with_setup(setup_remapped_with_classifier)
.with_env(OPENAI_KEY_ENV);

// Covers: repeated presses advance the queued mode, leaving the active turn's
// policy unchanged; cancellation still applies the pending policy before reuse.
// Owner: interactive queued permission shortcut
pub(super) const QUEUED: Scenario = Scenario::new(
    "permission_cycle_queued",
    "Queue permission changes during a turn and apply the last mode after cancellation",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture gated reply"),
        Step::WaitText {
            text: "reply waiting for release",
            timeout: STREAM,
        },
        Step::Key(Key::Alt('m')),
        Step::WaitText {
            text: "permission mode plan queued for next turn",
            timeout: SETTLE,
        },
        Step::AssertText("Bypass ·"),
        Step::Key(Key::Alt('m')),
        Step::WaitText {
            text: "permission mode supervised queued for next turn",
            timeout: SETTLE,
        },
        Step::AssertText("Bypass ·"),
        Step::Custom(assert_config_unchanged),
        Step::Key(Key::Esc),
        Step::WaitText {
            text: "Supervised ·",
            timeout: STREAM,
        },
        // A later idle press must start from the applied mode, not a stale queue.
        Step::Key(Key::Alt('m')),
        Step::WaitText {
            text: "Allow edits ·",
            timeout: SETTLE,
        },
        Step::ExitCommand,
        Step::Custom(assert_config_unchanged),
    ],
    /*smoke*/ true,
)
.with_setup(remember_config)
.with_env(OPENAI_KEY_ENV);
