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
    // Running the refresh would ask real provider endpoints what they host, so
    // the scenario stops at the choice and leaves the network out of it.
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Refresh model lists",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Models & reasoning",
        timeout: SETTLE,
    },
    Step::Phase("select_edit_tool"),
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Inline shell",
        timeout: SETTLE,
    },
    Step::AssertText("Edit tool"),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Hash-line",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Apply patch",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
