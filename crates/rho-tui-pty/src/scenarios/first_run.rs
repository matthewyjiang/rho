//! First-run and signed-out setup states in the session header and statusline.

use crate::{keys::Key, scenario::Step};

use super::{SETTLE, STARTUP};

/// Environment that forces the first-run presentation without deleting a config.
pub(super) const FIRST_RUN_ENV: &[(&str, &str)] = &[("RHO_FIRST_RUN", "1")];

pub(super) const FIRST_RUN_WELCOME_STEPS: &[Step] = &[
    Step::Phase("welcome_header"),
    Step::WaitText {
        text: "Welcome to Rho",
        timeout: STARTUP,
    },
    // Credentials are already in place under the fixture, so the header points
    // at the composer rather than at login.
    Step::AssertText("Type a prompt and press enter."),
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
    // and the header hints lead with login.
    Step::AssertText("not signed in"),
    Step::AssertText("Sign in to a provider"),
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
    Step::Phase("exit"),
    // Clear the held prompt so /exit is read as a command, not as a suffix.
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];
