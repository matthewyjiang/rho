use super::*;
use crate::tools::mcp::report::{ConnectedServerReport, McpLiveServerState, McpToolReport};

// Covers: show must fail closed when the identity is missing.
// Owner: pure unit
#[test]
fn show_missing_identity_lists_known_servers() {
    let report = McpSessionReport {
        mode: McpLoadMode::Native,
        servers: vec![McpServerReport::connected(ConnectedServerReport {
            identity: "filesystem".into(),
            transport: McpTransportSummary::StreamableHttp {
                url: "https://example.com/mcp".into(),
            },
            tools: vec![McpToolReport {
                remote_name: "read".into(),
                exported_name: "mcp__filesystem__read".into(),
            }],
            instructions: None,
            live: McpLiveServerState::default(),
            filtered_out_count: 0,
            collision_skipped_count: 0,
        })],
    };
    // Behavior: unknown id fails closed; a known id succeeds.
    assert!(print_show(&report, "missing", false).is_err());
    assert!(print_show(&report, "filesystem", false).is_ok());
}
