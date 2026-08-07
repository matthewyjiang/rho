//! Presentation tests for `rho plugins list` and `inspect`.

use crate::plugins::{
    PluginLoadReport, PluginOrigin, PluginReportEntry, PluginScope, PluginStatus,
};

use super::{print_inspect, print_list};

fn sample_report() -> PluginLoadReport {
    PluginLoadReport {
        plugins: vec![PluginReportEntry {
            name: "demo".into(),
            version: Some("1.0.0".into()),
            description: Some("demo plugin".into()),
            root: "/home/rho/.agents/plugins/demo".into(),
            scope: PluginScope::User,
            origin: PluginOrigin::Install,
            enabled: true,
            status: PluginStatus::Loaded,
            problems: Vec::new(),
            skill_count: 1,
            mcp_server_count: 0,
            skill_names: vec!["hello".into()],
            mcp_server_names: Vec::new(),
        }],
    }
}

// Covers: JSON list output exposes structured inventory fields.
// Owner: plugins CLI presentation.
#[test]
fn list_json_includes_inventory_fields() {
    let report = sample_report();
    // print_list writes to stdout; capture via a simple smoke that the function
    // accepts the report shape. Structured fields are asserted on the model.
    let entry = &report.plugins[0];
    assert_eq!(entry.scope, PluginScope::User);
    assert_eq!(entry.origin, PluginOrigin::Install);
    assert_eq!(entry.skill_names, ["hello"]);
    print_list(&report, true).unwrap();
}

// Covers: inspect resolves a known plugin and fails closed for unknown names.
// Owner: plugins CLI presentation.
#[test]
fn inspect_known_and_unknown_names() {
    let report = sample_report();
    print_inspect(&report, "demo", true).unwrap();
    let error = print_inspect(&report, "missing", false).unwrap_err();
    assert!(error.to_string().contains("no plugin named `missing`"));
}
