use pretty_assertions::assert_eq;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

use super::*;

fn round_trips<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(
    value: T,
    wire: Value,
) {
    assert_eq!(serde_json::to_value(&value).unwrap(), wire);
    assert_eq!(serde_json::from_value::<T>(wire).unwrap(), value);
}

// Covers: a listed plugin travels with its source tagged by `type`, the git
// ref under `ref`, and a rejected package's error in its own field, absent
// optionals left out, so rho-ui reads one shape. Owner: protocol wire
// contract.
#[test]
fn a_plugin_list_round_trips_through_its_wire_form() {
    let list = PluginsListResponse::new(vec![
        Plugin::new(
            "review".into(),
            Some("1.2.0".into()),
            Some("Code review helpers".into()),
            PluginScope::User,
            "/home/u/.agents/plugins/review".into(),
            PluginSource::Git {
                url: "https://github.com/o/r".into(),
                git_ref: Some("main".into()),
                path: Some("plugins/review".into()),
                commit: "abc123".into(),
            },
            true,
            "/home/u/.rho/plugins.toml".into(),
            PluginStatus::Loaded,
            None,
            vec!["skipping skill `x`: no description".into()],
            vec!["review".into()],
            vec!["lint".into()],
            vec!["reviewer.md".into()],
        ),
        Plugin::new(
            "broken".into(),
            None,
            None,
            PluginScope::Project,
            "/w/app/.agents/plugins/broken".into(),
            PluginSource::Link {
                target: "/src/broken".into(),
            },
            true,
            "/w/app/.rho/plugins.toml".into(),
            PluginStatus::Rejected,
            Some("invalid manifest: missing field `name`".into()),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ),
    ]);
    let wire = json!({
        "plugins": [
            {
                "name": "review",
                "version": "1.2.0",
                "description": "Code review helpers",
                "scope": "user",
                "path": "/home/u/.agents/plugins/review",
                "source": {
                    "type": "git",
                    "url": "https://github.com/o/r",
                    "ref": "main",
                    "path": "plugins/review",
                    "commit": "abc123",
                },
                "enabled": true,
                "state_path": "/home/u/.rho/plugins.toml",
                "status": "loaded",
                "problems": ["skipping skill `x`: no description"],
                "skills": ["review"],
                "mcp_servers": ["lint"],
                "agent_files": ["reviewer.md"],
            },
            {
                "name": "broken",
                "scope": "project",
                "path": "/w/app/.agents/plugins/broken",
                "source": {"type": "link", "target": "/src/broken"},
                "enabled": true,
                "state_path": "/w/app/.rho/plugins.toml",
                "status": "rejected",
                "error": "invalid manifest: missing field `name`",
                "problems": [],
                "skills": [],
                "mcp_servers": [],
                "agent_files": [],
            },
        ],
    });
    round_trips(list, wire);
}

// Covers: each install source a client sends is tagged by `type`, and
// `force` is left out when false. Owner: protocol wire contract.
#[test]
fn install_params_tag_each_source_kind() {
    let cases = [
        (
            PluginInstallSource::Path {
                path: "/src/p".into(),
            },
            json!({"type": "path", "path": "/src/p"}),
        ),
        (
            PluginInstallSource::Link {
                path: "/src/p".into(),
            },
            json!({"type": "link", "path": "/src/p"}),
        ),
        (
            PluginInstallSource::Git {
                url: "https://github.com/o/r/tree/main/p".into(),
            },
            json!({"type": "git", "url": "https://github.com/o/r/tree/main/p"}),
        ),
    ];
    for (source, source_wire) in cases {
        round_trips(
            PluginsInstallParams::new(
                source,
                PluginScope::Project,
                false,
                Some("/w/app".into()),
                ClientEnvironment::default(),
            ),
            json!({"source": source_wire, "scope": "project", "workspace": "/w/app"}),
        );
    }
}

// Covers: an update report names each plugin with its outcome, a failure
// carrying the error code a lone update would have answered. Owner:
// protocol wire contract.
#[test]
fn an_update_report_round_trips_through_its_wire_form() {
    let target = |name: &str| PluginTarget::new(name.into(), PluginScope::User);
    let report = PluginsUpdateResponse::new(
        vec![
            PluginUpdate::new(
                target("a"),
                PluginUpdateOutcome::Updated {
                    previous: "111".into(),
                    current: "222".into(),
                },
            ),
            PluginUpdate::new(
                target("b"),
                PluginUpdateOutcome::UpToDate {
                    commit: "333".into(),
                },
            ),
            PluginUpdate::new(
                target("c"),
                PluginUpdateOutcome::Failed {
                    code: crate::ErrorCode::PluginFetchFailed.code(),
                    message: "git fetch failed".into(),
                },
            ),
        ],
        vec!["d".into()],
    );
    let wire = json!({
        "results": [
            {
                "plugin": {"name": "a", "scope": "user"},
                "outcome": {"type": "updated", "previous": "111", "current": "222"},
            },
            {
                "plugin": {"name": "b", "scope": "user"},
                "outcome": {"type": "up_to_date", "commit": "333"},
            },
            {
                "plugin": {"name": "c", "scope": "user"},
                "outcome": {"type": "failed", "code": -32028, "message": "git fetch failed"},
            },
        ],
        "untrusted": ["d"],
    });
    round_trips(report, wire);
}
