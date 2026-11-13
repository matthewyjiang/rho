//! A home whose `config.toml` rho 2.x wrote starts on 3.0: the display,
//! update-check, and keybinding keys move to the TUI's own files and still
//! apply, and the original is kept beside the rewritten file.

use std::path::Path;

use anyhow::{ensure, Context, Result};

use super::{DEFAULT_SIZE, OPENAI_KEY_ENV, SETTLE, STARTUP};
use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

/// The groups 2.x saved that 3.0 reads elsewhere, around agent keys that
/// stay.
const CONFIG_2X: &str = r#"[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

[display]
show_reasoning_output = true
zen_mode = false
theme = "nord"
show_header_hints = false

[web_search]
mode = "off"

[behavior]
check_for_updates = false
permission_mode = "bypass"
credential_store = "file"

[keybindings]
editing_mode = "default"
cycle_permission_mode = "alt+n"
undo = "ctrl+z"
queue_prompt = "alt+enter"
"#;

/// A ready-session hint line; `display.show_header_hints = false` hides it.
const HINT_TEXT: &str = "Show available commands";

fn write_2x_home(home: &IsolatedHome) -> Result<()> {
    std::fs::write(&home.config_path, CONFIG_2X)?;
    // A 2.x home has neither 3.0 file.
    std::fs::remove_file(&home.client_settings_path)?;
    Ok(())
}

fn assert_hints_hidden(harness: &mut PtyHarness) -> Result<()> {
    // The brand line proves the header rendered, so absence means hidden.
    harness.wait_for_text("rho  v", SETTLE)?;
    harness.wait_for_text_gone(HINT_TEXT, SETTLE)
}

fn assert_original_kept(harness: &mut PtyHarness) -> Result<()> {
    // `IsolatedHome` lays the rho home beside the workspace.
    let rho_home = harness
        .working_directory()
        .and_then(Path::parent)
        .context("missing sandbox root")?
        .join("home")
        .join(".rho");
    let backup = std::fs::read_to_string(rho_home.join("config.toml.2x.bak"))?;
    ensure!(
        backup == CONFIG_2X,
        "the backup is not the 2.x file: {backup}"
    );
    let tui = std::fs::read_to_string(rho_home.join("tui.toml"))?;
    ensure!(
        tui.contains("theme = \"nord\""),
        "the theme did not move to tui.toml: {tui}"
    );
    Ok(())
}

const STEPS: &[Step] = &[
    Step::WaitText {
        text: "Bypass ·",
        timeout: STARTUP,
    },
    Step::Custom(assert_hints_hidden),
    Step::Key(Key::Alt('n')),
    Step::WaitText {
        text: "Plan ·",
        timeout: SETTLE,
    },
    Step::ExitCommand,
    Step::Custom(assert_original_kept),
];

// Covers: a 2.x config.toml does not stop startup, and the display and
// keybinding keys it held still apply from their 3.0 files.
// Owner: interactive startup
pub(super) const SCENARIO: Scenario = Scenario::new(
    "config_2x_upgrade",
    "Start on a 2.x config.toml and keep its display and keybinding settings",
    DEFAULT_SIZE,
    STEPS,
    /*smoke*/ false,
)
.with_setup(write_2x_home)
.with_env(OPENAI_KEY_ENV);
