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

const DIR: &str = "gamma-unique-dir";
const FILE_ONE: &str = "gamma-unique-fixture.txt";
const FILE_TWO: &str = "gamma-unique-second.txt";

fn setup_shell_completion(home: &IsolatedHome) -> Result<()> {
    let dir = home.workspace.join(DIR);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(FILE_ONE), "gamma fixture body\n")?;
    std::fs::write(dir.join(FILE_TWO), "gamma second body\n")?;
    Ok(())
}

// Covers: Tab completes one path component per press. A lone directory
// match inserts `dir/` and stays open for descent; inside it an ambiguous
// word opens the directory's own entries, typing narrows to one, and Tab
// inserts that file.
// Owner: interactive TUI
const SHELL_TAB_COMPLETION_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("descend_into_directory"),
    Step::TypeText("!cat gamma"),
    Step::WaitText {
        text: "shell · included in context",
        timeout: SETTLE,
    },
    Step::Key(Key::Tab),
    Step::WaitText {
        text: "cat gamma-unique-dir/",
        timeout: SETTLE,
    },
    Step::Phase("list_directory_entries"),
    Step::Key(Key::Tab),
    Step::WaitText {
        text: FILE_TWO,
        timeout: SETTLE,
    },
    Step::Phase("narrow_and_accept"),
    Step::TypeText("gamma-unique-f"),
    Step::WaitTextGone {
        text: FILE_TWO,
        timeout: SETTLE,
    },
    Step::Key(Key::Tab),
    Step::WaitText {
        text: "cat gamma-unique-dir/gamma-unique-fixture.txt",
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];

pub(super) const SHELL_TAB_COMPLETION_SCENARIO: Scenario = Scenario::new(
    "shell_tab_completion",
    "Complete a nested workspace path one component per Tab inside the inline shell composer",
    SIZE,
    SHELL_TAB_COMPLETION_STEPS,
    /* smoke */ false,
)
.with_setup(setup_shell_completion);
