//! Installation consent stays separate from desktop authority. All executables are fixtures.

use super::{computer::setup_driver, SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};
use anyhow::Result;
use std::{fs, os::unix::fs::PermissionsExt};

pub(super) const COMPUTER_SETUP_SCENARIO: Scenario = Scenario::new(
    "computer_setup",
    "Missing driver installation requires separate visible consent before desktop connection",
    PtySize {
        rows: 40,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "Install Cua Driver?",
            timeout: SETTLE,
        },
        Step::WaitText {
            text: "telemetry is enabled by default",
            timeout: SETTLE,
        },
        Step::Key(Key::Enter),
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "Install Cua Driver?",
            timeout: SETTLE,
        },
        Step::Resize { rows: 8, cols: 60 },
        Step::WaitTextGone {
            text: "telemetry is enabled by default",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('i')),
        Step::WaitText {
            text: "enlarge terminal",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::Resize {
            rows: 40,
            cols: 120,
        },
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "Install Cua Driver?",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('i')),
        Step::WaitText {
            text: "Grant desktop access?",
            timeout: STARTUP,
        },
        // Installing alone must not register the computer tool.
        Step::Key(Key::Enter),
        Step::SubmitText("fixture tool available computer"),
        Step::WaitText {
            text: "tool available computer: false",
            timeout: STREAM,
        },
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "Grant desktop access?",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('g')),
        Step::WaitText {
            text: "driver handshake verified",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture tool available computer"),
        Step::WaitText {
            text: "tool available computer: true",
            timeout: STREAM,
        },
        Step::SubmitText("/computer off"),
        Step::WaitText {
            text: "computer use off; access revoked",
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_installer)
.with_env(&[("PATH", ".rho-fixture-bin")]);

pub(super) const COMPUTER_SETUP_CANCEL_SCENARIO: Scenario = Scenario::new(
    "computer_setup_cancel",
    "Cancelling a pending installation keeps the TUI usable and desktop authority off",
    PtySize {
        rows: 40,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "Install Cua Driver?",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('i')),
        Step::WaitText {
            text: "installing Cua Driver; desktop access remains off",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "Cua Driver installation pending",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('r')),
        Step::Key(Key::Esc),
        Step::WaitText {
            text: "Cua Driver installation cancelled",
            timeout: SETTLE,
        },
        Step::SubmitText("fixture tool available computer"),
        Step::WaitText {
            text: "tool available computer: false",
            timeout: STREAM,
        },
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "Install Cua Driver?",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_blocked_installer)
.with_env(&[("PATH", ".rho-fixture-bin")]);

fn setup_blocked_installer(home: &IsolatedHome) -> Result<()> {
    setup_installer(home)?;
    fs::write(
        home.home.join("fixture-installer.sh"),
        "exec /usr/bin/python3 -c 'import signal; signal.pause()'\n",
    )?;
    Ok(())
}

fn setup_installer(home: &IsolatedHome) -> Result<()> {
    setup_driver(home)?;
    fs::rename(
        home.home.join(".local/bin/cua-driver"),
        home.home.join("fixture-driver"),
    )?;
    let bin = home.home.join(".rho-fixture-bin");
    fs::create_dir_all(&bin)?;
    let curl = bin.join("curl");
    fs::write(
        &curl,
        "#!/bin/sh\nexec /bin/cat \"$HOME/fixture-installer.sh\"\n",
    )?;
    fs::set_permissions(curl, fs::Permissions::from_mode(0o700))?;
    fs::write(
        home.home.join("fixture-installer.sh"),
        r#"set -eu
[ "$1" = --no-modify-path ]
[ "$2" = --bin-dir ]
[ "$3" = "$HOME/.local/bin" ]
# Fail if a repeated setup attempts to reinstall an existing driver.
[ ! -e "$HOME/.local/bin/cua-driver" ]
/bin/cp "$HOME/fixture-driver" "$HOME/.local/bin/cua-driver"
"#,
    )?;
    Ok(())
}
