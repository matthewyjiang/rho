//! A daemon session whose only tool sources are OpenAPI sources holds a
//! turn while they load, as one with MCP servers does.
//!
//! Each reuses an MCP startup scenario's steps: what a held turn shows, and
//! when it starts, must not depend on which kind of source still loads.

use std::{fs, net::TcpListener};

use anyhow::{Context, Result};

use super::mcp::{MCP_CONNECT_RELEASE_SCENARIO, MCP_HOLD_TAKE_BACK_SCENARIO};
use crate::{env::IsolatedHome, scenario::Scenario};

const DAEMON_ENV: &[(&str, &str)] = &[("RHO_TUI_DAEMON", "1")];

pub(super) const HOLD_TAKE_BACK: Scenario = Scenario {
    id: "daemon_openapi_hold_take_back",
    description: "Hold a turn while a daemon session's only tool source, an OpenAPI spec, loads",
    env: DAEMON_ENV,
    setup: Some(setup_stalled_openapi),
    ..MCP_HOLD_TAKE_BACK_SCENARIO
};

pub(super) const LOAD_RELEASE: Scenario = Scenario {
    id: "daemon_openapi_load_release",
    description: "Start a daemon turn held during an OpenAPI load once the load ends",
    env: DAEMON_ENV,
    setup: Some(setup_settling_openapi),
    ..MCP_CONNECT_RELEASE_SCENARIO
};

/// A source whose spec download outlasts the scenario.
fn setup_stalled_openapi(home: &IsolatedHome) -> Result<()> {
    append_source(home, "")
}

/// A source whose spec download gives up after a second, so a turn sent
/// during the load is released while the scenario still runs. It fails:
/// the load ending is what matters here, not the load succeeding.
fn setup_settling_openapi(home: &IsolatedHome) -> Result<()> {
    append_source(home, "timeout_secs = 1\n")
}

/// Adds an OpenAPI source, and no MCP server, whose spec no one serves: a
/// loopback listener takes the connection and never answers, so the
/// download ends only at the source's timeout. `keys` are further keys of
/// the source's table.
fn append_source(home: &IsolatedHome, keys: &str) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind the spec listener")?;
    let port = listener.local_addr()?.port();
    // Open for the rest of the process: a setup hook has no one to hand it
    // to, and closing it would refuse the download and end the load early.
    std::mem::forget(listener);
    let existing = fs::read_to_string(&home.config_path).context("read isolated config")?;
    fs::write(
        &home.config_path,
        format!(
            "{existing}\n[openapi.stalled]\nspec = \"http://127.0.0.1:{port}/spec.json\"\n{keys}"
        ),
    )
    .context("append OpenAPI source to isolated config")?;
    Ok(())
}
