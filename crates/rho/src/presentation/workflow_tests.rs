use std::path::Path;

use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolFact, ToolStatus};
use serde_json::json;

use super::{finished_card, parse_summary, Headline, Node, NodeProgress, Summary};

// Covers: bounded summaries must not turn omitted or unknown states into
// completed nodes; every terminal outcome counts, not just successful nodes.
// Owner: pure workflow summary projection (no terminal rendering involved).
#[test]
fn progress_is_partial_when_state_details_are_missing_or_unknown() {
    for (states, total, expected) in [
        (
            vec![
                "success",
                "failure",
                "denial",
                "cancellation",
                "skipped",
                "blocked",
                "running",
                "ready",
                "pending",
            ],
            9,
            NodeProgress {
                finished: 6,
                running: 1,
                waiting: 2,
                available: 9,
                total: 9,
                partial: false,
            },
        ),
        (
            vec!["success", "running"],
            4,
            NodeProgress {
                finished: 1,
                running: 1,
                waiting: 0,
                available: 2,
                total: 4,
                partial: true,
            },
        ),
        (
            vec!["success", "future_state"],
            2,
            NodeProgress {
                finished: 1,
                running: 0,
                waiting: 0,
                available: 2,
                total: 2,
                partial: true,
            },
        ),
        (
            vec![],
            0,
            NodeProgress {
                finished: 0,
                running: 0,
                waiting: 0,
                available: 0,
                total: 0,
                partial: false,
            },
        ),
    ] {
        let mut lines = vec![
            "workflow run-1: running".to_owned(),
            format!("nodes: {total}"),
        ];
        lines.extend(
            states
                .iter()
                .enumerate()
                .map(|(index, state)| format!("  node-{index} · {state} · attempt 2")),
        );
        let refs: Vec<_> = lines.iter().map(String::as_str).collect();
        let summary = parse_summary(&refs, Some("status")).unwrap();
        assert_eq!(summary.progress(total), expected, "{states:?}");
    }
}

// Covers: indented diagnostic/export/artifact data may contain state-looking
// labels; only protocol-depth fields may affect the receipt.
// Owner: pure workflow summary parser.
#[test]
fn nested_details_do_not_override_run_fields() {
    let lines = [
        "workflow run-1: completed",
        "program_digest: sha256:run",
        "root outcome: success",
        "  export summary: {\"accepted\":true}",
        "    nodes: 99",
        "    cancellation: pending",
        "nodes: 1",
        "  build · success · attempt 2",
        "    stdout: artifacts/build/stdout · 8 bytes · digest sha256:artifact",
        "    fake-node · failure",
        "    root outcome: failure",
        "future_detail: kept",
    ];
    assert_eq!(
        parse_summary(&lines, Some("status")),
        Some(Summary {
            headline: Headline::Run {
                id: "run-1",
                state: "completed"
            },
            plan_id: None,
            node_count: Some(1),
            nodes: vec![Node {
                id: "build",
                state: "success"
            }],
            outcome: Some("success"),
            cancellation: None,
            diagnostics: 0,
            errors: vec![],
            omission: None,
        })
    );
    for action in ["run", "status", "resume"] {
        let card = finished_card(
            &json!({"action": action}),
            &lines.join("\n"),
            true,
            Path::new("."),
        );
        assert_eq!(
            card.body,
            ToolBody::Lines(lines[1..].iter().map(|line| (*line).into()).collect())
        );
    }
}

// Covers: a planned run lifecycle is not a newly frozen plan; unknown and
// clipped headers must retain their details rather than inventing a receipt.
// Owner: pure workflow summary parser.
#[test]
fn headline_distinguishes_plan_from_planned_run_and_rejects_incomplete_results() {
    for (action, expected) in [
        ("plan", Headline::Plan { name: "identity" }),
        (
            "status",
            Headline::Run {
                id: "identity",
                state: "planned",
            },
        ),
        (
            "run",
            Headline::Run {
                id: "identity",
                state: "planned",
            },
        ),
        (
            "resume",
            Headline::Run {
                id: "identity",
                state: "planned",
            },
        ),
        (
            "cancel",
            Headline::Run {
                id: "identity",
                state: "planned",
            },
        ),
    ] {
        assert_eq!(
            parse_summary(&["workflow identity: planned"], Some(action))
                .unwrap()
                .headline,
            expected
        );
    }
    for first in [
        "",
        "workflow validation: inv",
        "workflow run-1:",
        "workflow : running",
        "{\"state\":\"running\"}",
    ] {
        assert_eq!(parse_summary(&[first], Some("status")), None, "{first}");
        let card = finished_card(
            &json!({"action": "status", "run_id": "run-1"}),
            first,
            true,
            Path::new("."),
        );
        assert_eq!(
            card.body,
            ToolBody::Lines(first.lines().map(str::to_owned).collect())
        );
    }
}

// Covers: validation/tool errors stay visible while multiline diagnostics,
// positions and output-omission notices remain available on expansion.
// Owner: pure workflow card projection.
#[test]
fn failures_promote_a_reason_without_losing_detail() {
    let invalid = [
        "workflow validation: invalid",
        "diagnostics:",
        "  error [cycle]: cycle found",
        "    nodes: 99",
        "    source: review.star",
        "    line: 7",
        "    column: 3",
        "... 2 more line(s) omitted; workflow summary is 800 bytes and the limit is 400 bytes",
    ]
    .join("\n");
    let arguments = json!({"action": "validate"});
    let card = finished_card(&arguments, &invalid, true, Path::new("."));
    assert_eq!(card.status, ToolStatus::Error);
    assert_eq!(
        card.facts,
        vec![
            ToolFact::Error {
                text: "error [cycle]: cycle found".into()
            },
            ToolFact::Count {
                label: "diagnostics".into(),
                value: 1,
                detail: None
            },
            ToolFact::Meta {
                text: invalid.lines().last().unwrap().into()
            },
        ]
    );
    assert_eq!(
        card.body,
        ToolBody::Lines(invalid.lines().skip(1).map(str::to_owned).collect())
    );
    for action in ["validate", "plan", "run", "status", "cancel", "resume"] {
        let error = "permission denied\nworkflow was not started";
        let card = finished_card(&json!({"action": action}), error, false, Path::new("."));
        assert_eq!(card.status, ToolStatus::Error);
        assert_eq!(
            card.facts.first(),
            Some(&ToolFact::Error {
                text: "permission denied".into()
            })
        );
        assert_eq!(
            card.body,
            ToolBody::Lines(error.lines().map(str::to_owned).collect())
        );
    }
}
