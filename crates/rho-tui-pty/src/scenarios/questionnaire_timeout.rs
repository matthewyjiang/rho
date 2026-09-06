//! The automatic result is a durable wait target. Pause routing uses a long
//! deadline so slow CI cannot race input against an intermediate countdown.
//! Match the active-question marker: bare prompt text also survives in tool cards
//! after submission and would allow input before the next form opens.
use super::{DEFAULT_SIZE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::Key,
    scenario::{Scenario, Step},
};
use anyhow::Result;

fn setup(home: &IsolatedHome, seconds: u64) -> Result<()> {
    let mut config = std::fs::read_to_string(&home.config_path)?;
    config.push_str(&format!("\n[questionnaire]\ntimeout_seconds = {seconds}\n"));
    std::fs::write(&home.config_path, config)?;
    Ok(())
}

pub(super) const TIMEOUT: Scenario = Scenario::new(
    "questionnaire_timeout",
    "Configure a timeout and submit explicit fallback answers with distinct provenance",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/config"),
        Step::WaitText {
            text: "Config · saves automatically",
            timeout: STARTUP,
        },
        Step::TypeText("agent"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Config / Agent behavior",
            timeout: STREAM,
        },
        Step::TypeText("questionnaire"),
        Step::WaitText {
            text: "Disabled",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "edit questionnaire timeout seconds",
            timeout: STREAM,
        },
        Step::TypeText("0"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "questionnaire timeout must be positive whole seconds",
            timeout: STREAM,
        },
        Step::Key(Key::Backspace),
        Step::TypeText("1"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Config / Agent behavior",
            timeout: STREAM,
        },
        Step::Key(Key::Esc),
        Step::Key(Key::Esc),
        Step::SubmitText("fixture questionnaire timeout"),
        Step::WaitText {
            text: "questionnaire response observed exactly 1 time",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "\"source\":\"timeout_fallback\"",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "\"answer\":\"blue\"",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    true,
);

pub(super) const PAUSE: Scenario = Scenario::new(
    "questionnaire_timeout_pause",
    "Pause fallback on keyboard, paste, and mouse interaction, then submit as user",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture questionnaire timeout"),
        Step::WaitText {
            text: "▸ Choose a fallback color",
            timeout: STREAM,
        },
        Step::Key(Key::Down),
        Step::WaitText {
            text: "Fallback paused",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "\"source\":\"user\"",
            timeout: STREAM,
        },
        Step::SubmitText("fixture questionnaire timeout"),
        Step::WaitText {
            text: "▸ Choose a fallback color",
            timeout: STREAM,
        },
        Step::Paste("green"),
        Step::WaitText {
            text: "Fallback paused",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "\"answer\":\"green\"",
            timeout: STREAM,
        },
        Step::SubmitText("fixture questionnaire timeout"),
        Step::WaitText {
            text: "▸ Choose a fallback color",
            timeout: STREAM,
        },
        Step::Custom(|harness| harness.mouse_move(1, 1)),
        Step::WaitText {
            text: "Fallback paused",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "\"answer\":\"red\"",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    true,
)
.with_setup(|home| setup(home, 3600));
