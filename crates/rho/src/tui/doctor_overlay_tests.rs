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
        scroll: 0,
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
