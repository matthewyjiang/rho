use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    AttemptNumber, CommandExit, Digest, FrozenWorkflow, NodeId, NodeResetReason, NodeTerminalState,
    PlanId, RunId, RunLifecycle, WorkflowState, WorkflowValue,
};

pub(crate) const PLAN_MANIFEST_VERSION: u32 = 1;
pub(crate) const RUN_MANIFEST_VERSION: u32 = 1;
pub(crate) const RUN_STATE_VERSION: u32 = 2;
pub(crate) const EVENT_VERSION: u32 = 3;
pub(crate) const ATTEMPT_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanManifest {
    pub(crate) schema_version: u32,
    pub(crate) plan_id: PlanId,
    /// Creation time used for durable newest-first inventory ordering.
    #[serde(default)]
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) graph_digest: Digest,
    pub(crate) workspace_identity: String,
    pub(crate) source_digests: BTreeMap<String, Digest>,
    /// Inventory label. Default empty for manifests written before this field.
    #[serde(default)]
    pub(crate) name: String,
    /// Inventory step count. Default 0 for manifests written before this field.
    #[serde(default)]
    pub(crate) step_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredPlan {
    pub(crate) manifest: PlanManifest,
    pub(crate) graph: FrozenWorkflow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanConsent {
    pub(crate) graph_digest: Digest,
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunManifest {
    pub(crate) schema_version: u32,
    pub(crate) run_id: RunId,
    /// Creation time used for durable newest-first inventory ordering.
    #[serde(default)]
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) plan_id: PlanId,
    pub(crate) graph_digest: Digest,
    pub(crate) workspace_identity: String,
    pub(crate) consent: PlanConsent,
    /// Inventory label. Default empty for manifests written before this field.
    #[serde(default)]
    pub(crate) name: String,
    /// Inventory step count. Default 0 for manifests written before this field.
    #[serde(default)]
    pub(crate) step_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunStateRecord {
    pub(crate) schema_version: u32,
    pub(crate) last_event_sequence: u64,
    pub(crate) state: WorkflowState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredRun {
    pub(crate) manifest: RunManifest,
    pub(crate) graph: FrozenWorkflow,
    pub(crate) state: RunStateRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkflowEventRecord {
    pub(crate) schema_version: u32,
    pub(crate) sequence: u64,
    pub(crate) event: WorkflowEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WorkflowEvent {
    RunLifecycle {
        lifecycle: RunLifecycle,
    },
    CancellationRequested {
        request_id: String,
    },
    NodeReady {
        node: NodeId,
    },
    LaunchIntended {
        node: NodeId,
        attempt: AttemptNumber,
    },
    AttemptStarted {
        node: NodeId,
        attempt: AttemptNumber,
        owner: ExternalOwner,
    },
    NodeFinished {
        node: NodeId,
        completion: Box<NodeCompletion>,
    },
    StructuredOutput {
        node: NodeId,
        attempt: AttemptNumber,
        output: ValidatedOutputRef,
    },
    NodeReset {
        node: NodeId,
        reason: NodeResetReason,
    },
    CancellationCleared,
    CancellationAcknowledged {
        request_id: String,
    },
    HookObserved {
        event: String,
        node: Option<NodeId>,
        attempt: Option<AttemptNumber>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ExternalOwner {
    Process { pid: u32 },
    Agent { session_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AttemptRecord {
    pub(crate) schema_version: u32,
    pub(crate) attempt: AttemptNumber,
    pub(crate) state: AttemptState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum AttemptState {
    LaunchIntended,
    Started { owner: ExternalOwner },
    Completed { completion: Box<NodeCompletion> },
    CleanlyCancelled,
    InterruptedUncertain { owner: ExternalOwner },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArtifactRef {
    pub(crate) relative_path: String,
    pub(crate) retained_bytes: u64,
    pub(crate) observed: ArtifactObservation,
    pub(crate) digest: Digest,
}

impl ArtifactRef {
    /// How this artifact fell short of its full source, when it did.
    ///
    /// `None` means the retained bytes are the whole artifact, so callers stay
    /// quiet in the common case. Shared by the model-facing workflow tool and
    /// the TUI so one artifact is described the same way in both.
    pub(crate) fn observation_notice(&self) -> Option<String> {
        let retained_bytes = self.retained_bytes;
        match self.observed {
            ArtifactObservation::Complete { .. } => None,
            ArtifactObservation::Truncated {
                observed_bytes_at_least,
            } => Some(format!(
                "truncated · showing {retained_bytes} of at least {observed_bytes_at_least} bytes"
            )),
            ArtifactObservation::Incomplete { observed_bytes } => Some(format!(
                "incomplete · retained {retained_bytes} bytes (observed {observed_bytes})"
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ArtifactObservation {
    Complete { observed_bytes: u64 },
    Truncated { observed_bytes_at_least: u64 },
    Incomplete { observed_bytes: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ValidatedOutputRef {
    pub(crate) artifact: ArtifactRef,
    pub(crate) value: WorkflowValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CommandOutcome {
    pub(crate) exit: CommandExit,
    pub(crate) stdout: ArtifactRef,
    pub(crate) stderr: ArtifactRef,
    pub(crate) structured_output: Option<ValidatedOutputRef>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AttemptArtifacts {
    pub(crate) stdout: Option<ArtifactRef>,
    pub(crate) stderr: Option<ArtifactRef>,
    pub(crate) answer: Option<ArtifactRef>,
    pub(crate) structured_output: Option<ArtifactRef>,
    pub(crate) command_outcome: Option<ArtifactRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArtifactKind {
    Stdout,
    Stderr,
    AgentAnswer,
    StructuredOutput,
    CommandOutcome,
}

impl ArtifactKind {
    /// The human-readable artifact name.
    ///
    /// Shared by the model-facing workflow tool, the TUI, and the CLI so one
    /// artifact reads the same everywhere. The serialized token stays
    /// snake_case and is a separate concern.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::AgentAnswer => "answer",
            Self::StructuredOutput => "structured output",
            Self::CommandOutcome => "command outcome",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DurableArtifactReference {
    pub(crate) kind: ArtifactKind,
    #[serde(flatten)]
    pub(crate) artifact: ArtifactRef,
}

impl AttemptArtifacts {
    pub(crate) fn iter(&self) -> impl Iterator<Item = (ArtifactKind, &ArtifactRef)> {
        [
            (ArtifactKind::Stdout, self.stdout.as_ref()),
            (ArtifactKind::Stderr, self.stderr.as_ref()),
            (ArtifactKind::AgentAnswer, self.answer.as_ref()),
            (
                ArtifactKind::StructuredOutput,
                self.structured_output.as_ref(),
            ),
            (ArtifactKind::CommandOutcome, self.command_outcome.as_ref()),
        ]
        .into_iter()
        .filter_map(|(kind, artifact)| artifact.map(|artifact| (kind, artifact)))
    }

    pub(crate) fn references(&self) -> Vec<DurableArtifactReference> {
        self.iter()
            .map(|(kind, artifact)| DurableArtifactReference {
                kind,
                artifact: artifact.clone(),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NodeCompletion {
    pub(crate) attempt: Option<AttemptNumber>,
    pub(crate) outcome: NodeTerminalState,
    pub(crate) cancellation_resume: Option<CancellationResumeState>,
    pub(crate) command_exit: Option<CommandExit>,
    pub(crate) structured_output: Option<ValidatedOutputRef>,
    pub(crate) artifacts: AttemptArtifacts,
}

impl NodeCompletion {
    pub(crate) fn terminal(outcome: NodeTerminalState) -> Self {
        Self {
            attempt: None,
            outcome,
            cancellation_resume: None,
            command_exit: None,
            structured_output: None,
            artifacts: AttemptArtifacts::default(),
        }
    }

    pub(crate) fn cancelled(resume: CancellationResumeState) -> Self {
        Self {
            attempt: None,
            outcome: NodeTerminalState::Cancellation,
            cancellation_resume: Some(resume),
            command_exit: None,
            structured_output: None,
            artifacts: AttemptArtifacts::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CancellationResumeState {
    Pending,
    Ready,
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
