//! `/mcp` shows configured servers and session load status.

use std::{fs, time::Duration};

use anyhow::{Context, Result};

use crate::{
    env::IsolatedHome,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

pub(super) const MCP_INVENTORY_SCENARIO: Scenario = Scenario::new(
    "mcp_inventory",
    "Show configured MCP servers and a failed sibling in /mcp",
    PtySize {
        rows: 30,
        cols: 120,
    },
    MCP_INVENTORY_STEPS,
    /* smoke */ false,
)
.with_setup(setup_mcp);

pub(super) const MCP_CONNECTING_SCENARIO: Scenario = Scenario::new(
    "mcp_connecting",
    "Paint while MCP connects and still open /mcp",
    PtySize {
        rows: 30,
        cols: 120,
    },
    MCP_CONNECTING_STEPS,
    /* smoke */ false,
)
.with_setup(setup_slow_mcp);

pub(super) const MCP_HOLD_TAKE_BACK_SCENARIO: Scenario = Scenario::new(
    "mcp_hold_take_back",
    "Esc returns a turn held during MCP connect to the composer",
    PtySize {
        rows: 30,
        cols: 120,
    },
    MCP_HOLD_TAKE_BACK_STEPS,
    /* smoke */ false,
)
.with_setup(setup_slow_mcp);

pub(super) const MCP_CONNECT_RELEASE_SCENARIO: Scenario = Scenario::new(
    "mcp_connect_release",
    "Start a turn held during MCP connect once the servers settle",
    PtySize {
        rows: 30,
        cols: 120,
    },
    MCP_CONNECT_RELEASE_STEPS,
    /* smoke */ false,
)
.with_setup(setup_settling_mcp);

const MCP_INVENTORY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_mcp"),
    Step::SubmitText("/mcp"),
    // Status row plus the two configured identities must reach the screen.
    Step::WaitText {
        text: "MCP servers",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "disabled-fs",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "broken",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "1 problem",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const MCP_CONNECTING_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    // Slash commands and pickers stay usable while the servers are still
    // connecting. Submitting a prompt in that window is `mcp_connect_release`.
    Step::Phase("open_mcp"),
    Step::SubmitText("/mcp"),
    Step::WaitText {
        text: "slow-stdio",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "connecting",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::CtrlCExit,
];

const MCP_HOLD_TAKE_BACK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("submit_during_connect"),
    Step::SubmitText("hold-turn-xyz"),
    Step::WaitText {
        text: "connecting MCP servers",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "HOLD",
        timeout: SETTLE,
    },
    Step::AssertText("hold-turn-xyz"),
    Step::Phase("take_back_with_esc"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "hold-turn-xyz",
        timeout: SETTLE,
    },
    Step::CtrlCExit,
];

const MCP_CONNECT_RELEASE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("submit_during_connect"),
    Step::SubmitText("fixture stream"),
    // No second Enter: the held turn must start on its own once the server
    // settles, and the model's reply is the proof it ran.
    Step::Phase("turn_runs_after_connect"),
    Step::WaitText {
        text: "assistant stream part one",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "part two",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

fn setup_mcp(home: &IsolatedHome) -> Result<()> {
    let existing = fs::read_to_string(&home.config_path).context("read isolated config")?;
    fs::write(
        &home.config_path,
        format!(
            r#"{existing}

[mcp.servers.disabled-fs]
enabled = false
transport = "stdio"
command = "false"

[mcp.servers.broken]
transport = "stdio"
command = "rho-mcp-command-that-does-not-exist"
"#
        ),
    )
    .context("append MCP servers to isolated config")?;
    Ok(())
}

/// A server that settles shortly after the first frame, so a turn submitted
/// during connect is released while the scenario is still running. It exits
/// non-zero: connect finishing is what matters here, not connect succeeding.
fn setup_settling_mcp(home: &IsolatedHome) -> Result<()> {
    let existing = fs::read_to_string(&home.config_path).context("read isolated config")?;
    fs::write(
        &home.config_path,
        format!(
            r#"{existing}

[mcp.servers.settling-stdio]
transport = "stdio"
command = "sh"
args = ["-c", "sleep 1; exit 1"]
"#
        ),
    )
    .context("append settling MCP server to isolated config")?;
    Ok(())
}

fn setup_slow_mcp(home: &IsolatedHome) -> Result<()> {
    let existing = fs::read_to_string(&home.config_path).context("read isolated config")?;
    fs::write(
        &home.config_path,
        format!(
            r#"{existing}

[mcp.servers.slow-stdio]
transport = "stdio"
command = "sleep"
args = ["120"]
"#
        ),
    )
    .context("append slow MCP server to isolated config")?;
    Ok(())
}
