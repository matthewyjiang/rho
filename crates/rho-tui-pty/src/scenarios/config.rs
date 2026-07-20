use super::*;

pub(super) const OPEN_CONFIG_PICKER_STEPS: &[Step] = &[
    Step::Phase("open_config"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/config"),
    Step::WaitText {
        text: "Models & reasoning",
        timeout: SETTLE,
    },
    Step::AssertText("Agent behavior"),
    Step::AssertText("Context & limits"),
    Step::AssertText("Tools"),
    Step::AssertText("Providers"),
    Step::AssertText("Updates"),
    Step::Phase("open_models"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Conversation model",
        timeout: SETTLE,
    },
    Step::AssertText("Session title model"),
    Step::AssertText("Show reasoning output"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Agent behavior",
        timeout: SETTLE,
    },
    Step::Phase("open_refresh_models"),
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Log in to provider",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "All configured providers",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "no refreshable providers are configured",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Refresh model lists",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Models & reasoning",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
