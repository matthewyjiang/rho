//! Plan/run inventory helpers for the durable workflow store.
//!
//! Hub and status UIs need name, lifecycle, and progress only. Full
//! `load_plan` / `load_run` rehash sources, re-digest graphs, and replay
//! event journals. Inventory reads the small on-disk fields the UI needs
//! and skips that validation path.
//!
//! Name and step counts come from manifests so the hot path never opens
//! `graph.json`. Manifests written before those fields may still fall back
//! to a graph peek once; if the graph is gone, the label is `(unnamed)`.
//! Lifecycle and progress still come from `state.json`.
//!
//! This module is projection only. Destructive store mutations live on
//! [`WorkflowStore`] in `store.rs`.

use std::{collections::BTreeMap, path::Path, str::FromStr};

use serde::Deserialize;

use super::{plan_relative, read_json, run_relative, WorkflowStore};
use crate::workflow::{
    NodeId, NodeState, PlanId, PlanManifest, RunId, RunLifecycle, RunManifest, WorkflowOutcome,
    WorkflowResult,
};

/// Label used when a pre-field manifest has no graph fallback either.
const UNNAMED_WORKFLOW: &str = "(unnamed)";

/// Lightweight plan row for workspace inventory UIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlanInventoryItem {
    pub(crate) plan_id: PlanId,
    /// Persisted creation time. Legacy manifests use zero.
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) workspace_identity: String,
    pub(crate) name: String,
    pub(crate) step_count: usize,
}

/// Lightweight run row for workspace inventory UIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunInventoryItem {
    pub(crate) run_id: RunId,
    /// Persisted creation time. Legacy manifests use zero.
    pub(crate) created_at_unix_nanos: u64,
    pub(crate) workspace_identity: String,
    pub(crate) name: String,
    pub(crate) lifecycle: RunLifecycle,
    pub(crate) outcome: Option<WorkflowOutcome>,
    pub(crate) done_steps: usize,
    pub(crate) total_steps: usize,
}

#[derive(Debug, Deserialize)]
struct InventoryGraphFile {
    graph: InventoryGraphBody,
}

#[derive(Debug, Deserialize)]
struct InventoryGraphBody {
    name: String,
    nodes: BTreeMap<String, serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
struct InventoryStateFile {
    state: InventoryWorkflowState,
}

#[derive(Debug, Deserialize)]
struct InventoryWorkflowState {
    lifecycle: RunLifecycle,
    outcome: Option<WorkflowOutcome>,
    nodes: BTreeMap<NodeId, NodeState>,
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

/// Resolve inventory label + step count from the manifest, with a one-shot
/// graph fallback only for manifests written before those fields existed.
fn inventory_identity(
    name: String,
    step_count: usize,
    graph_relative: &Path,
    root: &crate::workflow::secure_fs::SecureDirectory,
) -> (String, usize) {
    if !name.is_empty() {
        return (name, step_count);
    }
    match read_json::<InventoryGraphFile>(root, graph_relative) {
        Ok(graph) => (graph.graph.name, graph.graph.nodes.len()),
        Err(_) if step_count > 0 => (UNNAMED_WORKFLOW.to_owned(), step_count),
        Err(_) => (UNNAMED_WORKFLOW.to_owned(), 0),
    }
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

    /// Reads one plan manifest for workspace checks without loading the graph.
    pub(crate) fn read_plan_manifest(&self, id: PlanId) -> WorkflowResult<PlanManifest> {
        read_json(&self.root, &plan_relative(id, Path::new("manifest.json")))
    }

    /// Reads the durable revision without journal replay or graph validation.
    pub(crate) fn read_run_revision(&self, id: RunId) -> WorkflowResult<u64> {
        let state: RevisionStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        Ok(state.state.revision)
    }

    /// Reads lifecycle without journal replay. Used by locked delete checks.
    pub(crate) fn read_run_lifecycle(&self, id: RunId) -> WorkflowResult<RunLifecycle> {
        let state: LifecycleStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        Ok(state.state.lifecycle)
    }

    /// Reads one run inventory row without journal replay.
    ///
    /// Prefers manifest name/step_count. Opens graph.json only for pre-field
    /// manifests that still lack a name.
    pub(crate) fn read_run_inventory(&self, id: RunId) -> WorkflowResult<RunInventoryItem> {
        let manifest: RunManifest =
            read_json(&self.root, &run_relative(id, Path::new("manifest.json")))?;
        if manifest.run_id != id {
            return Err(crate::workflow::WorkflowError::Corrupt {
                path: self.layout.run_manifest(id),
                reason: "run manifest ID differs from its directory ID".to_owned(),
            });
        }
        let state: InventoryStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        let (name, total_steps) = inventory_identity(
            manifest.name,
            manifest.step_count,
            &run_relative(id, Path::new("graph.json")),
            &self.root,
        );
        let done_steps = state
            .state
            .nodes
            .values()
            .filter(|node| node.terminal().is_some())
            .count();
        Ok(RunInventoryItem {
            run_id: id,
            created_at_unix_nanos: manifest.created_at_unix_nanos,
            workspace_identity: manifest.workspace_identity,
            name,
            lifecycle: state.state.lifecycle,
            outcome: state.state.outcome,
            done_steps,
            total_steps,
        })
    }

    fn read_plan_inventory(&self, id: PlanId) -> WorkflowResult<PlanInventoryItem> {
        let manifest: PlanManifest =
            read_json(&self.root, &plan_relative(id, Path::new("manifest.json")))?;
        if manifest.plan_id != id {
            return Err(crate::workflow::WorkflowError::Corrupt {
                path: self.layout.plan_manifest(id),
                reason: "plan manifest ID differs from its directory ID".to_owned(),
            });
        }
        let (name, step_count) = inventory_identity(
            manifest.name,
            manifest.step_count,
            &plan_relative(id, Path::new("graph.json")),
            &self.root,
        );
        Ok(PlanInventoryItem {
            plan_id: id,
            created_at_unix_nanos: manifest.created_at_unix_nanos,
            workspace_identity: manifest.workspace_identity,
            name,
            step_count,
        })
    }
}
