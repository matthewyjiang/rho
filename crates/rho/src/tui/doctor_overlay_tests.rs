use pretty_assertions::assert_eq;

use super::*;
use crate::doctor::DoctorCheckId;

fn text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn overlay(checks: Vec<DoctorCheck>) -> DoctorOverlay {
    DoctorOverlay {
        report: DoctorReport::from_checks(checks),
        scroll: Default::default(),
        checking_started: Instant::now(),
    }
}

// Covers: /doctor must open a popup instead of inserting a transcript row or
// queuing a model turn, and must not leave probe tasks behind under the test
// gate.
// Owner: interactive TUI (unit seam; PTY covers the visible overlay)
#[tokio::test]
async fn opening_doctor_does_not_queue_model_context() {
    let mut app = super::super::tests::test_app();
    app.begin_provider_turn_ui();
    app.start_doctor_command().unwrap();

    assert!(app.pending.steering_prompts().is_empty());
    assert!(app.pending.queued_prompts().is_empty());
    assert!(matches!(
        app.input_ui.composer(),
        super::super::ComposerMode::Doctor(_)
    ));
    assert!(app.pending_doctor_probes.is_empty());
    assert!(
        app.history
            .entries()
            .iter()
            .all(|entry| !matches!(entry, super::super::Entry::Error(_))),
        "doctor overlay must not dump a transcript error, got {:?}",
        app.history.entries()
    );
}

// Covers: cancelling must stop the probe task, not just forget the handle.
// Owner: interactive TUI (unit seam)
#[tokio::test]
async fn cancelling_doctor_probes_waits_for_task_to_stop() {
    let mut app = super::super::tests::test_app();
    let task_marker = std::sync::Arc::new(());
    let captured_marker = task_marker.clone();
    app.pending_doctor_probes.push(PendingDoctorProbe {
        id: DoctorProbeId::Rtk,
        handle: tokio::spawn(async move {
            let _marker = captured_marker;
            std::future::pending::<DoctorProbeOutcome>().await
        }),
    });

    app.cancel_doctor_command().await;

    assert!(app.pending_doctor_probes.is_empty());
    assert_eq!(std::sync::Arc::strong_count(&task_marker), 1);
}

async fn wait_until_finished(handle: &tokio::task::JoinHandle<DoctorProbeOutcome>) {
    while !handle.is_finished() {
        tokio::task::yield_now().await;
    }
}

fn checking_rtk_row() -> DoctorCheck {
    DoctorCheck::new(
        DoctorCheckId::Rtk,
        "rtk",
        DoctorStatus::Checking,
        "checking",
    )
}

fn rtk_row(app: &mut super::super::App) -> DoctorCheck {
    let overlay = app
        .doctor_overlay_mut()
        .expect("doctor overlay should stay open");
    overlay
        .report
        .checks()
        .find(|check| check.id == DoctorCheckId::Rtk)
        .cloned()
        .expect("rtk row")
}

// Covers: a finished probe replaces its Checking row and clears is_checking;
// a join error becomes a failure row instead of spinning forever.
// Owner: interactive TUI (unit seam; PTY cannot observe task join)
#[tokio::test]
async fn poll_applies_finished_probe_and_failed_join() {
    let mut app = super::super::tests::test_app();
    app.start_doctor_command().unwrap();
    app.doctor_overlay_mut()
        .unwrap()
        .report
        .replace_checks(vec![checking_rtk_row()]);

    let handle = tokio::spawn(async { DoctorProbeOutcome::Rtk { available: true } });
    wait_until_finished(&handle).await;
    app.pending_doctor_probes.push(PendingDoctorProbe {
        id: DoctorProbeId::Rtk,
        handle,
    });
    assert!(app.poll_doctor_command().await.unwrap());
    assert!(!app.doctor_overlay_mut().unwrap().is_checking());
    let rtk = rtk_row(&mut app);
    assert_eq!(
        (rtk.status, rtk.summary.as_str(), rtk.hint.as_deref()),
        (DoctorStatus::Ok, "available", None)
    );

    app.doctor_overlay_mut()
        .unwrap()
        .report
        .replace_checks(vec![checking_rtk_row()]);
    let handle = tokio::spawn(std::future::pending::<DoctorProbeOutcome>());
    handle.abort();
    wait_until_finished(&handle).await;
    app.pending_doctor_probes.push(PendingDoctorProbe {
        id: DoctorProbeId::Rtk,
        handle,
    });
    assert!(app.poll_doctor_command().await.unwrap());
    let rtk = rtk_row(&mut app);
    assert_eq!(
        (rtk.status, rtk.summary.as_str()),
        (DoctorStatus::Warn, "probe failed")
    );
}

