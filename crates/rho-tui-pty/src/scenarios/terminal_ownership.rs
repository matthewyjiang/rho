//! Shell children must not take the terminal away from the TUI.

use crate::{
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{STARTUP, STREAM};

// Ignoring SIGTTOU lets a background process move the terminal's foreground
// group to itself, as job-control programs do. If it succeeds, the TUI's next
// read stops it with SIGTTIN. Without a controlling terminal, `/dev/tty`
// cannot be opened and the attempt fails.
const TERMINAL_THIEF: &str = "!!python3 -c \"import os,signal;signal.signal(signal.SIGTTOU,signal.SIG_IGN);os.tcsetpgrp(os.open('/dev/tty',os.O_RDWR),os.getpgrp())\" 2>/dev/null; printf 'thief-%s finished' run";

// Covers: a shell command that grabs the terminal foreground suspends the TUI.
// Owner: interactive lifecycle and child process isolation.
pub(super) const SHELL_KEEPS_TERMINAL_SCENARIO: Scenario = Scenario::new(
    "shell_keeps_terminal",
    "A shell child that tries to take the terminal leaves the TUI responsive",
    PtySize {
        rows: 28,
        cols: 100,
    },
    &[
        Step::Phase("startup"),
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("thief"),
        Step::SubmitText(TERMINAL_THIEF),
        Step::WaitText {
            text: "thief-run finished",
            timeout: STREAM,
        },
        Step::Phase("still_responsive"),
        Step::SubmitText("after terminal thief"),
        Step::WaitText {
            text: "fixture response: after terminal thief",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
);
