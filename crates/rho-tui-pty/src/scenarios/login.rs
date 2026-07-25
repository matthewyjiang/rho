use std::time::Duration;

use anyhow::Result;

use crate::{keys::Key, scenario::Step, PtyHarness};

use super::{SETTLE, STARTUP};

fn assert_claude_code_absent_from_login_groups(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let lower = screen.to_ascii_lowercase();
    if lower.contains("claude code") || lower.contains("claude-code") {
        anyhow::bail!(
            "claude code belongs under Anthropic methods, not top-level login groups:\n{screen}"
        );
    }
    Ok(())
}

pub(super) const LOGIN_PROVIDER_GROUPS_STEPS: &[Step] = &[
    Step::Phase("open_group_picker"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/login"),
    Step::WaitText {
        text: "select provider to login",
        timeout: SETTLE,
    },
    // Overlay height caps visible rows; assert on-screen labels, then filter
    // for providers that sit below the fold.
    Step::AssertText("OpenAI"),
    Step::AssertText("Anthropic"),
    Step::AssertText("Moonshot AI"),
    Step::Custom(assert_claude_code_absent_from_login_groups),
    Step::TypeText("xAI"),
    Step::WaitText {
        text: "xAI",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Phase("open_openai_methods"),
    Step::SubmitText("/login"),
    Step::WaitText {
        text: "select provider to login",
        timeout: SETTLE,
    },
    Step::TypeText("OpenAI"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "select OpenAI login method",
        timeout: SETTLE,
    },
    Step::AssertText("API Key"),
    Step::AssertText("OAuth"),
    Step::AssertText("Esc to back"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "select provider to login",
        timeout: SETTLE,
    },
    Step::AssertText("Esc to cancel"),
    Step::Phase("close_group_picker"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Phase("open_anthropic_methods"),
    Step::SubmitText("/login"),
    Step::WaitText {
        text: "select provider to login",
        timeout: SETTLE,
    },
    Step::TypeText("Anthropic"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "select Anthropic login method",
        timeout: SETTLE,
    },
    Step::AssertText("API Key"),
    Step::AssertText("Claude Code (delegation only)"),
    // Claude Code carries the detail pane; select it so ownership copy is visible.
    Step::Key(Key::Down),
    Step::WaitText {
        text: "not Anthropic API billing",
        timeout: SETTLE,
    },
    Step::AssertText("External Claude binary"),
    // Footer truncates long detail; assert the visible ownership prefix.
    Step::AssertText("Credentials are managed by Claude"),
    // Choose API Key deliberately after browsing methods, not by default selection.
    Step::TypeText("API Key"),
    Step::WaitText {
        text: "API Key",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "enter Anthropic API key",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
