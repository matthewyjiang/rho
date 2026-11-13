use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use std::path::Path;

use rho_protocol::{SessionMcpMode, SessionMcpStatus};

use super::{
    McpLoadMode, McpServerReport, McpServerStatus, McpSessionReport, McpSessionSummary,
    McpTransportSummary,
};
use crate::tools::mcp::config::{
    McpConfig, McpSamplingPolicy, McpServerConfig, McpToolFilter, McpTransport,
};
use crate::{app::session_mcp::session_mcp_inventory, tools::mcp::McpCatalog};

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
}

// Covers: a daemon session's inventory summarizes as the session's own
// report does, so `/doctor` reports the same MCP row on either host. A status
// or mode from a newer daemon must not read as healthy.
// Owner: MCP inventory
#[test]
fn inventory_summary_matches_the_session_report() {
    let report = McpSessionReport {
        mode: McpLoadMode::Native,
        servers: vec![
            McpServerReport::connecting("slow", &stdio_server(true)),
            McpServerReport::disabled("off", &stdio_server(false)),
            McpServerReport::invalid("bad", "missing command"),
            McpServerReport::failed(
                "down",
                McpTransportSummary::StreamableHttp {
                    url: "https://example.com/mcp".into(),
                },
                "connection refused",
            ),
        ],
    };
    let summary = report.summary();
    let mut inventory =
        session_mcp_inventory(&report, &McpCatalog::default(), Path::new("config.toml"));
    assert_eq!(McpSessionSummary::of_inventory(&inventory), Some(summary));

    inventory.servers[0].status = SessionMcpStatus::Unknown;
    assert_eq!(
        McpSessionSummary::of_inventory(&inventory),
        Some(McpSessionSummary {
            connecting: 0,
            problems: summary.problems + 1,
            ..summary
        })
    );

    inventory.mode = SessionMcpMode::Unknown;
    assert_eq!(McpSessionSummary::of_inventory(&inventory), None);
}
