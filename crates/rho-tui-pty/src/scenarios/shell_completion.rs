//! Shell-mode Tab path completion.

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

const FILE_GAMMA: &str = "gamma-unique-fixture.txt";
const FILE_GAMMA_TWO: &str = "gamma-unique-second.txt";

fn setup_shell_completion(home: &IsolatedHome) -> Result<()> {
    std::fs::write(home.workspace.join(FILE_GAMMA), "gamma fixture body\n")?;
    std::fs::write(home.workspace.join(FILE_GAMMA_TWO), "gamma second body\n")?;
    Ok(())
}

// Covers: in shell mode, Tab on an ambiguous word opens a path list, typing
// narrows it to one, and Tab again inserts that path into the command.
// Owner: interactive TUI
const SHELL_TAB_COMPLETION_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_completion"),
    Step::TypeText("!cat gamma"),
    Step::WaitText {
        text: "shell",
        timeout: SETTLE,
    },
    Step::Key(Key::Tab),
    Step::WaitText {
        text: FILE_GAMMA_TWO,
        timeout: SETTLE,
    },
    Step::Phase("narrow_and_accept"),
    Step::TypeText("-unique-f"),
    Step::WaitTextGone {
        text: FILE_GAMMA_TWO,
        timeout: SETTLE,
    },
    Step::Key(Key::Tab),
    Step::WaitText {
        text: "cat gamma-unique-fixture.txt",
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];

pub(super) const SHELL_TAB_COMPLETION_SCENARIO: Scenario = Scenario::new(
    "shell_tab_completion",
    "Complete a workspace path with Tab inside the inline shell composer",
    SIZE,
    SHELL_TAB_COMPLETION_STEPS,
    /* smoke */ false,
)
.with_setup(setup_shell_completion);
