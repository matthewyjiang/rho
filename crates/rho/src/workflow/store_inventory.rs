//! Plan/run inventory helpers for the durable workflow store.
//!
//! Hub and status UIs need name, lifecycle, and progress only. Full
//! `load_plan` / `load_run` rehash sources, re-digest graphs, and replay
//! event journals. Inventory reads the small on-disk fields the UI needs
//! and skips that validation path.
//!
//! Name and step counts come from manifests so the hot path never opens
//! the frozen program. Records written before scoped programs use version 1
//! manifests and flat run state; inventory reads both generations.
//! Lifecycle and progress still come from `state.json`.
//!
//! This module is projection only. Destructive store mutations live on
//! [`WorkflowStore`] in `store.rs`.

use std::{collections::BTreeMap, path::Path, str::FromStr};

use serde::Deserialize;

use super::{plan_relative, read_json, run_relative, WorkflowStore};
use crate::workflow::{
    check_schema_version, NodeId, NodeState, PlanId, RunId, RunLifecycle, ScopeInstanceId,
    WorkflowOutcome, WorkflowResult, PLAN_MANIFEST_VERSION, RUN_MANIFEST_VERSION,
};

/// Inventory owns generation policy; readers and mutation boundaries consume it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordAccess {
    Executable,
    ReadOnly,
}

impl RecordAccess {
    pub(super) fn from_version(
        kind: &'static str,
        version: u32,
        current: u32,
    ) -> WorkflowResult<Self> {
        if version == super::legacy::LEGACY_MANIFEST_VERSION {
            Ok(Self::ReadOnly)
        } else {
            check_schema_version(kind, version, current)?;
            Ok(Self::Executable)
        }
    }
}

#[derive(Deserialize)]
struct InventoryManifest {
    schema_version: u32,
    #[serde(default)]
    created_at_unix_nanos: u64,
    workspace_identity: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    step_count: usize,
}

#[derive(Deserialize)]
struct PlanSummary {
    plan_id: PlanId,
    #[serde(flatten)]
    metadata: InventoryManifest,
}

#[derive(Deserialize)]
struct RunSummary {
    run_id: RunId,
    #[serde(flatten)]
    metadata: InventoryManifest,
}

/// Lightweight plan row for workspace inventory UIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlanInventoryItem {
    pub(crate) plan_id: PlanId,
    pub(crate) access: RecordAccess,
    /// Persisted creation time.
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) workspace_identity: String,
    pub(crate) name: String,
    pub(crate) step_count: usize,
}

/// Lightweight run row for workspace inventory UIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunInventoryItem {
    pub(crate) run_id: RunId,
    /// Older generations support status and deletion, but not execution or watch.
    pub(crate) access: RecordAccess,
    /// Persisted creation time.
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) workspace_identity: String,
    pub(crate) name: String,
    pub(crate) lifecycle: RunLifecycle,
    pub(crate) outcome: Option<WorkflowOutcome>,
    pub(crate) done_steps: usize,
    pub(crate) total_steps: usize,
}

#[derive(Debug, Deserialize)]
struct InventoryStateFile {
    state: InventoryWorkflowState,
}

#[derive(Debug, Deserialize)]
struct InventoryWorkflowState {
    lifecycle: RunLifecycle,
    scopes: BTreeMap<ScopeInstanceId, InventoryScopeState>,
}

#[derive(Debug, Deserialize)]
struct InventoryScopeState {
    nodes: BTreeMap<NodeId, NodeState>,
    result: Option<InventoryScopeResult>,
}

#[derive(Debug, Deserialize)]
struct InventoryScopeResult {
    outcome: WorkflowOutcome,
}

#[derive(Debug, Deserialize)]
struct RevisionStateFile {
    state: RevisionOnly,
}

#[derive(Debug, Deserialize)]
struct RevisionOnly {
    revision: u64,
}

#[derive(Debug, Deserialize)]
struct LifecycleStateFile {
    state: LifecycleOnly,
}

#[derive(Debug, Deserialize)]
struct LifecycleOnly {
    lifecycle: RunLifecycle,
}

impl WorkflowStore {
    /// Lists plan rows without source rehash or graph re-digest.
    ///
    /// Non-UUID directory names are ignored. A valid plan ID directory that
    /// cannot be read fails the list; empty means empty, not broken.
    pub(crate) fn list_plan_inventory(&self) -> WorkflowResult<Vec<PlanInventoryItem>> {
        let mut plans = Vec::new();
        for name in self.root.directory_names(Path::new("plans"))? {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let Ok(id) = PlanId::from_str(&name) else {
                continue;
            };
            plans.push(self.read_plan_inventory(id)?);
        }
        plans.sort_by_key(|plan| std::cmp::Reverse((plan.created_at_unix_nanos, plan.plan_id)));
        Ok(plans)
    }

    /// Lists run rows without journal replay or frozen-graph validation.
    ///
    /// Non-UUID directory names (including trash names) are ignored. A valid
    /// run ID directory that cannot be read fails the list.
    pub(crate) fn list_run_inventory(&self) -> WorkflowResult<Vec<RunInventoryItem>> {
        let mut runs = Vec::new();
        for name in self.root.directory_names(Path::new("runs"))? {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let Ok(id) = RunId::from_str(&name) else {
                continue;
            };
            runs.push(self.read_run_inventory(id)?);
        }
        runs.sort_by_key(|run| std::cmp::Reverse((run.created_at_unix_nanos, run.run_id)));
        Ok(runs)
    }

