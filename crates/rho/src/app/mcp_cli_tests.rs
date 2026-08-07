use super::*;
use crate::tools::mcp::report::McpToolReport;

// Covers: show must fail closed when the identity is missing.
// Owner: pure unit
#[test]
fn show_missing_identity_lists_known_servers() {
    let report = McpSessionReport {
        mode: McpLoadMode::Native,
        servers: vec![McpServerReport::connected(
            "filesystem",
            McpTransportSummary::StreamableHttp {
                url: "https://example.com/mcp".into(),
            },
            vec![McpToolReport {
                remote_name: "read".into(),
                exported_name: "mcp__filesystem__read".into(),
            }],
            0,
            0,
        )],
    };
    // Behavior: unknown id fails closed; a known id succeeds.
    assert!(print_show(&report, "missing", false).is_err());
    assert!(print_show(&report, "filesystem", false).is_ok());
}
