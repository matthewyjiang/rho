use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    AttemptNumber, CommandExit, Digest, FrozenWorkflow, NodeId, NodeTerminalState, PlanId, RunId,
    RunLifecycle, WorkflowState, WorkflowValue,
};

pub(crate) const PLAN_MANIFEST_VERSION: u32 = 1;
pub(crate) const RUN_MANIFEST_VERSION: u32 = 1;
pub(crate) const RUN_STATE_VERSION: u32 = 1;
pub(crate) const EVENT_VERSION: u32 = 1;
pub(crate) const ATTEMPT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanManifest {
    pub(crate) schema_version: u32,
    pub(crate) plan_id: PlanId,
    pub(crate) graph_digest: Digest,
    pub(crate) workspace_identity: String,
    pub(crate) source_digests: BTreeMap<String, Digest>,
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
    pub(crate) plan_id: PlanId,
    pub(crate) graph_digest: Digest,
    pub(crate) workspace_identity: String,
    pub(crate) consent: PlanConsent,
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
    CancellationRequested,
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
        attempt: AttemptNumber,
        outcome: NodeTerminalState,
    },
    StructuredOutput {
        node: NodeId,
        value: WorkflowValue,
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
    Completed { outcome: NodeTerminalState },
    CleanlyCancelled,
    InterruptedUncertain { owner: ExternalOwner },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArtifactRef {
    pub(crate) relative_path: String,
    pub(crate) bytes: u64,
    pub(crate) digest: Digest,
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
