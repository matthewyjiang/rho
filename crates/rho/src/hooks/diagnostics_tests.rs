use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::super::{catalog::ProjectTrust, HookPipeline};
use super::*;
use crate::session_environment::SessionEnvironment;

fn project_with_hook() -> TempDir {
    let project = TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".rho/hooks")).unwrap();
    std::fs::write(project.path().join(".rho/hooks/fmt"), "#!/bin/sh\n").unwrap();
    std::fs::write(
        project.path().join(".rho/hooks.toml"),
        "version = 1\n\n[[hook]]\nid = \"fmt\"\non = \"after_tool_use\"\ntools = [\"str_replace\", \"read_*\"]\ncommand = [\"./.rho/hooks/fmt\"]\ntimeout = \"5s\"\n",
    )
    .unwrap();
    project
}

#[test]
fn a_session_without_hooks_reports_itself_disabled() {
    assert_eq!(
        serde_json::to_value(HookSnapshot::disabled().report()).unwrap(),
        json!({
            "enabled": false,
            "files": [],
            "hooks": [],
            "recent_activity": [],
        })
    );
}

// Covers: the typed hooks report a daemon session answers names the
// untrusted project file and tells its hooks from the active ones, with each
// hook's event, tool selection, timeout and edit-tool warning as data.
// Owner: hook diagnostics.
#[tokio::test]
async fn the_session_report_types_each_hook_and_the_untrusted_file() {
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
    let runtime = HookPipeline::start(
        catalog,
        SessionEnvironment::default(),
        rho_sdk::CancellationToken::new(),
    )
    .unwrap();

    let report = HookInspector::new(&runtime).snapshot().session_report();

    let hooks: Vec<_> = report
        .hooks
        .iter()
        .map(|hook| {
            (
                hook.id.as_str(),
                hook.status,
                hook.event,
                hook.tools.clone(),
                hook.timeout_ms,
                hook.warnings.clone(),
            )
        })
        .collect();
    assert_eq!(
        (
            report.enabled,
            report.files,
            report.skipped_untrusted,
            hooks
        ),
        (
            true,
            vec![home.path().join("hooks.toml")],
            Some(SkippedHooksFile {
                path: project.path().join(".rho/hooks.toml"),
                error: None,
            }),
            vec![
                (
                    "user:log",
                    HookStatus::Active,
                    rho_protocol::HookEvent::RunCompleted,
                    HookTools::All,
                    1_000,
                    vec![],
                ),
                (
                    "project:fmt",
                    HookStatus::Untrusted,
                    rho_protocol::HookEvent::AfterToolUse,
                    HookTools::Matching {
                        patterns: vec!["str_replace".into(), "read_*".into()],
                    },
                    5_000,
                    vec![HookWarning::MissingEditTools {
                        tools: vec!["apply_patch".into()],
                    }],
                ),
            ],
        )
    );
}

// Covers: a hooks report checks the files against what the session runs
// without reloading: an edited hook is pending beside its stale running
// self, and a hook of a kind the session started without (no gate or no
// observer, or no hook runtime at all) needs the session opened again,
// since a reload cannot add it and is refused, edits beside it included.
// Owner: hook diagnostics.
#[tokio::test]
async fn a_report_tells_which_hooks_a_reload_or_only_a_reopen_loads() {
    const LOG: &str = "[[hook]]\nid = \"log\"\non = \"run_completed\"\ncommand = [\"logger\"]\ntimeout = \"1s\"\n";
    const LOG_EDITED: &str = "[[hook]]\nid = \"log\"\non = \"run_completed\"\ncommand = [\"logger\"]\ntimeout = \"2s\"\n";
    const GUARD: &str = "[[hook]]\nid = \"guard\"\non = \"before_tool_use\"\ncommand = [\"guard\"]\ntimeout = \"1s\"\n";
    let home = TempDir::new().unwrap();
    let catalog = |hooks: &[&str]| {
        std::fs::write(
            home.path().join("hooks.toml"),
            format!("version = 1\n\n{}", hooks.join("\n")),
        )
        .unwrap();
        HookCatalog::discover(Some(home.path()), None, ProjectTrust::Untrusted)
    };
    // The hooks the session started with (`None`: no hook runtime), the
    // files now, and each listed hook's id and status.
    type Case<'a> = (
        Option<&'a [&'a str]>,
        &'a [&'a str],
        Vec<(&'a str, HookStatus)>,
    );
    let cases: [Case; 4] = [
        (
            Some(&[LOG]),
            &[LOG_EDITED],
            vec![
                ("user:log", HookStatus::Pending),
                ("user:log", HookStatus::Stale),
            ],
        ),
        (
            Some(&[LOG]),
            &[LOG, GUARD],
            vec![
                ("user:log", HookStatus::Active),
                ("user:guard", HookStatus::NeedsReopen),
            ],
        ),
        (
            Some(&[LOG]),
            &[LOG_EDITED, GUARD],
            vec![
                ("user:log", HookStatus::NeedsReopen),
                ("user:guard", HookStatus::NeedsReopen),
                ("user:log", HookStatus::Stale),
            ],
        ),
        (None, &[LOG], vec![("user:log", HookStatus::NeedsReopen)]),
    ];

    for (started, now, expected) in cases {
        let pipeline = started.map(|hooks| {
            HookPipeline::start(
                catalog(hooks).unwrap(),
                SessionEnvironment::default(),
                rho_sdk::CancellationToken::new(),
            )
            .unwrap()
        });
        let report = HookSnapshot::checked(pipeline.as_ref(), catalog(now)).session_report();

        let listed: Vec<_> = report
            .hooks
            .iter()
            .map(|hook| (hook.id.as_str(), hook.status))
            .collect();
        assert_eq!(
            (report.enabled, listed),
            (started.is_some(), expected),
            "started with {started:?}, files now {now:?}"
        );
    }
}

#[test]
fn an_activity_view_carries_the_denial_reason() {
    let activity = HookActivity {
        hook_id: Some("user:no-force-push".into()),
        event: ConfiguredHookEvent::Sdk(HookEventKind::BeforeToolUse),
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

// Covers: the typed report names each hook event as a hooks file does, so a
// swapped arm or an SDK event the mapping misses (sent as `unknown`) fails.
// Owner: hook diagnostics.
#[test]
fn every_hook_event_has_its_hooks_file_name_in_the_typed_report() {
    let events = HookEventKind::ALL
        .iter()
        .map(|event| ConfiguredHookEvent::Sdk(*event))
        .chain(
            WorkflowHookEventKind::ALL
                .iter()
                .map(|event| ConfiguredHookEvent::Workflow(*event)),
        );
    for event in events {
        assert_eq!(
            serde_json::to_value(wire_event(event)).unwrap(),
            json!(event.wire_name())
        );
    }
}
