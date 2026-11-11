//! The MCP startup scenarios again, with the daemon holding the session.
//!
//! Each reuses its in-process scenario's steps and setup unchanged: what a
//! connecting or failed server shows, and how a turn waits for connect, must
//! not depend on which side runs the session.

use super::mcp::{
    MCP_CONNECTING_SCENARIO, MCP_CONNECT_RELEASE_SCENARIO, MCP_HOLD_TAKE_BACK_SCENARIO,
};
use crate::scenario::Scenario;

const DAEMON_ENV: &[(&str, &str)] = &[("RHO_TUI_DAEMON", "1")];

pub(super) const CONNECTING: Scenario = Scenario {
    id: "daemon_mcp_connecting",
    description: "Paint while a daemon session's MCP connects and still open /mcp",
    env: DAEMON_ENV,
    ..MCP_CONNECTING_SCENARIO
};

pub(super) const HOLD_TAKE_BACK: Scenario = Scenario {
    id: "daemon_mcp_hold_take_back",
    description: "Esc returns a turn held during a daemon session's MCP connect to the composer",
    env: DAEMON_ENV,
    ..MCP_HOLD_TAKE_BACK_SCENARIO
};

pub(super) const CONNECT_RELEASE: Scenario = Scenario {
    id: "daemon_mcp_connect_release",
    description: "Start a daemon turn held during MCP connect once the servers settle",
    env: DAEMON_ENV,
    ..MCP_CONNECT_RELEASE_SCENARIO
};