    /// Reads the durable revision without journal replay or graph validation.
    pub(crate) fn read_run_revision(&self, id: RunId) -> WorkflowResult<u64> {
        let state: RevisionStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        Ok(state.state.revision)
    }

    /// Reads lifecycle without journal replay for watch cancellation polling.
    pub(crate) fn read_run_lifecycle(&self, id: RunId) -> WorkflowResult<RunLifecycle> {
        let state: LifecycleStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        Ok(state.state.lifecycle)
    }

    /// Reads one run inventory row without journal replay.
    ///
    /// Uses manifest metadata without opening the frozen program.
    pub(crate) fn read_run_inventory(&self, id: RunId) -> WorkflowResult<RunInventoryItem> {
        let summary: RunSummary =
            read_json(&self.root, &run_relative(id, Path::new("manifest.json")))?;
        if summary.run_id != id {
            return super::corrupt(
                &self.layout.run_manifest(id),
                "run manifest ID differs from its directory ID",
            );
        }
        let manifest = summary.metadata;
        let access = RecordAccess::from_version(
            "run manifest",
            manifest.schema_version,
            RUN_MANIFEST_VERSION,
        )?;
        if access == RecordAccess::ReadOnly {
            return self.read_legacy_run_inventory(id, manifest, access);
        }
        let state: InventoryStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        let scope = state
            .state
            .scopes
            .get(&ScopeInstanceId::ROOT)
            .ok_or_else(|| crate::workflow::WorkflowError::Corrupt {
                path: self.layout.run_state(id),
                reason: "run has no root scope instance".to_owned(),
            })?;
        Ok(RunInventoryItem {
            run_id: id,
            access,
            created_at_unix_nanos: manifest.created_at_unix_nanos,
            workspace_identity: manifest.workspace_identity,
            name: manifest.name,
            lifecycle: state.state.lifecycle,
            outcome: if state.state.lifecycle == RunLifecycle::Completed {
                scope.result.as_ref().map(|result| result.outcome)
            } else {
                None
            },
            done_steps: terminal_count(scope.nodes.values()),
            total_steps: manifest.step_count,
        })
    }

    // NEXT_MAJOR(rho-coding-agent): remove legacy version 1 run rows from workflow inventory.
    fn read_legacy_run_inventory(
        &self,
        id: RunId,
        manifest: InventoryManifest,
        access: RecordAccess,
    ) -> WorkflowResult<RunInventoryItem> {
        let state = self.read_legacy_run_state(id)?.state;
        Ok(RunInventoryItem {
            run_id: id,
            access,
            created_at_unix_nanos: manifest.created_at_unix_nanos,
            workspace_identity: manifest.workspace_identity,
            name: manifest.name,
            lifecycle: state.lifecycle,
            outcome: state.outcome,
            done_steps: state
                .nodes
                .values()
                .filter(|node| {
                    node.get("state").and_then(serde_json::Value::as_str) == Some("terminal")
                })
                .count(),
            total_steps: manifest.step_count,
        })
    }

    /// Reads one plan row. Accepts current and legacy read-only plans.
    pub(crate) fn read_plan_inventory(&self, id: PlanId) -> WorkflowResult<PlanInventoryItem> {
        let summary: PlanSummary =
            read_json(&self.root, &plan_relative(id, Path::new("manifest.json")))?;
        if summary.plan_id != id {
            return super::corrupt(
                &self.layout.plan_manifest(id),
                "plan manifest ID differs from its directory ID",
            );
        }
        let manifest = summary.metadata;
        // NEXT_MAJOR(rho-coding-agent): remove legacy version 1 plan rows from workflow inventory.
        let access = RecordAccess::from_version(
            "plan manifest",
            manifest.schema_version,
            PLAN_MANIFEST_VERSION,
        )?;
        Ok(PlanInventoryItem {
            plan_id: id,
            access,
            created_at_unix_nanos: manifest.created_at_unix_nanos,
            workspace_identity: manifest.workspace_identity,
            name: manifest.name,
            step_count: manifest.step_count,
        })
    }

    pub(super) fn require_executable_run(&self, id: RunId) -> WorkflowResult<()> {
        let summary: RunSummary =
            read_json(&self.root, &run_relative(id, Path::new("manifest.json")))?;
        if summary.run_id != id {
            return super::corrupt(
                &self.layout.run_manifest(id),
                "run manifest ID differs from its directory ID",
            );
        }
        match RecordAccess::from_version(
            "run manifest",
            summary.metadata.schema_version,
            RUN_MANIFEST_VERSION,
        )? {
            RecordAccess::Executable => Ok(()),
            RecordAccess::ReadOnly => Err(super::legacy::legacy_record("run", id)),
        }
    }
}

fn terminal_count<'a>(nodes: impl Iterator<Item = &'a NodeState>) -> usize {
    nodes.filter(|node| node.terminal().is_some()).count()
}
