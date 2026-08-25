use std::time::Duration;

use anyhow::Result;

use crate::{keys::Key, scenario::Step, PtyHarness};

use super::{SETTLE, STARTUP};

fn assert_claude_code_absent_from_login_groups(harness: &mut PtyHarness) -> Result<()> {
    // Type a claude filter so only matching groups remain; Claude Code must not
    // surface as its own top-level group.
    harness.type_text("claude")?;
    let screen = harness.screen().contents();
    let lower = screen.to_ascii_lowercase();
    if lower.contains("claude code")
        || lower.contains("claude-code")
        || lower.contains("delegation only")
    {
        anyhow::bail!(
            "claude code belongs under Anthropic methods, not top-level login groups:\n{screen}"
        );
    }
    // Clear the filter before the xAI step.
    for _ in 0.."claude".len() {
        harness.inject_key(&Key::Backspace)?;
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
    Step::AssertText("Esc back"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "select provider to login",
        timeout: SETTLE,
    },
    Step::AssertText("Esc cancel"),
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

pub(super) const LOGIN_CUSTOM_PROVIDER_STEPS: &[Step] = &[
    Step::Phase("open_custom_onboarding"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/login"),
    Step::WaitText {
        text: "select provider to login",
        timeout: SETTLE,
    },
    Step::TypeText("Chat Completions"),
    Step::WaitText {
        text: "Custom · Chat Completions",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit provider name",
        timeout: SETTLE,
    },
    // A rejected name must keep what was typed instead of clearing the field.
    Step::TypeText("openai"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "conflicts with a built-in provider",
        timeout: SETTLE,
    },
    Step::AssertText("openai"),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::TypeText("vllm"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit base URL",
        timeout: SETTLE,
    },
    Step::AssertText("http://127.0.0.1:8000/v1"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "enter API key (optional)",
        timeout: SETTLE,
    },
    Step::AssertText("saved custom provider vllm"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "vllm is ready",
        timeout: STARTUP,
    },
    Step::ExitCommand,
];

pub(super) const LOGIN_OLLAMA_STEPS: &[Step] = &[
    Step::Phase("open_ollama_onboarding"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/login ollama"),
    Step::WaitText {
        text: "edit base URL",
        timeout: SETTLE,
    },
    Step::AssertText("http://127.0.0.1:11434/v1"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "enter API key (optional)",
        timeout: SETTLE,
    },
    Step::AssertText("saved Ollama endpoint http://127.0.0.1:11434/v1"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "ollama is ready",
        timeout: STARTUP,
    },
    Step::ExitCommand,
];
