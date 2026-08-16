use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::{McpLoadMode, McpServerStatus, McpSessionReport};
use crate::tools::mcp::config::{
    McpConfig, McpSamplingPolicy, McpServerConfig, McpToolFilter, McpTransport,
};

fn stdio_server(enabled: bool) -> McpServerConfig {
    McpServerConfig {
        enabled,
        tools: McpToolFilter::default(),
        log_level: None,
        sampling: McpSamplingPolicy::Deny,
        transport: McpTransport::Stdio {
            command: "sleep".into(),
            args: vec!["30".into()],
            cwd: None,
            env: BTreeMap::new(),
            env_from_env: BTreeMap::new(),
        },
        filesystem: None,
    }
}

// Covers: a deferred connect inventory must show enabled servers as connecting, not failed or not-loaded.
// Owner: MCP inventory
#[test]
fn connecting_inventory_is_in_flight_not_a_problem() {
    let config = McpConfig {
        servers: BTreeMap::from([
            ("slow".into(), stdio_server(true)),
            ("off".into(), stdio_server(false)),
        ]),
        invalid_servers: Vec::new(),
    };
    let report = McpSessionReport::from_config_connecting(&config);
    let summary = report.summary();
    assert_eq!(summary.mode, McpLoadMode::Native);
    assert_eq!(summary.connecting, 1);
    assert_eq!(summary.connected, 0);
    assert_eq!(summary.problems, 0);
    assert_eq!(
        report.find("slow").map(|server| server.status()),
        Some(McpServerStatus::Connecting)
    );
    assert_eq!(
        report.find("off").map(|server| server.status()),
        Some(McpServerStatus::Disabled)
    );
    assert!(McpServerStatus::Connecting.is_healthy());
}
