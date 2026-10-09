//! `editing_mode = "vim"` edits the composer modally, shows the mode on the
//! composer rule, and shares one undo history with the undo chord.

use std::{fs::OpenOptions, io::Write};

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{
    assert_helpers::wait_for_turn_completion_after, prompt_history_search::wait_for_composer,
    DEFAULT_SIZE, SETTLE, STARTUP,
};

// Covers: Esc enters normal mode instead of aborting or clearing, normal-mode
// keys edit instead of typing, `u` and Ctrl+Z walk one undo history, Enter
// still submits, and a sent prompt starts the next draft in insert mode.
// Owner: interactive TUI composer key routing and chrome.
pub(super) const COMPOSER_VIM_SCENARIO: Scenario = Scenario::new(
    "composer_vim",
    "Edit the composer with vim normal-mode commands and undo",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "INSERT",
            timeout: STARTUP,
        },
        Step::TypeText("hello world"),
        Step::Custom(composer_shows_hello_world),
        Step::Key(Key::Esc),
        Step::WaitText {
            text: "NORMAL",
            timeout: SETTLE,
        },
        Step::Phase("operator_and_undo"),
        Step::TypeText("0dw"),
        Step::WaitTextGone {
            text: "hello",
            timeout: SETTLE,
        },
        Step::TypeText("u"),
        Step::Custom(composer_shows_hello_world),
        // Typing coalesces by word, so the undo chord drops "world" alone.
        Step::Key(Key::Ctrl('z')),
        Step::WaitTextGone {
            text: "world",
            timeout: SETTLE,
        },
        Step::Phase("append_and_submit"),
        Step::TypeText("Athere"),
        Step::WaitText {
            text: "INSERT",
            timeout: SETTLE,
        },
        Step::Custom(submit_and_wait),
        Step::Custom(composer_is_empty),
        Step::WaitText {
            text: "INSERT",
            timeout: SETTLE,
        },
        Step::CtrlCExit,
    ],
    /*smoke*/ false,
)
.with_setup(enable_vim);

fn enable_vim(home: &IsolatedHome) -> Result<()> {
    writeln!(
        OpenOptions::new().append(true).open(&home.config_path)?,
        "\n[keybindings]\nediting_mode = \"vim\""
    )?;
    Ok(())
}

fn composer_shows_hello_world(harness: &mut PtyHarness) -> Result<()> {
    wait_for_composer(harness, "hello world")
}

fn composer_is_empty(harness: &mut PtyHarness) -> Result<()> {
    wait_for_composer(harness, "Type a message")
}

fn submit_and_wait(harness: &mut PtyHarness) -> Result<()> {
    harness.settle_plain_text_input();
    harness.inject_key(&Key::Enter)?;
    wait_for_turn_completion_after(harness, "fixture response: hello there")
}
