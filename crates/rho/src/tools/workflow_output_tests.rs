use pretty_assertions::assert_eq;
use rho_sdk::tool::ToolErrorKind;

use crate::workflow::{
    ArtifactKind, ArtifactObservation, ArtifactRef, Digest, NodeState, NodeTerminalState,
    RunLifecycle,
};

use crate::tools::workflow::{
    WorkflowArtifactSummary, WorkflowCancellationStateSummary, WorkflowDiagnosticSummary,
    WorkflowNodeSummary, WorkflowToolResult,
};

use super::{bounded_result, format_workflow_result};

fn artifact(observed: ArtifactObservation) -> WorkflowArtifactSummary {
    WorkflowArtifactSummary {
        kind: ArtifactKind::Stdout,
        artifact: ArtifactRef {
            relative_path: "artifacts/build/stdout".into(),
            retained_bytes: 8,
            observed,
            digest: Digest("sha256:artifact".into()),
        },
    }
}

fn oversized_validation() -> WorkflowToolResult {
    WorkflowToolResult::Validate {
        valid: false,
        diagnostics: vec![WorkflowDiagnosticSummary {
            severity: "error".into(),
            code: "invalid".into(),
            message: "é".repeat(4096),
            source: None,
            line: None,
            column: None,
        }],
    }
}

// Covers: each workflow operation must expose its useful result fields as a
// readable line summary rather than model-facing JSON.
// Owner: model-facing workflow tool formatter.
#[test]
fn workflow_results_are_readable_line_summaries() {
    let cases = vec![
        (
            "validate",
            WorkflowToolResult::Validate {
                valid: false,
                diagnostics: vec![WorkflowDiagnosticSummary {
                    severity: "error".into(),
                    code: "invalid_graph".into(),
                    message: "cycle found\ncheck dependencies".into(),
                    source: Some("review.star".into()),
                    line: Some(7),
                    column: Some(3),
                }],
            },
            vec![
                "workflow validation: invalid",
                "diagnostics:",
                "  error [invalid_graph]: cycle found",
                "    check dependencies",
                "    source: review.star",
                "    line: 7",
                "    column: 3",
            ],
        ),
        (
            "plan",
            WorkflowToolResult::Plan {
                plan_id: "plan-1".into(),
                graph_digest: "sha256:plan".into(),
                workflow_name: "review".into(),
                node_count: 2,
            },
            vec![
                "workflow review: planned",
                "plan_id: plan-1",
                "graph_digest: sha256:plan",
                "nodes: 2",
            ],
        ),
        (
            "run with a truncated artifact",
            WorkflowToolResult::Run {
                run_id: "run-1".into(),
                graph_digest: "sha256:run".into(),
                state: RunLifecycle::Running,
                nodes: vec![WorkflowNodeSummary {
                    node_id: "build".into(),
                    state: NodeState::Running {
                        attempt: 2.try_into().expect("attempt"),
                    },
                    attempt: Some(2),
                    artifacts: vec![artifact(ArtifactObservation::Truncated {
                        observed_bytes_at_least: 12,
                    })],
                }],
            },
            vec![
                "workflow run-1: running",
                "graph_digest: sha256:run",
                "nodes: 1",
                "  build · running · attempt 2",
                "    stdout: artifacts/build/stdout · 8 bytes · digest sha256:artifact · truncated · showing 8 of at least 12 bytes",
            ],
        ),
        (
            "run with a complete artifact stays quiet about observation",
            WorkflowToolResult::Run {
                run_id: "run-2".into(),
                graph_digest: "sha256:run".into(),
                state: RunLifecycle::Completed,
                nodes: vec![WorkflowNodeSummary {
                    node_id: "build".into(),
                    state: NodeState::Terminal {
                        outcome: NodeTerminalState::Success,
                    },
                    attempt: Some(1),
                    artifacts: vec![artifact(ArtifactObservation::Complete { observed_bytes: 8 })],
                }],
            },
            vec![
                "workflow run-2: completed",
                "graph_digest: sha256:run",
                "nodes: 1",
                "  build · success · attempt 1",
                "    stdout: artifacts/build/stdout · 8 bytes · digest sha256:artifact",
            ],
        ),
        (
            "status without nodes",
            WorkflowToolResult::Run {
                run_id: "run-3".into(),
                graph_digest: "sha256:status".into(),
                state: RunLifecycle::NeedsRecovery,
                nodes: Vec::new(),
            },
            vec![
                "workflow run-3: needs_recovery",
                "graph_digest: sha256:status",
                "nodes: 0",
            ],
        ),
        (
            "cancel",
            WorkflowToolResult::Cancel {
                run_id: "run-4".into(),
                request_id: Some("request-1".into()),
                cancellation_state: WorkflowCancellationStateSummary::Pending,
                state: RunLifecycle::Running,
            },
            vec![
                "workflow run-4: running",
                "cancellation: pending",
                "request_id: request-1",
            ],
        ),
    ];

    for (operation, result, expected) in cases {
        assert_eq!(format_workflow_result(&result), expected, "{operation}");
    }
}

