use pretty_assertions::assert_eq;

use super::{ArtifactKind, ArtifactObservation, ArtifactRef, Digest};

fn artifact(retained_bytes: u64, observed: ArtifactObservation) -> ArtifactRef {
    ArtifactRef {
        relative_path: "artifacts/build/stdout".into(),
        retained_bytes,
        observed,
        digest: Digest("sha256:artifact".into()),
    }
}

// Covers: one artifact must read the same in the tool, the TUI, and the CLI.
// Owner: workflow domain vocabulary.
#[test]
fn artifact_kind_labels_are_human_readable() {
    let cases = [
        (ArtifactKind::Stdout, "stdout"),
        (ArtifactKind::Stderr, "stderr"),
        (ArtifactKind::AgentAnswer, "answer"),
        (ArtifactKind::StructuredOutput, "structured output"),
        (ArtifactKind::CommandOutcome, "command outcome"),
    ];
    for (kind, expected) in cases {
        assert_eq!(kind.label(), expected, "{kind:?}");
    }
}

// Covers: an artifact reports a shortfall only when one exists, so complete
// artifacts do not spend output budget on a notice that carries nothing.
// Owner: workflow domain vocabulary.
#[test]
fn observation_notices_describe_only_incomplete_artifacts() {
    assert_eq!(
        artifact(8, ArtifactObservation::Complete { observed_bytes: 8 }).observation_notice(),
        None
    );
    assert_eq!(
        artifact(
            8,
            ArtifactObservation::Truncated {
                observed_bytes_at_least: 12
            }
        )
        .observation_notice()
        .as_deref(),
        Some("truncated · showing 8 of at least 12 bytes")
    );
    assert_eq!(
        artifact(8, ArtifactObservation::Incomplete { observed_bytes: 20 })
            .observation_notice()
            .as_deref(),
        Some("incomplete · retained 8 bytes (observed 20)")
    );
}
