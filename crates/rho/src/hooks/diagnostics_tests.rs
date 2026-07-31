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
        "version = 1\n\n[[hook]]\nid = \"fmt\"\non = \"after_tool_use\"\ntools = [\"edit_file\"]\ncommand = [\"./.rho/hooks/fmt\"]\ntimeout = \"5s\"\n",
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

#[test]
fn the_contract_view_renders_argv_cwd_timeout_and_environment() {
    let project = project_with_hook();
    let catalog = HookCatalog::discover(None, Some(project.path()), ProjectTrust::Trusted).unwrap();

    let views = contract_views(&catalog);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].id, "project:fmt");
    assert_eq!(views[0].event, "after_tool_use");
    assert_eq!(views[0].tools, "edit_file");
    assert_eq!(views[0].timeout, "5s");
    assert_eq!(
        views[0].command,
        vec![crate::paths::display(
            &project.path().join(".rho/hooks/fmt")
        )]
    );
    assert!(views[0].environment.contains(&"PATH".to_owned()));
}

#[tokio::test]
async fn a_report_names_the_files_it_loaded_and_the_untrusted_file_it_skipped() {
    let project = project_with_hook();
    let untrusted =
        HookCatalog::discover(None, Some(project.path()), ProjectTrust::Untrusted).unwrap();
    assert!(untrusted.is_empty());

    let catalog = HookCatalog::discover(None, Some(project.path()), ProjectTrust::Trusted).unwrap();
    let runtime = HookPipeline::start(catalog, rho_sdk::CancellationToken::new())
        .expect("the fixture configures one hook");

    let report = HookInspector::new(&runtime).report();

    assert!(report.enabled);
    assert_eq!(
        report.files,
        vec![crate::paths::display(
            &project.path().join(".rho/hooks.toml")
        )]
    );
    assert_eq!(report.skipped_untrusted, None);
    assert_eq!(report.hooks.len(), 1);
    assert!(report.recent_activity.is_empty());
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

#[test]
fn a_successful_activity_view_omits_the_absent_fields() {
    let activity = HookActivity {
        hook_id: "user:log".into(),
        event: "after_tool_use",
        outcome: crate::hooks::activity::HookOutcome::Observed,
        duration: None,
        truncated: false,
    };

    assert_eq!(
        serde_json::to_value(HookActivityView::from(&activity)).unwrap(),
        json!({
            "hook": "user:log",
            "event": "after_tool_use",
            "outcome": "observed",
            "truncated": false,
        })
    );
}

#[test]
fn a_session_with_no_hooks_files_renders_a_plain_notice() {
    assert_eq!(
        HookReport::disabled().render(),
        "no hooks files found\nno hooks are configured"
    );
}

#[test]
fn a_loaded_hook_renders_its_full_spawn_contract() {
    let report = HookReport {
        enabled: true,
        files: vec!["/home/rho/.rho/hooks.toml".into()],
        skipped_untrusted: None,
        skipped_untrusted_error: None,
        hooks: vec![HookContractView {
            active: true,
            id: "user:deny-force-push".into(),
            event: "before_tool_use".into(),
            tools: "bash".into(),
            command: vec!["/home/rho/bin/deny".into(), "--strict".into()],
            working_directory: "/work".into(),
            timeout: "2s".into(),
            environment: vec!["PATH".into(), "RHO_IN_HOOK".into()],
        }],
        recent_activity: Vec::new(),
    };

    assert_eq!(
        report.render(),
        "hooks files: /home/rho/.rho/hooks.toml\n\
         user:deny-force-push on before_tool_use (tools: bash)\n  \
         argv: /home/rho/bin/deny --strict\n  \
         cwd: /work\n  \
         timeout: 2s\n  \
         env: PATH, RHO_IN_HOOK"
    );
}

#[test]
fn an_untrusted_project_file_is_rendered_with_the_way_to_trust_it() {
    let mut report = HookReport::disabled();
    report.skipped_untrusted = Some("/work/.rho/hooks.toml".into());

    let rendered = report.render();

    assert!(rendered.contains("ignoring /work/.rho/hooks.toml"));
    assert!(rendered.contains("RHO_TRUST_PROJECT_HOOKS=1"));
}