// Covers: an oversized summary must stay within the byte limit, keep as many
// whole lines as fit, and report how much it dropped.
// Owner: model-facing workflow tool formatter.
#[test]
fn bounded_results_keep_whole_lines_and_report_the_omission() {
    let result = WorkflowToolResult::Run {
        run_id: "run-1".into(),
        graph_digest: "sha256:run".into(),
        state: RunLifecycle::Running,
        nodes: (0..40)
            .map(|index| WorkflowNodeSummary {
                node_id: format!("node-{index}"),
                state: NodeState::Pending,
                attempt: None,
                artifacts: Vec::new(),
            })
            .collect(),
    };
    let max_output_bytes = 160;

    let output = bounded_result(&result, max_output_bytes).expect("bounded");

    assert!(output.len() <= max_output_bytes, "{} bytes", output.len());
    assert_eq!(
        output,
        concat!(
            "workflow run-1: running\n",
            "graph_digest: sha256:run\n",
            "nodes: 40\n",
            "... 40 more line(s) omitted; workflow summary is 888 bytes and the limit is 160 bytes"
        )
    );
}

// Covers: a single line too long for the budget must be clipped, not dropped,
// so the model still learns which diagnostic failed.
// Owner: model-facing workflow tool formatter.
#[test]
fn bounded_results_clip_a_line_the_budget_cannot_hold_whole() {
    let max_output_bytes = 256;

    let output = bounded_result(&oversized_validation(), max_output_bytes).expect("bounded");

    assert!(output.len() <= max_output_bytes, "{} bytes", output.len());
    let mut lines = output.lines();
    assert_eq!(lines.next(), Some("workflow validation: invalid"));
    assert_eq!(lines.next(), Some("diagnostics:"));
    let clipped = lines.next().expect("clipped diagnostic line");
    assert!(
        clipped.starts_with("  error [invalid]: ééé"),
        "clipped line should carry diagnostic detail, got {clipped:?}"
    );
    assert_eq!(lines.next(), Some("[truncated]"));
    assert_eq!(
        lines.next(),
        Some(
            "... 1 more line(s) omitted; workflow summary is 8253 bytes and the limit is 256 bytes"
        )
    );
    assert_eq!(lines.next(), None);
}

// Covers: a summary that already fits is returned whole.
// Owner: model-facing workflow tool formatter.
#[test]
fn bounded_results_pass_through_a_summary_that_fits() {
    let valid = WorkflowToolResult::Validate {
        valid: true,
        diagnostics: Vec::new(),
    };

    assert_eq!(
        bounded_result(&valid, 256).expect("bounded"),
        "workflow validation: valid"
    );
}

// Covers: a budget too small even for the omission notice must fail loudly
// rather than emit a summary that overruns the tool output limit.
// Owner: model-facing workflow tool formatter.
#[test]
fn bounded_results_reject_a_budget_below_the_omission_notice() {
    let tiny_limit = 4;

    let error = bounded_result(&oversized_validation(), tiny_limit).expect_err("budget too small");

    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert_eq!(
        error.message(),
        "workflow tool output budget is too small: accepted limit 4, required 83"
    );
}
