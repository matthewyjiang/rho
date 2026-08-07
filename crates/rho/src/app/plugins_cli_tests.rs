//! Presentation tests for `rho plugins list` and `inspect`.

use crate::plugins::{
    PluginLoadReport, PluginOrigin, PluginReportEntry, PluginScope, PluginStatus,
};

use super::{print_inspect, PluginsListDocument};

fn sample_report() -> PluginLoadReport {
    PluginLoadReport {
        plugins: vec![PluginReportEntry {
            name: "demo".into(),
            version: None,
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

// Covers: JSON list output exposes structured inventory fields and omits None
// optional fields via skip_serializing_if.
// Owner: plugins CLI presentation.
#[test]
fn list_json_includes_inventory_fields() {
    let report = sample_report();
    let document = PluginsListDocument {
        plugins: &report.plugins,
    };
    let value = serde_json::to_value(&document).unwrap();
    let expected = serde_json::json!({
        "plugins": [{
            "name": "demo",
            "description": "demo plugin",
            "root": "/home/rho/.agents/plugins/demo",
            "scope": "user",
            "origin": "install",
            "enabled": true,
            "status": "loaded",
            "problems": [],
            "skill_count": 1,
            "mcp_server_count": 0,
            "skill_names": ["hello"],
            "mcp_server_names": []
        }]
    });
    pretty_assertions::assert_eq!(value, expected);
}

// Covers: inspect resolves a known plugin and fails closed for unknown names.
// Owner: plugins CLI presentation.
#[test]
fn inspect_known_and_unknown_names() {
    let report = sample_report();
    print_inspect(&report, "demo", true).unwrap();
    assert!(print_inspect(&report, "missing", false).is_err());
}