// Covers: approval/questionnaire set_composer must abort live probes, not
// leave children running until shutdown. PTY cannot observe task lifetime.
// Owner: interactive TUI (unit seam)
#[tokio::test]
async fn replacing_doctor_overlay_aborts_probes() {
    let mut app = super::super::tests::test_app();
    app.start_doctor_command().unwrap();
    let task_marker = std::sync::Arc::new(());
    let captured_marker = task_marker.clone();
    app.pending_doctor_probes.push(PendingDoctorProbe {
        id: DoctorProbeId::Rtk,
        handle: tokio::spawn(async move {
            let _marker = captured_marker;
            std::future::pending::<DoctorProbeOutcome>().await
        }),
    });

    app.input_ui.set_composer(super::super::ComposerMode::Input);
    app.poll_doctor_command().await.unwrap();

    assert!(app.pending_doctor_probes.is_empty());
    assert_eq!(std::sync::Arc::strong_count(&task_marker), 1);
}

// Covers: rows share one label column, a pending probe renders as a spinner
// row with a section spinner, and hints appear only under issues, wrapped to
// the panel width.
// Owner: pure layout
#[test]
fn body_lines_align_status_column_and_show_hints_under_issues() {
    let overlay = overlay(vec![
        DoctorCheck::new(
            DoctorCheckId::ProviderAuth {
                auth_mode: "api-key".into(),
            },
            "OpenAI API key",
            DoctorStatus::Ok,
            "authenticated",
        ),
        DoctorCheck::new(
            DoctorCheckId::ProviderAuth {
                auth_mode: "anthropic-api-key".into(),
            },
            "Anthropic API key",
            DoctorStatus::Warn,
            "missing",
        )
        .with_hint("run /login anthropic-api-key"),
        DoctorCheck::new(
            DoctorCheckId::ProviderEndpoint {
                provider: "ollama".into(),
            },
            "Ollama connection",
            DoctorStatus::Checking,
            "checking",
        ),
        DoctorCheck::new(
            DoctorCheckId::ConfigPath,
            "Configuration",
            DoctorStatus::Ok,
            "writable",
        )
        .with_hint("/home/dev/.rho/config.toml"),
    ]);

    let lines = overlay_body_lines(&overlay, 40, Some("⠋"))
        .iter()
        .map(text)
        .collect::<Vec<_>>();

    assert_eq!(
        lines,
        vec![
            "1 warning · checking 1",
            "",
            "Authentication",
            "  ✓ OpenAI API key     authenticated",
            "  ! Anthropic API key  missing",
            "    run /login anthropic-api-key",
            "",
            "Providers                     ⠋ checking",
            "  ⠋ Ollama connection  checking",
            "",
            "Workspace",
            "  ✓ Configuration      writable",
        ]
    );
}

// Covers: a settled report with no issues reads as passed and reserves no
// spinner column.
// Owner: pure layout
#[test]
fn settled_healthy_report_reads_as_passed() {
    let overlay = overlay(vec![DoctorCheck::new(
        DoctorCheckId::Rtk,
        "rtk",
        DoctorStatus::Ok,
        "available",
    )]);
    assert!(!overlay.is_checking());
    let lines = overlay_body_lines(&overlay, 30, None)
        .iter()
        .map(text)
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["all checks passed", "", "Runtimes", "  ✓ rtk  available"]
    );
}
