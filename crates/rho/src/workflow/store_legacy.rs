//! Read-only access to plans and runs written before scoped workflow programs.
//!
//! Those releases stored plan and run manifests at version 1, a flat frozen
//! graph at version 2, and flat run state at version 2. They share the current
//! `plans/` and `runs/` directories. Inventory and `workflow status` still read
//! them. Run, resume, and cancel fail with [`WorkflowError::LegacyRecord`]
//! because those records have no root parameter schemas or journal-owned
//! attempt allocation to execute against.
//!
// NEXT_MAJOR(rho-coding-agent): stop reading version 1 workflow plan and run manifests and delete this module.

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use super::{plan_relative, read_json, run_relative, WorkflowStore};
use crate::workflow::{
    check_schema_version, CommandExit, Digest, NodeCompletion, NodeId, NodeState, PlanId, RunId,
    RunLifecycle, WorkflowError, WorkflowOutcome, WorkflowResult, WorkflowValue,
};

pub(super) const LEGACY_MANIFEST_VERSION: u32 = 1;
const LEGACY_FROZEN_GRAPH_VERSION: u32 = 2;
const LEGACY_RUN_STATE_VERSION: u32 = 2;

pub(super) enum ManifestRecord<T, L> {
    Current(T),
    Legacy(L),
}

/// A stored run for display; only current runs can execute.
pub(crate) enum RunRecord {
    Current(Box<crate::workflow::StoredRun>),
    // NEXT_MAJOR(rho-coding-agent): drop read-only legacy workflow runs from status.
    Legacy(Box<LegacyRun>),
}

