//! Plan/run inventory helpers for the durable workflow store.
//!
//! Hub and status UIs need name, lifecycle, and progress only. Full
//! `load_plan` / `load_run` rehash sources, re-digest graphs, and replay
//! event journals. Inventory reads the small on-disk fields the UI needs
//! and skips that validation path.
//!
//! Name and step counts come from manifests so the hot path never opens
//! the frozen program. Lifecycle and progress come from `state.json`.
//! Records with version 1 manifests (see `store_generation.rs`) are listed as
//! [`UnsupportedRecord`] rows read from the manifest alone, so users can see
//! and delete them.
//!
//! This module is projection only. Destructive store mutations live in
//! `store_mutate.rs`.

use std::{cmp::Reverse, collections::BTreeMap, path::Path, str::FromStr};

use serde::Deserialize;

use super::{
    generation::UNSUPPORTED_MANIFEST_VERSION, plan_relative, read_json, run_relative, WorkflowStore,
};
use crate::workflow::{
    check_schema_version, NodeId, NodeState, PlanId, RunId, RunLifecycle, ScopeInstanceId,
    WorkflowError, WorkflowOutcome, WorkflowResult, PLAN_MANIFEST_VERSION, RUN_MANIFEST_VERSION,
};

/// A plan or run saved before scoped workflow programs (manifest version 1).
/// This release lists and deletes it but cannot open it; Rho 2.x can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnsupportedRecord<Id> {
    pub(crate) id: Id,
    /// Persisted creation time, zero when the manifest predates it.
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) workspace_identity: String,
    pub(crate) name: String,
}

/// Inventory rows this release can open, plus unsupported records kept visible
/// so users can delete them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Inventory<T, Id> {
    pub(crate) items: Vec<T>,
    pub(crate) unsupported: Vec<UnsupportedRecord<Id>>,
}

impl<T, Id> Default for Inventory<T, Id> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            unsupported: Vec::new(),
        }
    }
}

pub(crate) type PlanInventory = Inventory<PlanInventoryItem, PlanId>;
pub(crate) type RunInventory = Inventory<RunInventoryItem, RunId>;

/// One store entry, read by generation.
pub(super) enum InventoryRow<T, Id> {
    Current(T),
    Unsupported(UnsupportedRecord<Id>),
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

impl InventoryManifest {
    fn unsupported<Id>(self, id: Id) -> UnsupportedRecord<Id> {
        UnsupportedRecord {
            id,
            created_at_unix_nanos: self.created_at_unix_nanos,
            workspace_identity: self.workspace_identity,
            name: self.name,
        }
    }
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
    pub(crate) fn list_plan_inventory(&self) -> WorkflowResult<PlanInventory> {
        let mut inventory = PlanInventory::default();
        for name in self.root.directory_names(Path::new("plans"))? {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let Ok(id) = PlanId::from_str(&name) else {
                continue;
            };
            match self.read_plan_row(id)? {
                InventoryRow::Current(plan) => inventory.items.push(plan),
                InventoryRow::Unsupported(record) => inventory.unsupported.push(record),
            }
        }
        inventory
            .items
            .sort_by_key(|plan| Reverse((plan.created_at_unix_nanos, plan.plan_id)));
        inventory
            .unsupported
            .sort_by_key(|record| Reverse((record.created_at_unix_nanos, record.id)));
        Ok(inventory)
    }

    /// Lists run rows without journal replay or frozen-graph validation.
    ///
    /// Non-UUID directory names (including trash names) are ignored. A valid
    /// run ID directory that cannot be read fails the list.
    pub(crate) fn list_run_inventory(&self) -> WorkflowResult<RunInventory> {
        let mut inventory = RunInventory::default();
        for name in self.root.directory_names(Path::new("runs"))? {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let Ok(id) = RunId::from_str(&name) else {
                continue;
            };
            match self.read_run_row(id)? {
                InventoryRow::Current(run) => inventory.items.push(run),
                InventoryRow::Unsupported(record) => inventory.unsupported.push(record),
            }
        }
        inventory
            .items
            .sort_by_key(|run| Reverse((run.created_at_unix_nanos, run.run_id)));
        inventory
            .unsupported
            .sort_by_key(|record| Reverse((record.created_at_unix_nanos, record.id)));
        Ok(inventory)
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

    /// Workspace that owns a plan of either generation, for delete checks.
    pub(crate) fn plan_workspace_identity(&self, id: PlanId) -> WorkflowResult<String> {
        Ok(match self.read_plan_row(id)? {
            InventoryRow::Current(plan) => plan.workspace_identity,
            InventoryRow::Unsupported(record) => record.workspace_identity,
        })
    }

    /// Workspace that owns a run of either generation, for delete checks.
    pub(crate) fn run_workspace_identity(&self, id: RunId) -> WorkflowResult<String> {
        Ok(match self.read_run_row(id)? {
            InventoryRow::Current(run) => run.workspace_identity,
            InventoryRow::Unsupported(record) => record.workspace_identity,
        })
    }

    /// Reads one run row without journal replay or opening the frozen program.
    /// Unsupported runs come from the manifest alone; their state is not read.
    pub(super) fn read_run_row(
        &self,
        id: RunId,
    ) -> WorkflowResult<InventoryRow<RunInventoryItem, RunId>> {
        let summary: RunSummary =
            read_json(&self.root, &run_relative(id, Path::new("manifest.json")))?;
        if summary.run_id != id {
            return super::corrupt(
                &self.layout.run_manifest(id),
                "run manifest ID differs from its directory ID",
            );
        }
        let manifest = summary.metadata;
        if manifest.schema_version == UNSUPPORTED_MANIFEST_VERSION {
            return Ok(InventoryRow::Unsupported(manifest.unsupported(id)));
        }
        check_schema_version(
            "run manifest",
            manifest.schema_version,
            RUN_MANIFEST_VERSION,
        )?;
        let state: InventoryStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        let scope = state
            .state
            .scopes
            .get(&ScopeInstanceId::ROOT)
            .ok_or_else(|| WorkflowError::Corrupt {
                path: self.layout.run_state(id),
                reason: "run has no root scope instance".to_owned(),
            })?;
        Ok(InventoryRow::Current(RunInventoryItem {
            run_id: id,
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
        }))
    }

    /// Reads one plan row from its manifest.
    pub(super) fn read_plan_row(
        &self,
        id: PlanId,
    ) -> WorkflowResult<InventoryRow<PlanInventoryItem, PlanId>> {
        let summary: PlanSummary =
            read_json(&self.root, &plan_relative(id, Path::new("manifest.json")))?;
        if summary.plan_id != id {
            return super::corrupt(
                &self.layout.plan_manifest(id),
                "plan manifest ID differs from its directory ID",
            );
        }
        let manifest = summary.metadata;
        if manifest.schema_version == UNSUPPORTED_MANIFEST_VERSION {
            return Ok(InventoryRow::Unsupported(manifest.unsupported(id)));
        }
        check_schema_version(
            "plan manifest",
            manifest.schema_version,
            PLAN_MANIFEST_VERSION,
        )?;
        Ok(InventoryRow::Current(PlanInventoryItem {
            plan_id: id,
            created_at_unix_nanos: manifest.created_at_unix_nanos,
            workspace_identity: manifest.workspace_identity,
            name: manifest.name,
            step_count: manifest.step_count,
        }))
    }
}

fn terminal_count<'a>(nodes: impl Iterator<Item = &'a NodeState>) -> usize {
    nodes.filter(|node| node.terminal().is_some()).count()
}
