//! `/mcp` shows configured servers and session load status.

use std::fs;

use anyhow::{Context, Result};

use crate::{
    env::IsolatedHome,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

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
    "Paint while MCP connects, keep composer text on Enter, and still open /mcp",
    PtySize {
        rows: 30,
        cols: 120,
    },
    MCP_CONNECTING_STEPS,
    /* smoke */ false,
)
.with_setup(setup_slow_mcp);

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
    Step::Phase("submit_blocked"),
    Step::TypeText("hold-turn-xyz"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "turn-xyz",
        timeout: SETTLE,
    },
    Step::Phase("open_mcp"),
    Step::Key(Key::Ctrl('c')),
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
