use pretty_assertions::assert_eq;

use super::RunReporter;
use crate::run_artifacts::RunArtifactIdentity;

#[test]
fn reporter_discards_partial_text_when_provider_attempt_resets() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("result.json");
    let mut reporter = RunReporter::new(
        output,
        RunArtifactIdentity {
            agent_id: "reviewer".into(),
            agent_fingerprint: "fingerprint".into(),
            provider: "test".into(),
            model: Some("test".into()),
            runtime: crate::agent::AgentRuntime::Rho,
            reasoning: None,
        },
        root.path().to_path_buf(),
        "review",
        /* stream_output */ false,
        None,
    )
    .unwrap();

    reporter.on_event(&rho_sdk::RunEvent::AssistantTextDelta {
        text: "stale partial response".into(),
    });
    reporter.on_event(&rho_sdk::RunEvent::ProviderStreamReset {
        reason: rho_sdk::ProviderStreamResetReason::RetryableFailure {
            kind: rho_sdk::ProviderErrorKind::Unavailable,
            retry_after: None,
        },
        detail: "retrying".into(),
    });

    assert_eq!(reporter.status().last_text, None);
    assert_eq!(
        reporter.status().last_activity.as_deref(),
        Some("retrying provider response")
    );
}