/// Reads once, checks the generation, then deserializes its typed manifest.
fn read_manifest<T: serde::de::DeserializeOwned, L: serde::de::DeserializeOwned>(
    store: &WorkflowStore,
    relative: &Path,
    kind: &'static str,
    current_version: u32,
) -> WorkflowResult<ManifestRecord<T, L>> {
    let value: serde_json::Value = read_json(&store.root, relative)?;
    #[derive(Deserialize)]
    struct Version {
        schema_version: u32,
    }
    let version: Version = serde_json::from_value(value.clone())?;
    if version.schema_version == LEGACY_MANIFEST_VERSION {
        Ok(ManifestRecord::Legacy(serde_json::from_value(value)?))
    } else {
        check_schema_version(kind, version.schema_version, current_version)?;
        Ok(ManifestRecord::Current(serde_json::from_value(value)?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyPlanManifest {
    pub(crate) schema_version: u32,
    pub(crate) plan_id: PlanId,
    #[serde(default)]
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) graph_digest: Digest,
    pub(crate) workspace_identity: String,
    pub(crate) source_digests: BTreeMap<String, Digest>,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) step_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyPlanConsent {
    pub(crate) graph_digest: Digest,
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyRunManifest {
    pub(crate) schema_version: u32,
    pub(crate) run_id: RunId,
    #[serde(default)]
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) plan_id: PlanId,
    pub(crate) graph_digest: Digest,
    pub(crate) workspace_identity: String,
    pub(crate) consent: LegacyPlanConsent,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) step_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyRunState {
    pub(crate) schema_version: u32,
    pub(crate) last_event_sequence: u64,
    pub(crate) state: LegacyWorkflowState,
}

/// Flat single-graph run state keyed by node ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyWorkflowState {
    pub(crate) revision: u64,
    pub(crate) lifecycle: RunLifecycle,
    pub(crate) outcome: Option<WorkflowOutcome>,
    pub(crate) cancellation_requested: bool,
    pub(crate) nodes: BTreeMap<NodeId, NodeState>,
    pub(crate) command_exits: BTreeMap<NodeId, CommandExit>,
    pub(crate) outputs: BTreeMap<NodeId, WorkflowValue>,
    pub(crate) completions: BTreeMap<NodeId, NodeCompletion>,
}

/// A legacy run as stored. Serializes to the status document shape those
/// releases printed. The frozen graph stays opaque because nothing executes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct LegacyRun {
    pub(crate) manifest: LegacyRunManifest,
    pub(crate) graph: serde_json::Value,
    pub(crate) state: LegacyRunState,
}

impl WorkflowStore {
    /// True when the run was written before scoped programs and is read-only.
    pub(crate) fn is_legacy_run(&self, id: RunId) -> WorkflowResult<bool> {
        Ok(matches!(
            self.read_run_manifest(id)?,
            ManifestRecord::Legacy(_)
        ))
    }

    pub(super) fn read_plan_manifest(
        &self,
        id: PlanId,
    ) -> WorkflowResult<ManifestRecord<crate::workflow::PlanManifest, LegacyPlanManifest>> {
        let manifest = read_manifest::<crate::workflow::PlanManifest, LegacyPlanManifest>(
            self,
            &plan_relative(id, Path::new("manifest.json")),
            "plan manifest",
            crate::workflow::PLAN_MANIFEST_VERSION,
        )?;
        let stored_id = match &manifest {
            ManifestRecord::Current(manifest) => manifest.plan_id,
            ManifestRecord::Legacy(manifest) => manifest.plan_id,
        };
        if stored_id != id {
            return Err(WorkflowError::Corrupt {
                path: self.layout.plan_manifest(id),
                reason: "plan manifest ID differs from its directory ID".to_owned(),
            });
        }
        Ok(manifest)
    }

    pub(super) fn read_run_manifest(
        &self,
        id: RunId,
    ) -> WorkflowResult<ManifestRecord<crate::workflow::RunManifest, LegacyRunManifest>> {
        let manifest = read_manifest::<crate::workflow::RunManifest, LegacyRunManifest>(
            self,
            &run_relative(id, Path::new("manifest.json")),
            "run manifest",
            crate::workflow::RUN_MANIFEST_VERSION,
        )?;
        let stored_id = match &manifest {
            ManifestRecord::Current(manifest) => manifest.run_id,
            ManifestRecord::Legacy(manifest) => manifest.run_id,
        };
        if stored_id != id {
            return Err(WorkflowError::Corrupt {
                path: self.layout.run_manifest(id),
                reason: "run manifest ID differs from its directory ID".to_owned(),
            });
        }
        Ok(manifest)
    }

    pub(super) fn read_legacy_run_state(&self, id: RunId) -> WorkflowResult<LegacyRunState> {
        let state: LegacyRunState =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        check_schema_version(
            "legacy run state",
            state.schema_version,
            LEGACY_RUN_STATE_VERSION,
        )?;
        Ok(state)
    }

    /// Reads a legacy run's stored snapshot for display only. It does not
    /// replay the journal or rehash artifacts, since the run cannot change.
    pub(super) fn load_legacy_run(
        &self,
        id: RunId,
        manifest: LegacyRunManifest,
    ) -> WorkflowResult<LegacyRun> {
        let graph: serde_json::Value =
            read_json(&self.root, &run_relative(id, Path::new("graph.json")))?;
        let graph_version = graph
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| WorkflowError::Corrupt {
                path: self.layout.run_graph(id),
                reason: "legacy frozen graph has no schema version".to_owned(),
            })?;
        if graph_version != u64::from(LEGACY_FROZEN_GRAPH_VERSION) {
            return Err(WorkflowError::UnsupportedVersion {
                kind: "legacy frozen graph",
                found: u32::try_from(graph_version).unwrap_or(u32::MAX),
                supported: LEGACY_FROZEN_GRAPH_VERSION,
            });
        }
        let state = self.read_legacy_run_state(id)?;
        Ok(LegacyRun {
            manifest,
            graph,
            state,
        })
    }
}

pub(super) fn legacy_record(kind: &'static str, id: impl ToString) -> WorkflowError {
    WorkflowError::LegacyRecord {
        kind,
        id: id.to_string(),
    }
}

#[cfg(test)]
#[path = "store_legacy_tests.rs"]
mod tests;
