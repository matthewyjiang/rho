use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::super::{catalog::ProjectTrust, HookPipeline};
use super::*;

fn project_with_hook() -> TempDir {
    let project = TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".rho/hooks")).unwrap();
    std::fs::write(project.path().join(".rho/hooks/fmt"), "#!/bin/sh\n").unwrap();
    std::fs::write(
        project.path().join(".rho/hooks.toml"),
        "version = 1\n\n[[hook]]\nid = \"fmt\"\non = \"after_tool_use\"\ntools = [\"edit\"]\ncommand = [\"./.rho/hooks/fmt\"]\ntimeout = \"5s\"\n",
    )
    .unwrap();
    project
}

#[test]
fn a_session_without_hooks_reports_itself_disabled() {
    assert_eq!(
        serde_json::to_value(HookReport::disabled()).unwrap(),
        json!({
            "enabled": false,
            "files": [],
            "hooks": [],
            "recent_activity": [],
        })
    );
}

#[tokio::test]
async fn an_untrusted_project_file_is_named_in_the_report() {
    let project = project_with_hook();
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("hooks.toml"),
        "version = 1\n\n[[hook]]\nid = \"log\"\non = \"run_completed\"\ncommand = [\"logger\"]\ntimeout = \"1s\"\n",
    )
    .unwrap();
    let catalog = HookCatalog::discover(
        Some(home.path()),
        Some(project.path()),
        ProjectTrust::Untrusted,
    )
    .unwrap();
    let runtime = HookPipeline::start(catalog, rho_sdk::CancellationToken::new()).unwrap();

    let report = HookInspector::new(&runtime).report();

    assert_eq!(
        report.skipped_untrusted,
        Some(crate::paths::display(
            &project.path().join(".rho/hooks.toml")
        ))
    );
}

#[test]
fn an_activity_view_carries_the_denial_reason() {
    let activity = HookActivity {
        hook_id: "user:no-force-push".into(),
        event: "before_tool_use",
        outcome: crate::hooks::activity::HookOutcome::Denied {
            reason: "force push".into(),
        },
        duration: Some(std::time::Duration::from_millis(7)),
        truncated: false,
    };

    assert_eq!(
        serde_json::to_value(HookActivityView::from(&activity)).unwrap(),
        json!({
            "hook": "user:no-force-push",
            "event": "before_tool_use",
            "outcome": "denied",
            "duration_ms": 7,
            "truncated": false,
            "detail": "force push",
        })
    );
}
