//! Driver updates: checks never run an untrusted driver implicitly, and installs
//! need separate consent and leave desktop access off. All executables are fixtures.

use super::{SETTLE, STARTUP};
use crate::{
    env::IsolatedHome,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};
use anyhow::Result;
use std::{fs, os::unix::fs::PermissionsExt};

pub(super) const COMPUTER_UPDATE_SCENARIO: Scenario = Scenario::new(
    "computer_update",
    "Driver update check and pinned install require explicit requests and keep access off",
    PtySize {
        rows: 40,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        // Access is off and unsaved, so opening status must not run the driver:
        // an auto-check would render "checking…" on the first frame instead.
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "Updates: not checked",
            timeout: SETTLE,
        },
        // `u` only checks; install consent belongs to /computer update.
        Step::Key(Key::Char('u')),
        Step::WaitText {
            text: "0.31.0 available · /computer update installs it",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::SubmitText("/computer update"),
        Step::WaitText {
            text: "Update Cua Driver 0.28.2 to 0.31.0?",
            timeout: SETTLE,
        },
        // Cancel is the default; the fixture installer fails if it ever runs twice.
        Step::Key(Key::Enter),
        Step::SubmitText("/computer update"),
        Step::WaitText {
            text: "Update Cua Driver 0.28.2 to 0.31.0?",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('u')),
        Step::WaitText {
            text: "Cua Driver updated to 0.31.0; desktop access is still off",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "Version 0.31.0 · up to date",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_updatable_driver)
.with_env(&[("PATH", ".rho-fixture-bin")]);

pub(super) const COMPUTER_UPDATE_PLAN_SCENARIO: Scenario = Scenario::new(
    "computer_update_plan",
    "Plan mode may check for driver updates but never offers to install one",
    PtySize {
        rows: 40,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer update"),
        Step::WaitText {
            text: "0.31.0 available",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::SubmitText("/computer update"),
        Step::WaitText {
            text: "could not update Cua Driver to 0.31.0: finish the current turn and leave plan mode first",
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_updatable_driver)
.with_env(&[("PATH", ".rho-fixture-bin")])
.with_args(&["--permission-mode", "plan"]);

fn setup_updatable_driver(home: &IsolatedHome) -> Result<()> {
    let local_bin = home.home.join(".local/bin");
    fs::create_dir_all(&local_bin)?;
    fs::write(home.home.join("driver-version"), "0.28.2")?;
    // Only maintenance subcommands are implemented; `mcp` (desktop) fails.
    let driver = local_bin.join("cua-driver");
    fs::write(
        &driver,
        r#"#!/bin/sh
set -eu
[ "$CUA_DRIVER_RS_TELEMETRY_ENABLED" = false ]
current=$(/bin/cat "$HOME/driver-version")
# Updating must never connect; a desktop attempt poisons later checks.
[ ! -e "$HOME/mcp-ran" ] || { echo '{"bad":"mcp ran"}'; exit 0; }
case "$*" in
mcp) printf ran > "$HOME/mcp-ran"; exit 64 ;;
--version) echo "cua-driver $current" ;;
# Updates must leave the saved telemetry preference alone.
"telemetry disable") exit 65 ;;
"check-update --json")
    if [ "$current" = 0.31.0 ]; then available=false; else available=true; fi
    printf '{"current_version":"%s","latest_version":"0.31.0","update_available":%s,"error":null,"release_notes_url":null}\n' "$current" "$available" ;;
*) exit 64 ;;
esac
"#,
    )?;
    fs::set_permissions(driver, fs::Permissions::from_mode(0o755))?;
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
[ "$CUA_DRIVER_RS_TELEMETRY_ENABLED" = false ]
[ "$CUA_DRIVER_RS_VERSION" = 0.31.0 ]
[ "$1" = --no-modify-path ]
# Fail if a cancelled confirmation still ran the installer.
[ ! -e "$HOME/installer-ran" ]
printf ran > "$HOME/installer-ran"
printf 0.31.0 > "$HOME/driver-version"
"#,
    )?;
    Ok(())
}
