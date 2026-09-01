use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::doctor::{DoctorCheck, DoctorCheckId, DoctorStatus};

// Covers: `rho doctor --json` emits summary counts plus sections with
// snake_case status and tagged check ids.
// Owner: CLI (pure serialization)
#[test]
fn json_document_carries_summary_and_sections() {
    let report = DoctorReport::from_checks(vec![
        DoctorCheck::new(
            DoctorCheckId::ProviderAuth {
                auth_mode: "openai-api-key".into(),
            },
            "OpenAI API key",
            DoctorStatus::Ok,
            "authenticated",
        ),
        DoctorCheck::new(DoctorCheckId::Mcp, "MCP", DoctorStatus::Warn, "degraded")
            .with_hint("run /mcp for details"),
    ]);

    let document = serde_json::to_value(DoctorDocument::new(&report)).unwrap();

    assert_eq!(
        document,
        json!({
            "summary": { "ok": 1, "info": 0, "warn": 1, "fail": 0, "checking": 0 },
            "sections": [
                {
                    "id": "authentication",
                    "checks": [{
                        "id": { "kind": "provider_auth", "auth_mode": "openai-api-key" },
                        "label": "OpenAI API key",
                        "status": "ok",
                        "summary": "authenticated"
                    }]
                },
                {
                    "id": "extensions",
                    "checks": [{
                        "id": { "kind": "mcp" },
                        "label": "MCP",
                        "status": "warn",
                        "summary": "degraded",
                        "hint": "run /mcp for details"
                    }]
                }
            ]
        })
    );
}

// Covers: a probe that outlives the deadline is aborted and reported as timed
// out; finished probes keep their results.
// Owner: CLI (paused tokio clock, no child processes)
#[tokio::test(start_paused = true)]
async fn collect_probes_times_out_slow_tasks() {
    let fast = tokio::spawn(async { DoctorProbeOutcome::Rtk { available: true } });
    let slow = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
        DoctorProbeOutcome::Rtk { available: true }
    });

    let outcomes = collect_probes(
        vec![(DoctorProbeId::Rtk, fast), (DoctorProbeId::Claude, slow)],
        Duration::from_secs(1),
    )
    .await;

    assert!(matches!(
        outcomes[0],
        DoctorProbeOutcome::Rtk { available: true }
    ));
    assert!(matches!(
        outcomes[1],
        DoctorProbeOutcome::TimedOut(DoctorProbeId::Claude)
    ));
}
