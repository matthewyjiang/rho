//! Presentation tests for `rho plugins list` and `inspect`.

use crate::plugins::{
    PluginLoadReport, PluginOrigin, PluginReportEntry, PluginScope, PluginStatus,
};

use super::{inspect_entry, print_inspect, PluginsListDocument};

fn sample_entry(
    name: &str,
    scope: PluginScope,
    status: PluginStatus,
    root: &str,
) -> PluginReportEntry {
    PluginReportEntry {
        name: name.into(),
        version: None,
        description: Some("demo plugin".into()),
        root: root.into(),
        scope,
        origin: PluginOrigin::Install,
        enabled: status != PluginStatus::Disabled,
        status,
        problems: Vec::new(),
        skill_count: 1,
        mcp_server_count: 0,
        skill_names: vec!["hello".into()],
        mcp_server_names: Vec::new(),
    }
}

fn sample_report() -> PluginLoadReport {
    PluginLoadReport {
        plugins: vec![sample_entry(
            "demo",
            PluginScope::User,
            PluginStatus::Loaded,
            "/home/rho/.agents/plugins/demo",
        )],
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

// Covers: inspect prefers the active package when a same-named untrusted
// project copy is listed first, and still reaches an untrusted-only name.
// Owner: plugins CLI presentation.
#[test]
fn inspect_prefers_active_duplicate_and_keeps_untrusted_reachable() {
    let report = PluginLoadReport {
        plugins: vec![
            sample_entry(
                "dup",
                PluginScope::Project,
                PluginStatus::Untrusted,
                "/repo/.agents/plugins/dup",
            ),
            sample_entry(
                "dup",
                PluginScope::User,
                PluginStatus::Loaded,
                "/home/rho/.agents/plugins/dup",
            ),
            sample_entry(
                "risky",
                PluginScope::Project,
                PluginStatus::Untrusted,
                "/repo/.agents/plugins/risky",
            ),
        ],
    };

    let active = inspect_entry(&report, "dup").unwrap();
    assert_eq!(active.status, PluginStatus::Loaded);
    assert_eq!(active.scope, PluginScope::User);

    let untrusted = inspect_entry(&report, "risky").unwrap();
    assert_eq!(untrusted.status, PluginStatus::Untrusted);
    assert_eq!(untrusted.scope, PluginScope::Project);
}
