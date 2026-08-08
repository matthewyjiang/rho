//! First-launch setup screen, and the signed-out state a session falls back to.

use anyhow::Result;

use crate::{keys::Key, scenario::Step, IsolatedHome, PtyHarness};

use super::{SETTLE, STARTUP};

/// Opens setup and lets it choose its own step, as a real first launch does.
pub(super) const FIRST_RUN_ENV: &[(&str, &str)] = &[("RHO_FIRST_RUN", "1")];

/// Opens setup at sign-in whatever credentials are around, so the provider
/// menu is reachable on a machine that could already list models.
pub(super) const FIRST_RUN_SIGNIN_ENV: &[(&str, &str)] = &[("RHO_FIRST_RUN", "signin")];

/// Give the workspace a prompt template, so the signed-out scenario can submit
/// a prompt that the composer clears while it expands.
pub(super) fn setup_prompt_template(home: &IsolatedHome) -> Result<()> {
    let prompts = home.workspace.join(".rho").join("prompts");
    std::fs::create_dir_all(&prompts)?;
    std::fs::write(prompts.join("greet.md"), "expanded template body")?;
    Ok(())
}

/// Setup owns the whole screen, so nothing a session draws may show through.
fn assert_session_chrome_hidden(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    for chrome in ["Type a message", "shift+tab", "Auto ·"] {
        if screen.contains(chrome) {
            anyhow::bail!("setup screen leaked session chrome {chrome:?}:\n{screen}");
        }
    }
    Ok(())
}

pub(super) const FIRST_RUN_SETUP_STEPS: &[Step] = &[
    Step::Phase("setup_opens_on_sign_in"),
    Step::WaitText {
        text: "Welcome",
        timeout: STARTUP,
    },
    Step::AssertText("Sign in to a provider"),
    Step::AssertText("Choose a model"),
    Step::AssertText("Esc to skip setup"),
    Step::Custom(assert_session_chrome_hidden),
    // The provider menu is the step's whole point, so it must list providers
    // to choose between rather than the one this scenario happens to filter to.
    Step::Phase("provider_menu"),
    Step::AssertText("select provider to login"),
    Step::AssertText("Anthropic"),
    Step::AssertText("GitHub Copilot"),
    Step::AssertText("Google Gemini"),
    Step::Phase("sign_in"),
    Step::TypeText("openai"),
    Step::WaitText {
        text: "OpenAI",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "select OpenAI login method",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    // The isolated home already names a credential store, so the API key
    // prompt follows the method directly.
    Step::WaitText {
        text: "enter OpenAI API key",
        timeout: SETTLE,
    },
    Step::TypeText("sk-fixture-key"),
    Step::Key(Key::Enter),
    // Matrix runs seed the fixture model cache, so the model step lists the
    // fixture model and Enter confirms it before setup hands off.
    Step::Phase("choose_model"),
    Step::WaitText {
        text: "select model",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::Phase("hand_off_to_session"),
    Step::WaitText {
        text: "Type a message",
        timeout: STARTUP,
    },
    Step::AssertText("shift+tab"),
    Step::Phase("exit"),
    Step::ExitCommand,
];

/// Esc at the first step leaves setup for a normal session, so a user who does
/// not want to sign in yet is never stuck on a screen with no way out.
pub(super) const FIRST_RUN_SKIP_STEPS: &[Step] = &[
    Step::Phase("setup_opens"),
    Step::WaitText {
        text: "Esc to skip setup",
        timeout: STARTUP,
    },
    Step::Custom(assert_session_chrome_hidden),
    // A narrow pane keeps the welcome, the steps, and the picker readable.
    Step::Phase("narrow_pane"),
    Step::Resize { rows: 24, cols: 60 },
    Step::WaitText {
        text: "Sign in to a provider",
        timeout: SETTLE,
    },
    Step::AssertText("Choose a model"),
    Step::AssertText("Esc to skip setup"),
    Step::Phase("skip"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Type a message",
        timeout: STARTUP,
    },
    Step::AssertText("shift+tab"),
    Step::Phase("exit"),
    Step::ExitCommand,
];

pub(super) const SIGNED_OUT_SETUP_STEPS: &[Step] = &[
    Step::Phase("signed_in_baseline"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("sign_out"),
    Step::SubmitText("/logout openai"),
    Step::WaitText {
        text: "no longer has credentials",
        timeout: SETTLE,
    },
    // The statusline names the gap instead of a model the session cannot reach,
    // and the header hints lead with the command that fixes it.
    Step::AssertText("not signed in"),
    Step::AssertText("/login       Sign in to a provider"),
    Step::Phase("prompt_opens_login_picker"),
    Step::SubmitText("hello"),
    Step::WaitText {
        text: "select provider to login",
        timeout: SETTLE,
    },
    Step::AssertText("OpenAI"),
    Step::AssertText("still in the composer"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "hello",
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    // A template command clears the composer while it expands, so the expanded
    // prompt must be written back rather than lost with the text that made it.
    Step::Phase("template_prompt_survives"),
    Step::SubmitText("/prompt:greet with a trailing note"),
    Step::WaitText {
        text: "select provider to login",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "expanded template body with a trailing note",
        timeout: SETTLE,
    },
    Step::Phase("exit"),
    // Clear the held prompt so /exit is read as a command, not as a suffix.
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];
